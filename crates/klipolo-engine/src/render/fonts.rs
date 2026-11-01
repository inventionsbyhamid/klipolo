//! The fonts the engine draws with. Klipolo's managed faces are registered in
//! every font database under their aliases, from their files (memory-mapped by fontdb, never compiled in):
//! - every bundled face, at its real weight and style (`kl:b:<family>`);
//! - every downloaded family registered here (`register`), the same way (`kl:g:<family>:<version>`);
//! - the fallback aliases at all nine weights in both styles, each pointing at its nearest real file:
//!   `kl:fallback:latin` (Inter) and `kl:fallback:notdef` (Adobe NotDef's boxes, the last CJK fallback). The CJK
//!   aliases `kl:fallback:{sc,tc,jp,kr}` exist only once their Noto Sans is downloaded and registered here
//!   (`index().cjk(script)`, the downloaded Noto Sans SC, TC, JP or KR only, never a system face).
//!
//! This computer's own fonts are in every scanned font system under their own names (fontdb's scan, plus the files a
//! font manager activated: `installed`), and in the engine's index once the scan publishes them. An export
//! worker takes the app's scan from its job instead of scanning again (`recorded`).
//!
//! cosmic-text 0.14 keeps only exact-weight match keys and needs the exact style, so a text asks for exactly the face
//! `klipolo_fonts::resolve` picked (`family_for`), and fallbacks answer at every weight a face can have. The engine's own
//! font index (`index()`) is built from the same sources as the app's: a download registered here resolves here, with
//! its coverage, and so does an installed family once the scan has found it.
//!
//! `configure` is called once (the app's set-up or the export worker); before it, `bundled_dir()`'s order applies.
//! `register` and `unregister` run in any order, before or after the first font system, and bump `generation()`, which
//! makes the next font system (and the text drawers holding one) take the change.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use klipolo_fonts::bundled::bundled_dir;
use klipolo_fonts::cjk::CjkCover;
use klipolo_fonts::fallback::Fallback;
use klipolo_fonts::index::{Downloaded, FontIndex};
use klipolo_fonts::installed::NORMAL_WIDTH;
use klipolo_fonts::resolve::{Face, resolve, resolve_fallback};
use klipolo_model::font::{FontRef, FontSource};
use klipolo_model::text_language::CjkScript;

#[path = "fonts_installed.rs"]
mod installed;
#[path = "fonts_managed.rs"]
mod managed;
#[path = "fonts_recorded.rs"]
mod recorded;
#[path = "fonts_testing.rs"]
mod testing;
#[path = "fonts_twins.rs"]
mod twins;
pub(crate) use installed::publish_installed;
pub use installed::{installed_families, subscribe_scanned};
pub use managed::ManagedFace;
pub(crate) use managed::{add_managed, push_managed, width_of};
pub(crate) use recorded::take_given as take_recorded_scan;
pub use recorded::{record_scan, use_recorded_scan};
#[doc(hidden)]
pub use testing::{
    TestFace, test_fallback_face, test_group_installed, test_index, test_installed_draw,
    test_installed_shape, test_record_scan_of, test_shape_face, test_system_fallback_face,
    test_template_face,
};
pub(crate) use twins::width_alias;

/// Where the engine's fonts come from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontDirs {
    /// The bundled fonts folder; None for a broken install (texts draw in the system sans-serif and say so).
    pub bundled: Option<PathBuf>,
}

/// A second `configure`: its caller logs it; the first folder stays.
#[derive(Debug, PartialEq, Eq)]
pub struct AlreadyConfigured;

impl std::fmt::Display for AlreadyConfigured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the engine's fonts were already configured")
    }
}

impl std::error::Error for AlreadyConfigured {}

static DIRS: OnceLock<FontDirs> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The downloaded families registered with the engine (`register`), as their manifests describe them.
static DOWNLOADED: RwLock<Vec<Downloaded>> = RwLock::new(Vec::new());
/// The engine's index, with the generation it was built at.
static INDEX: RwLock<Option<(u64, Arc<FontIndex>)>> = RwLock::new(None);

/// Sets where the bundled fonts are, once per process (the app's set-up, or the export worker). A second call is an
/// error, which the caller logs: it never silently replaces the first.
pub fn configure(dirs: FontDirs) -> Result<(), AlreadyConfigured> {
    DIRS.set(dirs).map_err(|_| AlreadyConfigured)?;
    // A font system made before (the GPU warm-up's) registered from the default folder: the next one re-registers.
    GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// Tests: `bundled_dir()`'s folder, the repository's `assets/fonts` in a debug build (a release run sets
/// `KLIPOLO_FONTS_DIR`). Every test of a binary calls it, so a second call is fine here.
pub fn configure_for_tests() {
    let _ = configure(FontDirs {
        bundled: bundled_dir(),
    });
}

/// Where this process's fonts come from: the configured folders, else `bundled_dir()`'s order (until `configure`
/// runs). An export job records it (`ExportJob::fonts_dir`), so its worker draws from the very folder the
/// app does.
pub fn dirs() -> FontDirs {
    DIRS.get().cloned().unwrap_or_else(|| FontDirs {
        bundled: bundled_dir(),
    })
}

/// The bundled folder of `dirs()`.
fn bundled() -> Option<PathBuf> {
    dirs().bundled
}

/// Bumped by `configure`, `register` and `unregister`: font systems made at an older one take the change.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

/// Registers downloaded families (each replacing a registration of the same family and version), with what their
/// manifests say: their exact version, faces and coverage, so the engine's own index resolves them (the app calls it
/// at start-up and after a download, and the export worker before frame 0).
pub fn register(families: Vec<Downloaded>) {
    {
        let mut downloaded = DOWNLOADED.write().unwrap_or_else(PoisonError::into_inner);
        for family in families {
            downloaded.retain(|d| d.family != family.family || d.version != family.version);
            downloaded.push(family);
        }
    }
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// The downloaded families registered with the engine now: an export job records them (`ExportJob::downloads`), so
/// its worker registers the same faces before frame 0, the CJK fallbacks' Noto Sans among them.
pub fn downloads() -> Vec<Downloaded> {
    DOWNLOADED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Forgets the downloaded family registered under `alias` (`kl:g:<family>:<version>`): texts using it draw the
/// stand-in and say so.
pub fn unregister(alias: &str) {
    DOWNLOADED
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|d| d.alias() != alias);
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// The engine's view of the fonts it can draw, built from the same sources as the app's (bundled, installed once the
/// scan has found them, downloaded). Rebuilt once per generation; the resolver itself only ever takes it as a value.
pub fn index() -> Arc<FontIndex> {
    let now = generation();
    if let Some((built, index)) = INDEX
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        && *built == now
    {
        return index.clone();
    }
    let downloaded = DOWNLOADED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let index = Arc::new(
        FontIndex::shipped(bundled())
            .with_installed_list(installed_families())
            .with_downloaded(downloaded),
    );
    *INDEX.write().unwrap_or_else(PoisonError::into_inner) = Some((now, index.clone()));
    index
}

/// Whether `kl:fallback:<script>` exists, i.e. its Noto Sans is downloaded. Until then CJK text of that script
/// draws Adobe NotDef's boxes and the frame says `frame.cjkFontMissing`.
pub fn cjk_registered(script: CjkScript) -> bool {
    resolve_fallback(&index(), Fallback::Cjk(script), 400, false).is_some()
}

/// The face a text asks cosmic-text for: exactly the one `resolve` picked, at its own weight, style and width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceChoice {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    /// The width class (5 normal) cosmic-text is asked for: it finds a face only at the asked width. An installed
    /// face of another width is asked at 5 under its width twin's alias (`family`), so this is 5 for every face now.
    pub stretch: u16,
    /// The font isn't here: this is Inter's nearest face (or the system sans-serif for a broken install).
    pub stand_in: bool,
    /// The CJK the face draws itself: its characters need no CJK fallback.
    pub cjk: CjkCover,
    /// No file: a broken install's last resort, shaped as the generic sans-serif.
    pub no_file: bool,
}

/// `resolve`'s answer as the engine shapes with it, with the CJK the face covers. A family with only italic faces is
/// asked for upright, where it is registered too (`managed`): the system's script faces are upright, and cosmic-text
/// only falls back to faces of the asked style, so its Thai or Devanagari never boxes. For
/// the same reason a face that isn't normal width (an installed condensed one) is asked at normal width under its width
/// twin's alias (`width_alias`): it draws its own glyphs, at its own width, and keeps the system's script
/// faces for the letters it lacks.
pub fn family_for(index: &FontIndex, font: &FontRef, weight: u16, italic: bool) -> FaceChoice {
    let resolved = resolve(index, font, weight, italic);
    let cjk = index.cjk_cover(&resolved.face);
    let upright = !resolved.stand_in && index.italic_only(font);
    let Face {
        alias,
        weight,
        italic,
        stretch,
        file,
    } = resolved.face;
    // Only installed faces have other widths, and their width twins (`managed`).
    let twin = stretch != NORMAL_WIDTH
        && !resolved.stand_in
        && file.is_some()
        && matches!(font.source, FontSource::Installed);
    let (family, stretch) = if twin {
        (width_alias(&alias, stretch), NORMAL_WIDTH)
    } else {
        (alias, stretch)
    };
    FaceChoice {
        family,
        weight,
        italic: italic && !upright,
        stretch,
        stand_in: resolved.stand_in,
        cjk,
        no_file: file.is_none(),
    }
}
