//! The fonts the Font menu offers and a text can draw with: the engine's index (the bundled set, this computer's fonts
//! once the engine's one scan has found them, and the families downloaded from Google, registered with the engine) with
//! Google's catalogue, decoded once off the main thread, so the menu also lists what can be downloaded. The engine's
//! generation moves on whenever its fonts change (a scan, a download), and the index is built again then.
//!
//! A search or a browse waits for the scan, the catalogue and start-up's registration of the downloads (at most
//! `SCAN_WAIT` for all), so it never misses this computer's fonts or Google's.

pub mod cjk;
pub mod downloads;
pub mod google;

#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use klipolo_fonts::google::Catalogue;
use klipolo_fonts::index::FontIndex;
use klipolo_fonts::missing::{MissingFont, missing_in};
use klipolo_fonts::resolve::resolve;
use klipolo_fonts::search::{FontRow, SearchFilter, search};
use klipolo_model::coded::CodedError;
use klipolo_model::font::FontRef;
use klipolo_model::project::Project;
use serde::Serialize;
use tokio::sync::watch;
use ts_rs::TS;

/// The most rows a typed search answers from each source (bundled, Google's, this computer's). Browsing (an empty
/// query) answers every family, in sections, windowed in the page.
pub const MAX_ROWS: usize = 200;
/// How long a search waits for the scan, the catalogue and the downloads before answering without them.
const SCAN_WAIT: Duration = Duration::from_secs(3);

/// Where the index's fonts come from: the engine's (the app), or a fixed index (tests).
enum Base {
    Engine,
    Fixed(Arc<FontIndex>),
}

/// The Font menu's index and what it waits for.
pub struct Fonts {
    base: Base,
    catalogue: OnceLock<Arc<Catalogue>>,
    catalogue_ready: watch::Sender<bool>,
    downloads_ready: watch::Sender<bool>,
    /// The index last built, with the engine's generation and whether the catalogue was in.
    built: RwLock<Option<(u64, bool, Arc<FontIndex>)>>,
}

impl Fonts {
    /// The engine's fonts; the catalogue and the downloads are still to come (`publish_catalogue`, `downloads::start`).
    pub fn new() -> Self {
        Self::with_base(Base::Engine)
    }

    /// A fixed index for tests: nothing to scan, the catalogue and the downloads still to come.
    pub fn for_tests(index: FontIndex) -> Self {
        Self::with_base(Base::Fixed(Arc::new(index)))
    }

    fn with_base(base: Base) -> Self {
        Self {
            base,
            catalogue: OnceLock::new(),
            catalogue_ready: watch::channel(false).0,
            downloads_ready: watch::channel(false).0,
            built: RwLock::new(None),
        }
    }

    /// The index now: the engine's fonts, with Google's catalogue once it is decoded.
    pub fn index(&self) -> Arc<FontIndex> {
        let generation = match self.base {
            Base::Engine => klipolo_engine::render::fonts::generation(),
            Base::Fixed(_) => 0,
        };
        let catalogue = self.catalogue.get();
        if let Some((g, with, index)) = self
            .built
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            && *g == generation
            && *with == catalogue.is_some()
        {
            return index.clone();
        }
        let base = match &self.base {
            Base::Engine => klipolo_engine::render::fonts::index(),
            Base::Fixed(index) => index.clone(),
        };
        let index = Arc::new(match catalogue {
            Some(c) => (*base).clone().with_catalogue(c.clone()),
            None => (*base).clone(),
        });
        *self.built.write().unwrap_or_else(PoisonError::into_inner) =
            Some((generation, catalogue.is_some(), index.clone()));
        index
    }

    /// Google's catalogue, decoded: the index takes it (shared, never copied), and searches waiting for it answer.
    pub fn publish_catalogue(&self, catalogue: Arc<Catalogue>) {
        let _ = self.catalogue.set(catalogue);
        self.catalogue_ready.send_replace(true);
    }

    /// Start-up's registration of the downloads is done: downloads and searches waiting for it go on.
    pub fn downloads_started(&self) {
        self.downloads_ready.send_replace(true);
    }

    /// Waits while start-up reads the download store (at most 30 s).
    pub async fn downloads_ready(&self) {
        let mut ready = self.downloads_ready.subscribe();
        let _ = tokio::time::timeout(Duration::from_secs(30), ready.wait_for(|done| *done)).await;
    }

    /// Waits while Google's catalogue decodes at start-up (at most `SCAN_WAIT`): a download asked for at once still
    /// finds its family.
    pub async fn catalogue_ready(&self) {
        let mut ready = self.catalogue_ready.subscribe();
        let _ = tokio::time::timeout(SCAN_WAIT, ready.wait_for(|done| *done)).await;
    }

    /// Waits for this computer's fonts, the catalogue and the downloads (at most `SCAN_WAIT` for all): a search
    /// before them would miss this computer's fonts or Google's. Once they are in, nothing waits.
    pub async fn after_scan(&self) {
        let mut catalogue = self.catalogue_ready.subscribe();
        let mut downloads = self.downloads_ready.subscribe();
        let scanned = matches!(self.base, Base::Engine)
            .then(klipolo_engine::render::fonts::subscribe_scanned);
        let all = async {
            if let Some(mut scanned) = scanned {
                let _ = scanned.wait_for(|done| *done).await;
            }
            let _ = catalogue.wait_for(|done| *done).await;
            let _ = downloads.wait_for(|done| *done).await;
        };
        let _ = tokio::time::timeout(SCAN_WAIT, all).await;
    }

    /// `query` among the fonts, once they are in (or the wait is over): the menu's one search.
    pub async fn search_after_scan(&self, query: &str, filter: SearchFilter) -> Vec<FontRow> {
        self.after_scan().await;
        let limit = if query.trim().is_empty() {
            usize::MAX
        } else {
            MAX_ROWS
        };
        search(&self.index(), query, &filter, limit)
    }

    /// Whether this computer's fonts, Google's catalogue and the downloads are all in the index.
    fn settled(&self) -> bool {
        let scanned = match self.base {
            Base::Engine => *klipolo_engine::render::fonts::subscribe_scanned().borrow(),
            Base::Fixed(_) => true,
        };
        scanned && *self.catalogue_ready.borrow() && *self.downloads_ready.borrow()
    }

    /// The fonts `project` (its text in `text_language`) uses that aren't here, once this computer's fonts, the catalogue
    /// and the downloads are in: an installed font is never called missing because the scan hadn't finished. Still not
    /// in after the wait (`SCAN_WAIT`): `Pending`, and the page asks again.
    pub async fn missing_after_scan(
        &self,
        project: &Project,
        text_language: &str,
    ) -> MissingAnswer {
        self.after_scan().await;
        if !self.settled() {
            return MissingAnswer::Pending;
        }
        MissingAnswer::Ready {
            fonts: missing_in(&self.index(), project, text_language),
        }
    }
}

/// The open project's missing fonts, once this computer's are read (`fonts_missing`); `Pending` while they aren't.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum MissingAnswer {
    Pending,
    Ready { fonts: Vec<MissingFont> },
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}

/// The bytes of the face `font` draws with at `weight` and `italic`, as the engine resolves it: a bundled,
/// downloaded or installed file, read whole (at most `MAX_FACE_BYTES`); a stand-in's when the font isn't here. Only
/// files the index holds are read: a family names one of them, never a path. A blocking call.
pub fn face_bytes(
    index: &FontIndex,
    font: &FontRef,
    weight: u16,
    italic: bool,
) -> Result<Vec<u8>, CodedError> {
    let missing = || klipolo_fonts::english::missing(&font.family);
    let resolved = resolve(index, font, weight, italic);
    let file = resolved.face.file.as_ref().ok_or_else(missing)?;
    klipolo_fonts::sfnt::read_face(&file.path, file.index, klipolo_fonts::sfnt::MAX_FACE_BYTES)
        .map_err(|_| missing())
}
