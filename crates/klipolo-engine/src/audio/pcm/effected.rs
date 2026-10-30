//! Effected sources for the preview's loudness estimate: a source's 100 ms windows through its audio effect, kept as
//! `<hash>.energy` beside the caches, under the same budget (`PcmStore::effected`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::energy::EnergyWriter;
use super::{Energies, Pcm, PcmStore, build, tidy};
use crate::audio::loudness::write_effected;
use crate::audio::mix::to_frame;
use crate::audio::plan::AudioSource;
use crate::media::key::{CacheKind, hash_name, stem};

/// A source with an audio effect, as the preview's loudness estimate gets it (`PcmStore::effected`).
pub enum Effected {
    /// Its 100 ms K-weighted windows through its effect (stereo, unity gain, no fades) from its start, mapped.
    Ready(Energies),
    /// The estimate was cancelled while they were rendered: nothing is kept.
    Cancelled,
    /// They can't be kept (the file can't be read, the disk is full): the estimate renders the source this time.
    Unavailable,
}

impl PcmStore {
    /// Source `s` (with an audio effect, its file decoded in `pcm`) for the loudness estimate: its windows through
    /// the effect, before its volume and fades (which the estimate applies), kept as `<hash>.energy` in this folder.
    /// The name hashes what changes them: the file's cache (its path, size and time), where the source starts in the
    /// file, its speed and drift rate, the channel it plays centred from, the effect's filter chain and its length. Rendered through the effect only the first time (a
    /// 10-minute voiceover takes seconds); an edit elsewhere, or to its volume, fades or place, reuses them. 288 KB
    /// per hour of effected sound, checked like a cache's energies (`Energies::open`), under the folder's budget and
    /// evicted with the other caches by least recent use (`tidy`; each use touches it).
    pub fn effected(&self, s: &AudioSource, pcm: &Arc<Pcm>, cancel: &AtomicBool) -> Effected {
        let Some(name) = effected_name(s) else {
            return Effected::Unavailable;
        };
        let path = self.dir.join(format!("{name}.energy"));
        let frames = to_frame(s.end_us - s.start_us).max(0) as u64;
        if let Some(kept) = Energies::open(&path, 2, frames) {
            tidy::touch(&path);
            return Effected::Ready(kept);
        }
        self.effect_renders.fetch_add(1, Ordering::Relaxed);
        let n = build::TMP.fetch_add(1, Ordering::Relaxed);
        let tmp = self
            .dir
            .join(format!("{name}.{}-{n}.energy.tmp", std::process::id()));
        let made = (|| {
            std::fs::create_dir_all(&self.dir)?;
            let mut writer = EnergyWriter::create(&tmp, 2, frames)?;
            if !write_effected(s, pcm, &mut writer, cancel)? {
                return Ok(false);
            }
            writer.finish(frames)?;
            std::fs::rename(&tmp, &path)?;
            Ok::<_, std::io::Error>(true)
        })();
        if !matches!(made, Ok(true)) {
            let _ = std::fs::remove_file(&tmp);
        }
        match made {
            Ok(true) => {
                Energies::open(&path, 2, frames).map_or(Effected::Unavailable, Effected::Ready)
            }
            Ok(false) => Effected::Cancelled,
            Err(error) => {
                tracing::warn!(%error, "an effected source's loudness couldn't be kept");
                Effected::Unavailable
            }
        }
    }

    /// Effected sources rendered for the loudness estimate so far (tests, and how often the cache missed).
    pub fn effect_renders(&self) -> u64 {
        self.effect_renders.load(Ordering::Relaxed)
    }
}

/// The name of an effected source's kept windows (`PcmStore::effected`); None when its file can't be read.
fn effected_name(s: &AudioSource) -> Option<String> {
    let file = stem(&s.path, &CacheKind::Pcm)?;
    Some(hash_name(&[
        b"effected-energy-1",
        file.as_bytes(),
        &to_frame(s.source_in_us).to_le_bytes(),
        &s.speed.to_bits().to_le_bytes(),
        &s.rate.to_bits().to_le_bytes(),
        &[s.centre_from.map_or(0, |k| k.saturating_add(1))],
        s.filter.as_deref().unwrap_or_default().as_bytes(),
        &to_frame(s.end_us - s.start_us).to_le_bytes(),
    ]))
}
