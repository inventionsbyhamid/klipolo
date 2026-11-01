//! Google Fonts downloads: only after the person's one in-place consent and while Settings' switch is on (refused
//! otherwise, whatever the page shows), straight from Google into the store (`<app data>/fonts/google`), one download
//! per family at a time, cancellable. A finished family is registered with the engine (`render::fonts::register`),
//! whose index the Font menu's is built on, and `fonts-changed` tells the page, which asks its faces again; the texts
//! using it redraw in it, and an export draws it (the job records the engine's downloads).
//!
//! Start-up (`start`) tidies what a quit or crash left, then registers every downloaded family; a download waits for
//! it. Previews go at most four at a time.
//!
//! The core takes a `Host` (the setting and the page's events), so the tests run it without a window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_fonts::download::{FontError, download_family, fetch_preview, which_for};
use klipolo_fonts::english::{google_off, needs_consent, unknown_google, version_gone};
use klipolo_fonts::google::GoogleFamily;
use klipolo_fonts::store::FontStore;
use klipolo_model::coded::CodedError;
use klipolo_model::font::FontRef;

use super::Fonts;
use super::google::GoogleFonts;

/// The online previews' cache: at most this much on disk, the oldest used first out.
pub const PREVIEWS_MAX: u64 = 20 * 1024 * 1024;

/// What the downloads need of the app: Google Fonts' setting, and the page's events.
pub trait Host: Send + Sync {
    fn google(&self) -> GoogleFonts;
    /// Changes and saves the setting; the setting as saved.
    fn set_google(&self, change: &dyn Fn(&mut GoogleFonts)) -> Result<GoogleFonts, CodedError>;
    /// `fonts-download { family, done, total }`.
    fn progress(&self, family: &str, done: u64, total: u64);
    /// `fonts-changed { family }`.
    fn changed(&self, family: &str);
}

/// Previews in flight at once, at most.
const PREVIEWS_AT_ONCE: usize = 4;

/// The store, the previews' folder and the downloads running now. Cheap to clone.
#[derive(Clone)]
pub struct Downloads(Arc<Inner>);

struct Inner {
    /// None when the app data folder couldn't be made: every download says why.
    store: Option<FontStore>,
    previews: Option<PathBuf>,
    /// One download per family at a time: two picks of one family wait for each other.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Each family's cancel flag, shared by its downloads running and waiting their turn (so Cancel reaches them all),
    /// with how many there are: it goes when the last one ends.
    cancels: Mutex<HashMap<String, (Arc<AtomicBool>, usize)>>,
    /// Requests let through to the network (tests: none before consent or while off).
    asked: AtomicUsize,
    previews_gate: tokio::sync::Semaphore,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A store error as a code.
fn failed(e: FontError) -> CodedError {
    if !matches!(e, FontError::Cancelled) {
        tracing::warn!(error = %e, "fonts: a download didn't finish");
    }
    e.code()
}

impl Downloads {
    /// The store in `<data>/fonts/google` and the previews in `<data>/fonts/previews`.
    pub fn open(data: Option<&Path>) -> Downloads {
        let store = data.and_then(|d| match FontStore::open(d.join("fonts").join("google")) {
            Ok(store) => Some(store),
            Err(e) => {
                tracing::warn!(error = %e, "fonts: the downloads folder couldn't be made");
                None
            }
        });
        Downloads(Arc::new(Inner {
            store,
            previews: data.map(|d| d.join("fonts").join("previews")),
            locks: Mutex::default(),
            cancels: Mutex::default(),
            asked: AtomicUsize::new(0),
            previews_gate: tokio::sync::Semaphore::new(PREVIEWS_AT_ONCE),
        }))
    }

    pub fn store(&self) -> Option<&FontStore> {
        self.0.store.as_ref()
    }

    /// Requests that were let through to the network so far (tests).
    pub fn asked(&self) -> usize {
        self.0.asked.load(Ordering::SeqCst)
    }

    /// One download of `family` at a time: two picks of one family wait for each other, since they write one manifest.
    fn family_lock(&self, family: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.0.locks)
            .entry(family.to_owned())
            .or_default()
            .clone()
    }

    /// Stops `family`'s downloads, the one running (between two chunks, its finished files kept) and any waiting their
    /// turn (before they ask anything). A download asked after this one gets a fresh flag: a pick made while the
    /// cancelled one is still ending downloads, never ends as that cancel.
    pub fn cancel(&self, family: &str) {
        if let Some((flag, _)) = lock(&self.0.cancels).remove(family) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// Every download's flag (Google Fonts switched off); later ones start fresh, as after `cancel`.
    fn cancel_all(&self) {
        for (_, (flag, _)) in lock(&self.0.cancels).drain() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// `family`'s cancel flag, for one more download of it (`done` when that one ends).
    fn cancel_flag(&self, family: &str) -> Arc<AtomicBool> {
        let mut all = lock(&self.0.cancels);
        let entry = all
            .entry(family.to_owned())
            .or_insert_with(|| (Arc::new(AtomicBool::new(false)), 0));
        entry.1 += 1;
        entry.0.clone()
    }

    /// One download of `family` under `flag` ended: the entry goes with the last of them, and never a newer entry a
    /// cancel made meanwhile (matched by the flag itself).
    fn done(&self, family: &str, flag: &Arc<AtomicBool>) {
        let mut all = lock(&self.0.cancels);
        if let Some(entry) = all.get_mut(family).filter(|e| Arc::ptr_eq(&e.0, flag)) {
            entry.1 -= 1;
            if entry.1 == 0 {
                all.remove(family);
            }
        }
    }
}

/// The start: what a quit or crash left goes, then every downloaded family is registered with the engine (searches
/// and downloads wait for it). On a blocking thread: it reads each family's manifest and checks its files.
pub fn start(fonts: &Fonts, downloads: &Downloads) {
    if let Some(store) = downloads.store() {
        // An unfinished download at a version Google no longer offers can't resume: it goes whole.
        let catalogue = klipolo_fonts::google::shipped();
        let offered = catalogue
            .families
            .iter()
            .map(|g| (g.family.as_str(), g.version.as_str()));
        match store.tidy_keeping(offered) {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                removed = n,
                "fonts: what unfinished downloads left was removed"
            ),
            Err(e) => {
                tracing::warn!(error = %e, "fonts: what unfinished downloads left couldn't be removed")
            }
        }
        let found = store.downloaded();
        if !found.is_empty() {
            klipolo_engine::render::fonts::register(found);
        }
    }
    fonts.downloads_started();
}

/// Refused unless Settings' switch is on and the person has confirmed once: no request otherwise.
fn allowed(host: &dyn Host) -> Result<(), CodedError> {
    let g = host.google();
    if !g.enabled {
        return Err(google_off());
    }
    if !g.consented {
        return Err(needs_consent());
    }
    Ok(())
}

/// The catalogue's family named `family` (as the menu lists it).
fn find(fonts: &Fonts, family: &str) -> Result<GoogleFamily, CodedError> {
    fonts
        .index()
        .catalogue()
        .and_then(|c| c.find(&family.trim().to_lowercase()))
        .cloned()
        .ok_or_else(|| unknown_google(family))
}

/// Downloads `family` (`version`: a project's exact one, refused with `versionGone` unless Google still serves it;
/// None: the catalogue's) for a text at `weight` and `italic`: the whole family up to 15 MB, else that face. Then the
/// engine has it and the page hears `fonts-changed`. Its `FontRef`, at the version downloaded.
pub async fn download(
    host: Arc<dyn Host>,
    fonts: &Fonts,
    downloads: &Downloads,
    family: &str,
    version: Option<&str>,
    weight: u16,
    italic: bool,
) -> Result<FontRef, CodedError> {
    allowed(&*host)?;
    let fam = find(fonts, family)?;
    if let Some(v) = version.filter(|v| *v != fam.version) {
        return Err(version_gone(&fam.family, v));
    }
    let which = which_for(&fam, weight, italic);
    let store = downloads
        .store()
        .cloned()
        .ok_or_else(|| FontError::Disk("no downloads folder".into()).code())?;
    let family = fam.family.clone();
    // Its cancel flag first: a Cancel reaches it while it waits its turn too.
    let cancel = downloads.cancel_flag(&family);
    let got: Result<klipolo_fonts::store::Manifest, CodedError> = async {
        // Never while start-up's tidy runs: it removes what a quit left half-done.
        fonts.downloads_ready().await;
        let held = downloads.family_lock(&family);
        let _one_at_a_time = held.lock().await;
        // Switched off or cancelled while it waited its turn: still nothing goes out.
        allowed(&*host)?;
        if cancel.load(Ordering::SeqCst) {
            return Err(FontError::Cancelled.code());
        }
        downloads.0.asked.fetch_add(1, Ordering::SeqCst);
        let (tell, name, stop) = (host.clone(), family.clone(), cancel.clone());
        let (manifest, mine) = tokio::task::spawn_blocking(move || {
            let progress = |done, total| tell.progress(&name, done, total);
            let manifest = download_family(&store, &fam, which, &progress, &stop)?;
            // This family's faces as the store has them now, any weight fetched before.
            let mine: Vec<_> = store
                .downloaded()
                .into_iter()
                .filter(|d| d.family == manifest.family && d.version == manifest.version)
                .collect();
            Ok::<_, FontError>((manifest, mine))
        })
        .await
        .map_err(|_| FontError::Disk("the download stopped".into()).code())?
        .map_err(failed)?;
        klipolo_engine::render::fonts::register(mine);
        Ok(manifest)
    }
    .await;
    downloads.done(&family, &cancel);
    let manifest = got?;
    host.changed(&manifest.family);
    Ok(manifest.font())
}

/// The online preview of `family`'s name (Google's small `menu` file), refused as a download is: the person's consent
/// first, never while Google Fonts is off.
pub async fn preview(
    host: &dyn Host,
    fonts: &Fonts,
    downloads: &Downloads,
    family: &str,
) -> Result<Vec<u8>, CodedError> {
    allowed(host)?;
    let fam = find(fonts, family)?;
    let cache = downloads
        .0
        .previews
        .clone()
        .ok_or_else(|| FontError::Disk("no previews folder".into()).code())?;
    // At most four at once, and the switch and the consent asked again once it's this one's turn.
    let _turn = downloads
        .0
        .previews_gate
        .acquire()
        .await
        .map_err(|_| FontError::Offline.code())?;
    allowed(host)?;
    downloads.0.asked.fetch_add(1, Ordering::SeqCst);
    tokio::task::spawn_blocking(move || fetch_preview(&fam, &cache, PREVIEWS_MAX))
        .await
        .map_err(|_| FontError::Offline.code())?
        .map_err(|e| e.code())
}

/// The person's one consent ("Download from Google Fonts?").
pub fn consent(host: &dyn Host) -> Result<GoogleFonts, CodedError> {
    host.set_google(&|g| g.consented = true)
}

/// Settings' switch. Off stops every download running now, between two chunks (finished files stay).
pub fn set_enabled(
    host: &dyn Host,
    downloads: &Downloads,
    enabled: bool,
) -> Result<GoogleFonts, CodedError> {
    if !enabled {
        downloads.cancel_all();
    }
    host.set_google(&|g| g.enabled = enabled)
}
