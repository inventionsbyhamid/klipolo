//! The local work of a clip's analysis, behind `Media` so tests can stand in for it: its sound as the speech models take
//! it, its sound levels, and the scene log's small copy. `EngineMedia` is the app's, on the engine's decoders and its
//! sound cache. Every call is blocking; the pipeline runs them one clip at a time on a thread of utility priority
//! (`AnalysisState`), making way for playback, an export or a take (`holds`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use klipolo_analysis::VideoError;
use klipolo_analysis::audio::{self, AudioError, AudioInfo};
use klipolo_analysis::proxy::{self, ProxyInfo};
use klipolo_engine::audio::pcm::{Energies, Pcm, PcmKey, PcmStore, WINDOW};
use klipolo_model::speech::Levels;
use klipolo_model::time::Us;

/// A clip's local work (see the module notes). Blocking calls.
pub trait Media: Send + Sync {
    /// `src`'s sound as a mono 16-bit WAV at 16 kHz, written to `out` (`audio::extract_wav_16k`); `cancel` stops it
    /// (`AudioError::Stopped`). `between` runs between decoded blocks and may block: the decode makes way for playback
    /// there, and goes on where it was.
    fn sound(
        &self,
        src: &Path,
        out: &Path,
        cancel: &AtomicBool,
        between: &mut dyn FnMut(),
    ) -> Result<AudioInfo, AudioError>;

    /// `src`'s loudness, dB per 100 ms of its sound; None when it has none or it can't be read now. Never paused midway: it
    /// decodes into the preview's own sound cache, which playback may be waiting on.
    fn levels(&self, src: &Path, cancel: &AtomicBool) -> Option<Levels>;

    /// `src`'s small copy for the scene log, in chunks under `dir` (`proxy::make_reporting`); `seen` hears the file time
    /// of each source frame decoded, and may block: the decode makes way for playback there, and goes on where it was.
    fn small_copy(
        &self,
        src: &Path,
        dir: &Path,
        cancel: &AtomicBool,
        seen: &mut dyn FnMut(Us),
    ) -> Result<ProxyInfo, VideoError>;
}

/// The app's local work: the engine's decoders, and its sound cache for the levels (shared with the preview, so a
/// file is decoded once).
pub struct EngineMedia {
    sounds: Option<Arc<PcmStore>>,
}

impl EngineMedia {
    /// Levels come from `sounds` (the app's sound cache); without one, clips have none.
    pub fn new(sounds: Option<Arc<PcmStore>>) -> Self {
        Self { sounds }
    }
}

impl Media for EngineMedia {
    fn sound(
        &self,
        src: &Path,
        out: &Path,
        cancel: &AtomicBool,
        between: &mut dyn FnMut(),
    ) -> Result<AudioInfo, AudioError> {
        audio::extract_wav_16k_paced(src, out, cancel, between)
    }

    /// From the sound cache's energies (decoding the file into it first if no build has): no second decode of our
    /// own.
    fn levels(&self, src: &Path, cancel: &AtomicBool) -> Option<Levels> {
        let store = self.sounds.as_ref()?;
        let key = PcmKey {
            path: src.to_string_lossy().into_owned(),
        };
        let pcm = store
            .build_cancellable(&key, cancel)
            .inspect_err(|error| tracing::info!(%error, "analysis: no sound levels"))
            .ok()?;
        let energies = store.energies(&key, &pcm)?;
        Some(levels_of(&pcm, &energies))
    }

    fn small_copy(
        &self,
        src: &Path,
        dir: &Path,
        cancel: &AtomicBool,
        seen: &mut dyn FnMut(Us),
    ) -> Result<ProxyInfo, VideoError> {
        proxy::make_reporting(src, dir, proxy::FIT.1, cancel, seen)
    }
}

/// A file's levels from its cache's energies: one value per 100 ms window of its sound, `10 log10(power / 2)` dB (the
/// mean over both speakers: `Energies::power` counts a mono channel twice and adds stereo's two), at least -120 dB.
/// Window `w` is `[w, w + 1) * 100 ms` of the file's sound, as `Speech::levels` and `speech::silence` read them.
pub fn levels_of(pcm: &Pcm, energies: &Energies) -> Levels {
    let windows = (pcm.frames() as u64).div_ceil(WINDOW as u64) as usize;
    let db: Vec<f32> = (0..windows)
        .map(|w| (10.0 * (energies.power(w) / 2.0).max(1e-12).log10()) as f32)
        .collect();
    Levels { db: db.into() }
}
