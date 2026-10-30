//! The PCM cache: each audio-bearing file decoded once, streaming, to 48 kHz s16 at its own channel count
//! (mono stays mono) in `<dir>/<16-hex key hash>.pcm`, memory-mapped so it is paged in on demand and never counts
//! toward the footprint. One cache per file: speed changes and effects run live in the mixer, so trying speeds
//! writes nothing. The same decode pass writes a peaks pyramid (`.peaks`, for the timeline's waveform) and the loudness
//! energies (`.energy`, 100 ms K-weighted windows: the preview's loudness and the file's own, `energy`). It reads sound only: a video's thumbnails come from a scan after it.
//! The preview's loudness estimate also keeps each effected source's windows here (`PcmStore::effected`, another
//! `<hash>.energy`), under the same budget.
//! Builds run one at a time on a utility-QoS thread, sources under the playhead first, and a file becomes playable
//! while it decodes (`Pcm` grows behind a watermark).
//!
//! A close deletes nothing: another project, or a paused export's frozen project, may use the same
//! file. The app's store keeps the folder under `DISK_BUDGET` by least recent use after each build (and at start),
//! never removing what the open project uses (every file this opening built, opened or queued) or an unfinished
//! export job names. Only the app's store evicts (`evict`): the export worker's can't know the open project.
//!
//! File layout: a 32-byte header (`HEADER`), then interleaved little-endian s16 at half scale (6 dB of headroom, so
//! a lossy decoder's overs never clip), padded with up to a second of silence past `frames`.

mod build;
mod effected;
pub(crate) mod energy;
mod handle;
#[cfg(test)]
mod hooks;
pub mod peaks;
mod queue;
mod quiet;
#[cfg(test)]
mod requeue_tests;
mod sink;
#[cfg(test)]
mod sync_tests;
#[cfg(test)]
mod tests;
mod tidy;
mod worker;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use super::plan::AudioSource;
use crate::media::events::{MediaEvent, MediaSink};
use crate::media::key::{CacheKind, stem};
use crate::media::store::ThumbStore;
use build::Progress;
pub use effected::Effected;
pub use energy::{Energies, WINDOW};
pub use handle::{CHUNK, Pcm};
pub use quiet::quiet_spans;

/// Header bytes before the samples.
pub const HEADER: usize = 32;
const MAGIC: &[u8; 8] = b"KLPOPCM1";
/// s16 → f32 (samples are stored at half scale).
const READ_SCALE: f32 = 2.0 / 32767.0;
/// The cache folder is kept under this many bytes (least recently used files go first). About six hours of stereo
/// (an hour is 0.69 GB at s16), so a long recording is never evicted by the next build; the open project's files are
/// never evicted at all.
pub const DISK_BUDGET: u64 = 4 << 30;
/// A file that failed to decode is tried again after this long (it may have been reconnected or replaced). One whose
/// reading failed part-way is tried `READ_TRIES` times per opening; then what decoded stays playable (silence past
/// the bad spot), the user hears about it once (`MediaEvent::AudioDamaged`), and it waits for the next opening.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// Why a cache build didn't finish.
#[derive(Debug)]
pub enum BuildError {
    /// A closed project cancelled it: not a failure, it may be built again.
    Cancelled,
    /// The cache couldn't be written (`klipolo_platform::durable::classify` tells a full disk).
    Cache(std::io::Error),
    /// The file's audio couldn't be read or decoded.
    Source(String),
    /// Reading the file failed part-way, at frame `at` of the run that hit it: what decoded before it is written.
    Unreadable { message: String, at: u64 },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Cache(e) => write!(f, "Audio cache: {e}"),
            Self::Source(e) | Self::Unreadable { message: e, .. } => f.write_str(e),
        }
    }
}

impl From<std::io::Error> for BuildError {
    fn from(e: std::io::Error) -> Self {
        Self::Cache(e)
    }
}

/// The cache folder's housekeeping at app start: unfinished files no build is writing any more (another process's
/// live build is kept), older cache formats, and the disk budget (unfinished exports' files kept, `jobs_root`). The
/// app runs it once, on its own thread.
pub fn tidy_at_start(dir: &Path, jobs_root: Option<&Path>) {
    tidy::tidy(dir, true, DISK_BUDGET, || {
        protected_stems(&HashSet::new(), jobs_root)
    });
}

/// Cache stems the tidy keeps: `paths`' and those of unfinished export jobs.
fn protected_stems(paths: &HashSet<String>, jobs_root: Option<&Path>) -> HashSet<String> {
    let jobs = jobs_root
        .map(crate::media::export_job_sources)
        .unwrap_or_default();
    paths
        .iter()
        .chain(&jobs)
        .filter_map(|p| stem(p, &CacheKind::Pcm))
        .collect()
}

/// One cache file per source file (its size and modification time are part of the file name's hash).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PcmKey {
    pub path: String,
}

impl PcmKey {
    pub fn of(s: &AudioSource) -> PcmKey {
        PcmKey {
            path: s.path.clone(),
        }
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone, Debug)]
struct Job {
    key: PcmKey,
    /// Decode from this frame first (the playhead), then the rest.
    start_at: u64,
}

#[derive(Default)]
struct Inner {
    ready: HashMap<PcmKey, Arc<Pcm>>,
    queue: Vec<Job>,
    building: HashMap<PcmKey, Arc<Progress>>,
    failed: HashMap<PcmKey, Instant>,
    /// Builds per file whose reading failed part-way in this opening (`READ_TRIES`).
    unreadable: HashMap<PcmKey, u32>,
    /// Files whose reading kept failing: what decoded stays playable, and they aren't built again until the project
    /// is opened again.
    damaged: HashSet<PcmKey>,
    worker: bool,
    /// The build thread starts no new build (`pause_queue`): Sync is listening.
    paused: bool,
    /// Cache files (by hash stem) opened or written since the open project opened, and the source each came from.
    /// The tidy never removes these; a close forgets them.
    used: HashMap<String, String>,
}

pub struct PcmStore {
    dir: PathBuf,
    inner: Mutex<Inner>,
    /// Signalled when a job is queued or a build finishes.
    changed: Condvar,
    /// Bumped whenever a file's `Pcm` appears, is replaced or is forgotten, so the mixer looks again.
    generation: AtomicU64,
    /// Thumbnail scans, held while this store decodes (`ThumbStore::sound_busy`): the running engine's (a store the app
    /// shares outlives an engine that is replaced).
    thumbs: Mutex<Option<Arc<ThumbStore>>>,
    /// Told when a file's peaks are ready.
    sink: Mutex<Option<MediaSink>>,
    /// Set on the app's store, which alone evicts (see the module doc): `<exports>/.jobs` when the exports folder is
    /// known.
    evicts: OnceLock<Option<PathBuf>>,
    /// Effected sources rendered for the loudness estimate (`effected`).
    effect_renders: AtomicU64,
}

impl PcmStore {
    pub fn new(dir: PathBuf) -> Arc<PcmStore> {
        Arc::new(PcmStore {
            dir,
            inner: Mutex::default(),
            changed: Condvar::new(),
            generation: AtomicU64::new(0),
            thumbs: Mutex::default(),
            sink: Mutex::default(),
            evicts: OnceLock::new(),
            effect_renders: AtomicU64::new(0),
        })
    }

    /// Thumbnail scans wait while this store decodes: one background decode at a time (the latest engine's).
    pub fn set_thumbs(&self, thumbs: Arc<ThumbStore>) {
        *lock(&self.thumbs) = Some(thumbs);
    }

    pub fn set_sink(&self, sink: MediaSink) {
        *lock(&self.sink) = Some(sink);
    }

    /// This store keeps the folder under its budget after each build (set once, on the app's store). Unfinished
    /// export jobs live in `jobs_root` (`<exports>/.jobs`), when it is known: their files are kept.
    pub fn evict(&self, jobs_root: Option<PathBuf>) {
        let _ = self.evicts.set(jobs_root);
    }

    /// `source`'s finished waveform peaks (`<stem>.peaks`), if its sound has been decoded.
    pub fn peaks_path(&self, source: &str) -> Option<PathBuf> {
        peaks_file(&self.dir, source)
    }

    /// The file's loudness energies (`<stem>.energy`, mapped), once its sound (`pcm`, complete) is decoded; None when
    /// they aren't there or don't match it (the preview then measures the mix exactly, and the next opening of the
    /// cache makes them again).
    pub fn energies(&self, key: &PcmKey, pcm: &Pcm) -> Option<Energies> {
        if !pcm.is_complete() {
            return None;
        }
        let stem = stem(&key.path, &CacheKind::Pcm)?;
        Energies::open(
            &self.dir.join(format!("{stem}.energy")),
            pcm.channels(),
            pcm.frames() as u64,
        )
    }

    fn emit(&self, event: MediaEvent) {
        let sink = lock(&self.sink).clone();
        if let Some(sink) = sink {
            sink(event);
        }
    }

    /// Whether `key`'s finished cache is in the folder, opened or not: a build renames it into place only when it is
    /// complete (Sync counts it as costing no disk).
    pub fn on_disk(&self, key: &PcmKey) -> bool {
        stem(&key.path, &CacheKind::Pcm)
            .is_some_and(|stem| self.dir.join(format!("{stem}.pcm")).is_file())
    }

    /// The file's audio (possibly still growing) once its build has started; never blocks on decoding.
    pub fn ready(&self, key: &PcmKey) -> Option<Arc<Pcm>> {
        lock(&self.inner).ready.get(key).cloned()
    }

    /// How far a running build of `key` has decoded (the frame it writes next), or None when none runs: a caller that
    /// waits on `build` tells a slow decode from a stuck one (transcription's stuck check).
    pub fn building_at(&self, key: &PcmKey) -> Option<u64> {
        lock(&self.inner)
            .building
            .get(key)
            .map(|p| p.at.load(Ordering::Relaxed))
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn insert_ready(&self, key: PcmKey, pcm: Arc<Pcm>) {
        self.publish(&mut lock(&self.inner), key, pcm);
    }

    fn publish(&self, inner: &mut Inner, key: PcmKey, pcm: Arc<Pcm>) {
        if let Some(old) = inner.ready.insert(key, pcm.clone())
            && !Arc::ptr_eq(&old, &pcm)
        {
            // Voices still reading the old copy let it go.
            old.superseded.store(true, Ordering::Release);
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// The project closed: cancel its builds and forget every mapping and queued build. Nothing is deleted
    /// tidy evicts, by least recent use.
    pub fn close_project(&self) {
        let mut inner = lock(&self.inner);
        // A cancelled build winds down on its own: the next project may queue the same file at once.
        for (_, progress) in inner.building.drain() {
            progress.cancel.store(true, Ordering::Release);
        }
        for pcm in inner.ready.values() {
            pcm.superseded.store(true, Ordering::Release);
        }
        inner.ready.clear();
        inner.queue.clear();
        inner.used.clear();
        inner.unreadable.clear();
        inner.damaged.clear();
        // A pause belongs to the opening it was taken in: a leaked one never holds the next.
        inner.paused = false;
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

/// `<dir>/<stem>.peaks` for `source`, when it exists.
pub fn peaks_file(dir: &Path, source: &str) -> Option<PathBuf> {
    let file = dir.join(format!("{}.peaks", stem(source, &CacheKind::Pcm)?));
    file.is_file().then_some(file)
}
