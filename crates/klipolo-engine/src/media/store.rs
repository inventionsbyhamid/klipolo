//! The thumbnail store: which files still need sprites, and one utility-QoS thread that scans them
//! (`scan::scan_thumbs`) one at a time, the file under the playhead first. Sound comes first: the sound
//! cache's pass reads sound only, and scans wait while it decodes.
//!
//! Background work runs alone: a scan waits while the video plays, an export runs, an editing
//! copy is made, a file is transcribed or Sync listens (`Hold`), or the sound cache is decoding, and keeps its place. A
//! full disk stops thumbnails quietly until the next project. The folder is kept under `DISK_BUDGET` by least recent
//! use, never removing the open project's files, the running scan's or an unfinished export's.

mod next;
mod tidy;
mod worker;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use klipolo_model::time::Us;
use klipolo_platform::durable::Steps;

use super::activity::ActivityStore;
use super::events::{MediaEvent, MediaKind, MediaSink};
pub use tidy::tidy_at_start;

/// The thumbnails folder is kept under this many bytes (least recently used files go first).
pub const DISK_BUDGET: u64 = 1 << 30;
/// The scan thread ends after this long with nothing to do.
const IDLE: Duration = Duration::from_secs(30);
/// Scans of one file whose reading failed part-way, per opening, before it waits for the next opening (each edit
/// asks for the project's files again: a bad sector shouldn't be read again on every one). Sound-cache builds use the
/// same cap (`PcmStore`).
pub(crate) const READ_TRIES: u32 = 3;
/// Hold bit for "the sound cache is decoding" (sound first, one background decode at a time).
const SOUND: u8 = 8;
/// Hold bit for "thumbnails have work queued or a scan running": the activity pass waits for it (sound → thumbnails →
/// activity, `next`); scans never do.
const THUMBS: u8 = 128;

/// A file the project shows: its length, and the source time under the playhead (its scan starts there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub path: String,
    pub duration_us: Us,
    pub at_us: Us,
}

/// Why thumbnail scans wait: they run only while nothing holds them, and a held scan keeps its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// The engine is playing (the actor).
    Playback,
    /// An export is running (the app).
    Export,
    /// An editing copy is being made (the app): one background job at a time. (Bit 8 is the sound cache's
    /// `SOUND`. Transcription no longer holds scans: it runs beside them.)
    Copy,
    /// Sync is listening (the app): it decodes and reads the sound caches, so other background reads wait.
    Sync,
    /// A take is recording (the app, while its editor is hidden for the take): never competed with.
    Recording,
}

impl Hold {
    fn bit(self) -> u8 {
        match self {
            Self::Playback => 1,
            Self::Export => 2,
            Self::Copy => 4,
            // 8 is the sound cache's (`SOUND`); 32 is kept for Speech.
            Self::Sync => 16,
            Self::Recording => 64,
        }
    }
}

/// What a scan writes through, shared with the store.
pub(crate) struct Shared {
    pub(crate) root: PathBuf,
    pub(crate) steps: Arc<dyn Steps + Send + Sync>,
    sink: Mutex<Option<MediaSink>>,
    pub(crate) disk_full: AtomicBool,
}

impl Shared {
    pub(crate) fn emit(&self, path: &str, kind: MediaKind) {
        let sink = lock(&self.sink).clone();
        if let Some(sink) = sink {
            sink(MediaEvent::Changed {
                path: path.to_owned(),
                kind,
            });
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Queue {
    wants: VecDeque<Want>,
    worker: bool,
    /// The job being run (or waiting to run) stops when this is set.
    cancel: Arc<AtomicBool>,
    /// The file being scanned.
    current: Option<String>,
    /// The open project's video files.
    project: HashSet<String>,
    /// Files a scan couldn't finish (damaged, undecodable, or reading kept failing) in this opening: not tried again
    /// until the next project.
    gave_up: HashSet<String>,
    /// Scans per file whose reading failed part-way in this opening (`READ_TRIES`).
    unreadable: HashMap<String, u32>,
    /// Files whose sprites were marked as used in this opening (the LRU order).
    touched: HashSet<String>,
}

pub struct ThumbStore {
    shared: Arc<Shared>,
    queue: Mutex<Queue>,
    /// Signalled when wants arrive.
    changed: Condvar,
    holds: Mutex<u8>,
    /// Signalled when the last hold goes, or a job is cancelled.
    released: Condvar,
    running: AtomicBool,
    /// `<exports>/.jobs`: unfinished exports' files are never tidied away.
    jobs_root: Mutex<Option<PathBuf>>,
    /// Whether scans that need work wait or run, and who hears it (`on_waiting`).
    waiting: Mutex<(bool, Option<Waiting>)>,
    /// The activity pass, next in line (`next`).
    next: OnceLock<Arc<ActivityStore>>,
}

type Waiting = Box<dyn Fn(bool) + Send + Sync>;

impl ThumbStore {
    pub fn new(root: PathBuf, steps: Arc<dyn Steps + Send + Sync>) -> Arc<ThumbStore> {
        Arc::new(ThumbStore {
            shared: Arc::new(Shared {
                root,
                steps,
                sink: Mutex::default(),
                disk_full: AtomicBool::new(false),
            }),
            queue: Mutex::default(),
            changed: Condvar::new(),
            holds: Mutex::new(0),
            released: Condvar::new(),
            running: AtomicBool::new(false),
            jobs_root: Mutex::default(),
            waiting: Mutex::new((false, None)),
            next: OnceLock::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.shared.root
    }

    pub fn set_sink(&self, sink: MediaSink) {
        if let Some(next) = self.next.get() {
            next.set_sink(sink.clone());
        }
        *lock(&self.shared.sink) = Some(sink);
    }

    /// Unfinished export jobs live in `jobs_root` (`<exports>/.jobs`): the tidy keeps their files.
    pub fn set_jobs_root(&self, jobs_root: PathBuf) {
        *lock(&self.jobs_root) = Some(jobs_root);
    }

    /// Told now and whenever it changes whether scans that need work wait or run (a file already done never counts):
    /// transcription lets them go first. One listener; a new one replaces it.
    pub fn on_waiting(&self, tell: impl Fn(bool) + Send + Sync + 'static) {
        let mut w = lock(&self.waiting);
        tell(w.0);
        w.1 = Some(Box::new(tell));
    }

    /// Scans that need work wait or run (`on`), or none does: told once per change.
    pub(crate) fn set_waiting(&self, on: bool) {
        let mut w = lock(&self.waiting);
        if w.0 != on {
            w.0 = on;
            if let Some(tell) = &w.1 {
                tell(on);
            }
        }
    }

    /// The project's video files, in the order to scan them (`media::wants`): replaces the queue. Each is checked
    /// on disk (a few `stat`s for a finished file); one a scan couldn't finish in this opening is skipped.
    pub fn want(self: &Arc<Self>, wants: Vec<Want>) {
        let mut q = lock(&self.queue);
        q.project = wants.iter().map(|w| w.path.clone()).collect();
        q.wants = wants
            .into_iter()
            .filter(|w| !q.gave_up.contains(&w.path))
            .collect();
        if !q.wants.is_empty() {
            self.thumbs_busy(true);
        }
        if !q.wants.is_empty() && !q.worker {
            q.worker = true;
            let store = self.clone();
            let spawned = std::thread::Builder::new()
                .name("klipolo-thumbs".into())
                .spawn(move || store.work());
            if let Err(error) = spawned {
                q.worker = false;
                self.thumbs_busy(false);
                tracing::warn!(%error, "thumbnail thread");
            }
        }
        drop(q);
        self.changed.notify_all();
    }

    /// The project closed or switched: the queue empties, and a running (or held) scan stops, keeping every chunk
    /// it wrote; so does the activity pass.
    pub fn cancel_all(&self) {
        if let Some(next) = self.next.get() {
            next.cancel_all();
        }
        {
            let mut q = lock(&self.queue);
            q.wants.clear();
            q.gave_up.clear();
            q.unreadable.clear();
            q.touched.clear();
            q.project.clear();
            q.cancel.store(true, Ordering::SeqCst);
        }
        self.set_waiting(false);
        // Under the holds lock, so a held scan can't miss the wake-up.
        let _holds = lock(&self.holds);
        self.released.notify_all();
    }

    /// A write found the disk full: thumbnails stop until the next project.
    pub fn disk_full(&self) -> bool {
        self.shared.disk_full.load(Ordering::SeqCst)
    }

    pub fn reset_disk_full(&self) {
        self.shared.disk_full.store(false, Ordering::SeqCst);
    }

    /// Waiters look again too: an export or a copy arriving while playback holds a pass ends it at once.
    pub fn pause(&self, why: Hold) {
        *lock(&self.holds) |= why.bit();
        self.released.notify_all();
    }

    pub fn resume(&self, why: Hold) {
        self.release(why.bit());
    }

    /// Every waiter looks again: scans and the activity pass each wait for their own bits.
    fn release(&self, bit: u8) {
        *lock(&self.holds) &= !bit;
        self.released.notify_all();
    }

    /// The sound cache's build thread is decoding (`on`) or idle: scans wait for it.
    pub(crate) fn sound_busy(&self, on: bool) {
        if on {
            *lock(&self.holds) |= SOUND;
        } else {
            self.release(SOUND);
        }
    }

    /// Files are queued or one is being looked at (scanned, or waiting for a hold to go).
    pub fn pending(&self) -> bool {
        let q = lock(&self.queue);
        !q.wants.is_empty() || q.current.is_some()
    }

    /// What scans write through (unit tests of `scan_thumbs`).
    #[cfg(test)]
    pub(crate) fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    /// A scan is running now: started and not held. No editing copy starts while this is true.
    pub fn scanning(&self) -> bool {
        self.running.load(Ordering::SeqCst) && *lock(&self.holds) & !THUMBS == 0
    }

    /// Blocks the scan thread while anything holds scans (no polling: `resume` and `cancel_all` notify); false once
    /// the scan is cancelled.
    fn wait_unheld(&self, cancel: &AtomicBool) -> bool {
        let mut holds = lock(&self.holds);
        while *holds & !THUMBS != 0 && !cancel.load(Ordering::SeqCst) {
            holds = self
                .released
                .wait(holds)
                .unwrap_or_else(PoisonError::into_inner);
        }
        !cancel.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::{Hold, SOUND};

    /// Every hold has a bit of its own, apart from the sound cache's. Two branches can each add `=> 8`
    /// without a textual conflict; this fails then. A new hold doesn't compile here until it is listed (list it in
    /// both places).
    #[test]
    fn every_hold_has_its_own_bit() {
        let listed = |h: Hold| match h {
            Hold::Playback => 0,
            Hold::Export => 1,
            Hold::Copy => 2,
            Hold::Sync => 3,
            Hold::Recording => 4,
        };
        let all = [
            Hold::Playback,
            Hold::Export,
            Hold::Copy,
            Hold::Sync,
            Hold::Recording,
        ];
        assert!(all.iter().enumerate().all(|(i, h)| listed(*h) == i));
        let mut bits: Vec<u8> = all.iter().map(|h| h.bit()).chain([SOUND]).collect();
        assert!(bits.iter().all(|b| b.count_ones() == 1), "{bits:?}");
        bits.sort_unstable();
        bits.dedup();
        assert_eq!(bits.len(), all.len() + 1, "{bits:?}");
    }
}
