//! What the app holds for the preview: the one engine and its caches. The open project is the project session's
//! (`project::ProjectSessionState`), which publishes it to the engine's feed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_engine::audio::pcm::PcmStore;
use klipolo_engine::media::{Hold, ThumbStore, thumbs};
use klipolo_engine::render::warm::Warmup;
use klipolo_engine::session::EngineSlot;

/// The preview engine: started by the first attach and kept for the app's lifetime (started again if it ended).
pub struct EngineState {
    pub(crate) slot: EngineSlot,
    /// The GPU warm-up started at launch, until the engine's first start takes its backend.
    warm: Mutex<Option<Warmup>>,
    /// The app's cache folder (decoded sound, thumbnails); None when it couldn't be found.
    pub(crate) cache_dir: Option<PathBuf>,
    /// The decoded sound cache (`CacheDirs::audio`), one for the app: the preview plays from it and analysis reads its
    /// levels, so a file is decoded once. None without a cache folder.
    pub(crate) sounds: Option<Arc<PcmStore>>,
    /// The engine's background media work and whether a take holds it (`hold_for_recording`).
    media: Mutex<MediaHold>,
}

/// The running engine's thumbnail store (its scans and the activity pass) and whether a take holds it.
#[derive(Default)]
struct MediaHold {
    thumbs: Option<Arc<ThumbStore>>,
    recording: bool,
}

impl EngineState {
    pub fn new(warm: Option<Warmup>, cache_dir: Option<PathBuf>) -> Self {
        Self {
            slot: EngineSlot::new(),
            warm: Mutex::new(warm),
            sounds: cache_dir
                .as_deref()
                .map(|dir| PcmStore::new(CacheDirs::in_folder(dir).audio)),
            cache_dir,
            media: Mutex::default(),
        }
    }

    /// The engine that starts works with `thumbs`: held at once when a take holds background media work.
    pub(crate) fn set_thumbs(&self, thumbs: Option<Arc<ThumbStore>>) {
        let mut media = self.media.lock().unwrap_or_else(PoisonError::into_inner);
        if media.recording
            && let Some(thumbs) = &thumbs
        {
            thumbs.pause(Hold::Recording);
        }
        media.thumbs = thumbs;
    }

    /// A take is the recorder's (`held`), from arming until it has landed: the engine's background media work
    /// (thumbnail scans and the activity pass) waits meanwhile (`Hold::Recording`), keeping its place, so it never
    /// competes with the take.
    pub(crate) fn hold_for_recording(&self, held: bool) {
        let mut media = self.media.lock().unwrap_or_else(PoisonError::into_inner);
        media.recording = held;
        if let Some(thumbs) = &media.thumbs {
            if held {
                thumbs.pause(Hold::Recording);
            } else {
                thumbs.resume(Hold::Recording);
            }
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

/// The engine's caches in the app's cache folder, named once: decoded sound and its waveform peaks in `audio`,
/// thumbnails (`thumbs::root_in`) and the rest in `media`. `start_engine` hands these to the engine, and the timeline's
/// media commands read from the same folders.
pub(crate) struct CacheDirs {
    pub(crate) audio: PathBuf,
    pub(crate) media: PathBuf,
}

impl CacheDirs {
    pub(crate) fn in_folder(root: &Path) -> Self {
        Self {
            audio: root.join("audio"),
            media: root.join("media"),
        }
    }

    pub(crate) fn thumbs(&self) -> PathBuf {
        thumbs::root_in(&self.media)
    }
}

impl EngineState {
    /// The engine's caches, when the app's cache folder was found.
    pub(crate) fn caches(&self) -> Option<CacheDirs> {
        self.cache_dir.as_deref().map(CacheDirs::in_folder)
    }
}
