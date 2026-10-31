//! The durable writer: one thread per take puts every run file's bytes and the journal's lines on
//! the disk, and makes them durable together once a second.
//!
//! - **Written at once.** Each fragment, sound block and journal line is handed to `write()` as soon as the thread
//!   takes it, so it survives a `kill -9` of the recorder: the OS has it.
//! - **One flush tick a second** (`tick`). Every file that got data since the last tick is pushed to the drive
//!   (`Io::push`, by key), a barrier orders those pushes ahead of what follows (`Io::order`: macOS's `F_BARRIERFSYNC`;
//!   nothing elsewhere, where a push already reaches the disk), one `Durable` line records each open file's length,
//!   and the journal is pushed and fully flushed once (`Io::barrier`, F_FULLFSYNC). The files and the line become
//!   durable together, and the line never reaches the disk ahead of the media it vouches for. A tick with nothing new
//!   does nothing (a pause costs no flushes), except `finish`'s last one, which always records the final lengths.
//! - **A bounded queue that never blocks** (`append`, `QUEUE_MESSAGES`). It is measured in seconds of media: the core
//!   warns at `WARN_QUEUED_MS` and stops and saves at `STOP_QUEUED_MS`, and a key never holds more than
//!   `HOLD_QUEUED_MS`, so memory stays bounded (25 s of a 30 Mb/s stream is 94 MB). Opens, closes, journal lines and
//!   ticks have room of their own past the appends' share, so a full queue of media never costs a journal entry.
//! - **Never an append after a lost one.** A run's appends depend on each other (P-frames on earlier frames, PCM on its
//!   place in time), so a media key whose append the queue couldn't take (`WriterError::Full`) takes nothing more: the
//!   core ends that run and opens the next at a keyframe. Its file no longer matches the muxer's
//!   own index: `WriterSnapshot::wrote_exactly` tells Stop to walk it rather than use `Fmp4Writer::remux_plan`.
//! - **Failure first.** An append that fails with an I/O error is retried after 50, 200 and 800 ms, each time from the
//!   length it started at (what the failed try left is cut off first). A full, read-only or forbidden disk fails at
//!   once, and so does a failed push or flush (after a failed `fsync` the OS may have dropped the data, so a retry
//!   could falsely succeed). The first failure is kept (`WriterSnapshot::failed`) and every later call is `Gone`: the
//!   core stops and saves. After a failed append the files still get one last push and `Durable` line, so everything
//!   whole that reached the file is durable; bytes past the last whole append are never counted.
//!
//! Costs: an append is one lock of the key table and one `try_send`, with no allocation (the bytes move). The thread
//! keeps a few small tables per open file and a 300-entry ring of tick times; a tick allocates its `Durable` line once.
//! The channel's slots are allocated up front: about 50 KB.

mod atomic;
mod thread;
mod tick;

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use klipolo_model::time::Us;

use crate::io::Io;
use crate::journal::{self, Entry};
use crate::session::{FileKey, SessionDir};
pub use thread::WriterProgress;
use thread::{Msg, OpenMsg, Shared, Thread};

/// Appends the queue holds before `append` gives `WriterError::Full`.
pub const QUEUE_MESSAGES: usize = 1_000;
/// Media queued that the core warns at (`capture.health.slowDrive`).
pub const WARN_QUEUED_MS: u32 = 5_000;
/// Media queued that the core stops and saves at (`capture.stop.slowDrive`).
pub const STOP_QUEUED_MS: u32 = 20_000;
/// The most media one key may have queued: an append past it is refused (`WriterError::Full`).
pub const HOLD_QUEUED_MS: u32 = 25_000;
/// Room for opens, closes, journal lines and ticks past the appends' share of the channel: they are refused (`Full`)
/// only once that is used up too, when the drive is far behind.
const CONTROL_ROOM: usize = 64;

/// Why the writer didn't take a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterError {
    /// The writer has failed (`WriterSnapshot::failed`) or stopped, or the key isn't open: stop and save.
    Gone,
    /// The queue had no room. For a run file this append is lost and the key takes nothing more: end the run.
    Full,
}

/// What stopped the writer, as far as the person can act on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskFailure {
    Full,
    ReadOnly,
    Denied,
    Io,
}

/// The writer's state, for the core's tick, the heartbeat and Stop.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WriterSnapshot {
    /// The most media any key has queued and not yet written.
    pub queued_ms: u32,
    pub last_tick_ms: u32,
    /// Over the last 300 ticks that did something.
    pub tick_p99_ms: u32,
    /// Every byte written so far, media and journal.
    pub bytes: u64,
    pub failed: Option<DiskFailure>,
    /// Each file's length as the last tick made it durable (every file this writer opened).
    pub durable: BTreeMap<FileKey, u64>,
}

impl WriterSnapshot {
    /// The writer never failed and `key` holds exactly `len` bytes, all durable: after `finish`, Stop can plan a run's
    /// remux from its `Fmp4Writer`'s own index when `len` is its `bytes_written()`. Otherwise the file holds less than
    /// the muxer emitted (an append was lost or failed), and Stop walks the file (recovery's per-file steps).
    pub fn wrote_exactly(&self, key: &str, len: u64) -> bool {
        self.failed.is_none() && self.durable.get(key) == Some(&len)
    }
}

/// A key as the callers see it.
struct Key {
    slot: usize,
    queued_us: Arc<AtomicU64>,
    /// A line file: its lines stand alone, so one lost doesn't refuse the next.
    lines: bool,
    lost: bool,
    closed: bool,
}

/// One take's writer thread. Every call returns at once; `finish` waits for the queue to drain.
pub struct DurableWriter {
    tx: Option<SyncSender<Msg>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    dir: SessionDir,
    keys: Mutex<(HashMap<FileKey, Key>, usize)>,
}

impl DurableWriter {
    /// Opens the take's journal to append to (`Io::open_append`: a resumed recorder goes on after the crashed one's
    /// last line, torn or not) and starts the thread at user-initiated QoS.
    pub fn spawn(io: Arc<dyn Io>, dir: &SessionDir) -> io::Result<Self> {
        let journal_path = dir.journal();
        let journal = io.open_append(&journal_path)?;
        let written = journal.metadata()?.len();
        let shared = Arc::new(Shared::default());
        let (tx, rx) = sync_channel(QUEUE_MESSAGES + CONTROL_ROOM);
        let thread = Thread::new(io, shared.clone(), journal, journal_path, written);
        let thread = std::thread::Builder::new()
            .name("klipolo-writer".into())
            .spawn(move || thread.run(rx))?;
        Ok(Self {
            tx: Some(tx),
            thread: Some(thread),
            shared,
            dir: dir.clone(),
            keys: Mutex::new((HashMap::new(), 0)),
        })
    }

    /// Creates run file `key` ("screen/run-01.mp4"), new only, and writes `head` (the fMP4 head or the CAF header).
    pub fn open(&self, key: &str, head: Vec<u8>) -> Result<(), WriterError> {
        self.open_key(key, head, false)
    }

    /// Opens line file `key` ("timing.jsonl", "cursor.jsonl") to append to, made if missing, through
    /// `journal::open_append`: a resumed recorder goes on after the crashed one's lines.
    pub fn open_lines(&self, key: &str) -> Result<(), WriterError> {
        self.open_key(key, Vec::new(), true)
    }

    fn open_key(&self, key: &str, head: Vec<u8>, lines: bool) -> Result<(), WriterError> {
        self.check()?;
        let path = self.dir.key_path(key).map_err(|e| {
            tracing::error!(key, error = %e, "writer: refused a key");
            WriterError::Gone
        })?;
        let mut keys = self.keys();
        let (table, next) = &mut *keys;
        // Closed keys stay until their last append is written, so `queued_ms` counts it.
        table.retain(|_, k| !k.closed || k.queued_us.load(Ordering::Acquire) > 0);
        if table.contains_key(key) {
            tracing::error!(key, "writer: a key is opened twice");
            return Err(WriterError::Gone);
        }
        let queued_us = Arc::new(AtomicU64::new(0));
        let msg = OpenMsg {
            slot: *next,
            key: key.to_owned(),
            path,
            head,
            lines,
            queued_us: queued_us.clone(),
        };
        self.send(Msg::Open(Box::new(msg)))?;
        table.insert(
            key.to_owned(),
            Key {
                slot: *next,
                queued_us,
                lines,
                lost: false,
                closed: false,
            },
        );
        *next += 1;
        Ok(())
    }

    /// Queues `bytes` for `key`, holding `media_us` of media (a fragment's `Fragment::media_us`, a sound block's
    /// frames ÷ rate; 0 for lines). Never blocks. `Full`: the queue had no room, and a run file's key takes nothing
    /// more.
    pub fn append(&self, key: &str, bytes: Vec<u8>, media_us: Us) -> Result<(), WriterError> {
        self.check()?;
        let mut keys = self.keys();
        let k = match keys.0.get_mut(key) {
            Some(k) if !k.closed => k,
            _ => return Err(WriterError::Gone),
        };
        if k.lost {
            return Err(WriterError::Full);
        }
        let us = u64::try_from(media_us).unwrap_or(0);
        let held = k.queued_us.load(Ordering::Acquire);
        let room = self.shared.appends.fetch_add(1, Ordering::AcqRel) < QUEUE_MESSAGES
            && held + us <= u64::from(HOLD_QUEUED_MS) * 1_000;
        let sent = if room {
            k.queued_us.fetch_add(us, Ordering::AcqRel);
            let msg = Msg::Append {
                slot: k.slot,
                bytes,
                media_us: us,
            };
            self.send(msg)
        } else {
            Err(WriterError::Full)
        };
        if let Err(e) = sent {
            self.shared.appends.fetch_sub(1, Ordering::AcqRel);
            if room {
                k.queued_us.fetch_sub(us, Ordering::AcqRel);
            }
            if e == WriterError::Full && !k.lines {
                tracing::warn!(
                    key,
                    "writer: the queue is full, so this run takes nothing more"
                );
                k.lost = true;
            }
        }
        sent
    }

    /// Queues one journal line. Never blocks. `StopRequested` jumps the queue: the thread writes it before anything
    /// still queued and syncs it at once, and a full queue never refuses it. It is the worker's guard against recording
    /// again after the person's Stop (`run::resume`), so a slow drive's backlog never delays it.
    pub fn journal(&self, entry: &Entry) -> Result<(), WriterError> {
        self.check()?;
        let line = journal::line(entry);
        if !matches!(entry, Entry::StopRequested { .. }) {
            return self.send(Msg::Journal(line));
        }
        self.shared.urgent_pending.fetch_add(1, Ordering::AcqRel);
        (self
            .shared
            .urgent
            .lock()
            .unwrap_or_else(PoisonError::into_inner))
        .push(line);
        // A full queue means the thread is busy: it takes the line before its next message anyway.
        let _ = self.send(Msg::Wake);
        Ok(())
    }

    /// Queues a flush tick (once a second, from the core). Never blocks.
    pub fn tick(&self) -> Result<(), WriterError> {
        self.check()?;
        self.send(Msg::Tick)
    }

    /// No more appends for `key`; the next tick pushes what it got last, records its final length and closes it.
    pub fn close(&self, key: &str) -> Result<(), WriterError> {
        self.check()?;
        let mut keys = self.keys();
        let slot = match keys.0.get_mut(key) {
            Some(k) if !k.closed => {
                k.closed = true;
                k.slot
            }
            _ => return Err(WriterError::Gone),
        };
        self.send(Msg::Close { slot })
    }

    pub fn snapshot(&self) -> WriterSnapshot {
        let queued_us = self
            .keys()
            .0
            .values()
            .map(|k| k.queued_us.load(Ordering::Acquire))
            .max()
            .unwrap_or(0);
        let state = self.shared.state();
        WriterSnapshot {
            queued_ms: u32::try_from(queued_us / 1_000).unwrap_or(u32::MAX),
            last_tick_ms: state.last_tick_ms,
            tick_p99_ms: state.tick_p99_ms,
            bytes: self.shared.bytes.load(Ordering::Acquire),
            failed: state.failed,
            durable: state.durable.clone(),
        }
    }

    /// The running byte count, still readable once `finish` has taken the writer: Stop's watchdog tells a slow drive
    /// (still writing) from one that stopped answering.
    pub fn progress(&self) -> WriterProgress {
        WriterProgress(self.shared.clone())
    }

    /// Lets the queue drain, makes a last tick (always, so the journal ends with every file's final length), and waits
    /// for the thread. A failure is in the snapshot; the error is only for a thread that died.
    pub fn finish(mut self) -> io::Result<WriterSnapshot> {
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| io::Error::other("the durable writer's thread panicked"))?;
        }
        Ok(self.snapshot())
    }

    fn check(&self) -> Result<(), WriterError> {
        if self.shared.failed.load(Ordering::Acquire) {
            return Err(WriterError::Gone);
        }
        Ok(())
    }

    fn send(&self, msg: Msg) -> Result<(), WriterError> {
        let tx = self.tx.as_ref().ok_or(WriterError::Gone)?;
        tx.try_send(msg).map_err(|e| match e {
            TrySendError::Full(_) => WriterError::Full,
            TrySendError::Disconnected(_) => WriterError::Gone,
        })
    }

    fn keys(&self) -> MutexGuard<'_, (HashMap<FileKey, Key>, usize)> {
        self.keys.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for DurableWriter {
    /// Without `finish`: the thread still drains the queue and makes its last tick, but nothing waits for it, so a
    /// process that exits now cuts that short (as a crash would; the journal's durable lengths stay exact).
    fn drop(&mut self) {
        self.tx = None;
    }
}
