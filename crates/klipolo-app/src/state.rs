//! What the app holds for the preview: the one engine and its caches. The open project is the project session's
//! (`project::ProjectSessionState`), which publishes it to the engine's feed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_engine::audio::pcm::PcmStore;
use klipolo_engine::render::warm::Warmup;
use klipolo_engine::session::EngineSlot;

/// The preview engine: started by the first attach and kept for the app's lifetime (started again if it ended).
pub struct EngineState {
    pub(crate) slot: EngineSlot,
    /// The GPU warm-up started at launch, until the engine's first start takes its backend.
    warm: Mutex<Option<Warmup>>,
    /// The app's cache folder (decoded sound, thumbnails); None when it couldn't be found.
    pub(crate) cache_dir: Option<PathBuf>,
    /// The decoded sound cache (`<cache>/audio`), one for the app: the preview plays from it and analysis reads its
    /// levels, so a file is decoded once. None without a cache folder.
    pub(crate) sounds: Option<Arc<PcmStore>>,
}

impl EngineState {
    pub fn new(warm: Option<Warmup>, cache_dir: Option<PathBuf>) -> Self {
        Self {
            slot: EngineSlot::new(),
            warm: Mutex::new(warm),
            sounds: cache_dir
                .as_ref()
                .map(|dir| PcmStore::new(dir.join("audio"))),
            cache_dir,
        }
    }

    /// The warm-up to take the backend from (the engine's first start); afterwards there is none.
    pub(crate) fn take_warm_up(&self) -> Option<Warmup> {
        self.warm
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}
