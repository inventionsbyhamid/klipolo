//! The writer's thread: it takes the messages in order, writes each at once, and ticks (the module doc in `writer`).

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_platform::durable::{DiskReason, classify};
use klipolo_platform::paths::tilde;
use klipolo_platform::qos::{self, Qos};

use super::DiskFailure;
use super::tick::{Ticks, write_retrying};
use crate::io::Io;
use crate::session::FileKey;

/// What the callers and the thread share. The thread updates `state` after each step that changes it.
#[derive(Default)]
pub(crate) struct Shared {
    pub(super) failed: AtomicBool,
    pub(super) appends: AtomicUsize,
    pub(super) bytes: AtomicU64,
    state: Mutex<State>,
    /// Journal lines that jump the queue (`DurableWriter::journal`'s Stop line), and how many aren't on the disk yet.
    pub(super) urgent: Mutex<Vec<Vec<u8>>>,
    pub(super) urgent_pending: AtomicUsize,
    /// Stop gave up on this writer (`WriterProgress::abandon`): a failed write is not retried, so a retry never cuts
    /// what another handle appended to the journal after it.
    pub(super) abandoned: AtomicBool,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) last_tick_ms: u32,
    pub(super) tick_p99_ms: u32,
    pub(super) failed: Option<DiskFailure>,
    pub(super) durable: BTreeMap<FileKey, u64>,
}

/// Every byte the writer has written so far (`DurableWriter::progress`).
#[derive(Clone)]
pub struct WriterProgress(pub(super) Arc<Shared>);

impl WriterProgress {
    pub fn bytes(&self) -> u64 {
        self.0.bytes.load(Ordering::Acquire)
    }

    /// Lines that jump the queue (the Stop line) not yet written and synced: Stop's exit on a stuck drive writes them
    /// itself, best effort.
    pub fn urgent_pending(&self) -> usize {
        self.0.urgent_pending.load(Ordering::Acquire)
    }

    /// Stop gave up on the writer and writes to the journal itself: from now on a failed write is not retried, since a
    /// retry first cuts the file back to the writer's own length, past what was appended meanwhile.
    pub fn abandon(&self) {
        self.0.abandoned.store(true, Ordering::Release);
    }
}

impl Shared {
    pub(super) fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub(crate) enum Msg {
    Open(Box<OpenMsg>),
    Append {
        slot: usize,
        bytes: Vec<u8>,
        media_us: u64,
    },
    Journal(Vec<u8>),
    Tick,
    Close {
        slot: usize,
    },
    /// An urgent line is waiting (`Shared::urgent`); it is taken before any message anyway.
    Wake,
    /// A small file written whole (`DurableWriter::write_atomic`).
    WriteAtomic {
        path: PathBuf,
        bytes: Vec<u8>,
    },
}

pub(crate) struct OpenMsg {
    pub slot: usize,
    pub key: FileKey,
    pub path: PathBuf,
    pub head: Vec<u8>,
    pub lines: bool,
    pub queued_us: Arc<AtomicU64>,
}

/// An open file.
pub(super) struct Run {
    pub(super) key: FileKey,
    pub(super) path: PathBuf,
    pub(super) file: File,
    /// Bytes known written whole: never the part a failed append left.
    pub(super) written: u64,
    pub(super) dirty: bool,
    pub(super) closed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Open,
    Write,
    Push,
    Order,
    Barrier,
}

pub(super) struct Failed {
    pub(super) step: Step,
    pub(super) path: PathBuf,
    pub(super) error: io::Error,
}

pub(crate) struct Thread {
    pub(super) io: Arc<dyn Io>,
    pub(super) shared: Arc<Shared>,
    pub(super) journal: File,
    pub(super) journal_path: PathBuf,
    journal_len: u64,
    /// Journal lines written since the last barrier.
    pub(super) journal_dirty: bool,
    /// By slot (a key's number in open order); None once closed.
    pub(super) files: Vec<Option<Run>>,
    /// Each slot's queued media, which the callers read.
    queued: Vec<Arc<AtomicU64>>,
    /// Slots written since the last tick.
    pub(super) dirty: Vec<usize>,
    pub(super) ticks: Ticks,
    pub(super) failed: bool,
}

impl Thread {
    pub(crate) fn new(
        io: Arc<dyn Io>,
        shared: Arc<Shared>,
        journal: File,
        journal_path: PathBuf,
        journal_len: u64,
    ) -> Self {
        Self {
            io,
            shared,
            journal,
            journal_path,
            journal_len,
            journal_dirty: false,
            files: Vec::new(),
            queued: Vec::new(),
            dirty: Vec::new(),
            ticks: Ticks::default(),
            failed: false,
        }
    }

    /// Until every sender is gone; then the last tick.
    pub(crate) fn run(mut self, rx: Receiver<Msg>) {
        qos::set_current(Qos::UserInitiated);
        for msg in rx {
            if let Msg::Append { .. } = msg {
                self.shared.appends.fetch_sub(1, Ordering::AcqRel);
            }
            self.write_urgent();
            if self.failed {
                // Nothing more is written; the queue's media count still drains.
                self.forget(msg);
            } else if let Err(f) = self.handle(msg) {
                self.fail(f);
            }
        }
        self.write_urgent();
        if !self.failed
            && let Err(f) = self.tick(true)
        {
            self.fail(f);
        }
    }

    fn handle(&mut self, msg: Msg) -> Result<(), Failed> {
        match msg {
            Msg::Open(o) => self.open(*o),
            Msg::Append {
                slot,
                bytes,
                media_us,
            } => {
                let done = self.append(slot, &bytes);
                self.queued[slot].fetch_sub(media_us, Ordering::AcqRel);
                done
            }
            Msg::Journal(line) => self.write_journal(&line),
            Msg::WriteAtomic { path, bytes } => self.write_whole(&path, &bytes),
            Msg::Tick => self.tick(false),
            Msg::Close { slot } => {
                if let Some(run) = self.files.get_mut(slot).and_then(Option::as_mut) {
                    run.closed = true;
                }
                Ok(())
            }
            Msg::Wake => Ok(()),
        }
    }

    fn forget(&mut self, msg: Msg) {
        match msg {
            Msg::Open(o) => self.queued.push(o.queued_us),
            Msg::Append { slot, media_us, .. } => {
                self.queued[slot].fetch_sub(media_us, Ordering::AcqRel);
            }
            _ => {}
        }
    }

    fn open(&mut self, o: OpenMsg) -> Result<(), Failed> {
        debug_assert_eq!(
            o.slot,
            self.queued.len(),
            "slots are numbered in open order"
        );
        self.queued.push(o.queued_us);
        let opened = if o.lines {
            self.io.open_append(&o.path)
        } else {
            self.io.create(&o.path)
        };
        let mut file = opened.map_err(Failed::at(Step::Open, &o.path))?;
        // A line file a resumed recorder goes on with: what the crashed one left is pushed at the next tick too.
        let mut written = if o.lines {
            let len = file.metadata().map_err(Failed::at(Step::Open, &o.path))?;
            len.len()
        } else {
            0
        };
        write_retrying(
            &*self.io,
            &self.shared.abandoned,
            &mut file,
            &o.path,
            written,
            &o.head,
        )
        .map_err(Failed::at(Step::Write, &o.path))?;
        written += o.head.len() as u64;
        self.shared
            .bytes
            .fetch_add(o.head.len() as u64, Ordering::AcqRel);
        if self.files.len() <= o.slot {
            self.files.resize_with(o.slot + 1, || None);
        }
        self.files[o.slot] = Some(Run {
            key: o.key,
            path: o.path,
            file,
            written,
            dirty: written > 0,
            closed: false,
        });
        if written > 0 {
            self.dirty.push(o.slot);
        }
        Ok(())
    }

    fn append(&mut self, slot: usize, bytes: &[u8]) -> Result<(), Failed> {
        let Some(run) = self.files.get_mut(slot).and_then(Option::as_mut) else {
            tracing::error!(slot, "writer: an append for a file that isn't open");
            return Ok(());
        };
        let abandoned = &self.shared.abandoned;
        write_retrying(
            &*self.io,
            abandoned,
            &mut run.file,
            &run.path,
            run.written,
            bytes,
        )
        .map_err(|e| Failed::new(Step::Write, &run.path, e))?;
        run.written += bytes.len() as u64;
        self.shared
            .bytes
            .fetch_add(bytes.len() as u64, Ordering::AcqRel);
        if !run.dirty {
            run.dirty = true;
            self.dirty.push(slot);
        }
        Ok(())
    }

    pub(super) fn write_journal(&mut self, line: &[u8]) -> Result<(), Failed> {
        write_retrying(
            &*self.io,
            &self.shared.abandoned,
            &mut self.journal,
            &self.journal_path,
            self.journal_len,
            line,
        )
        .map_err(|e| Failed::new(Step::Write, &self.journal_path, e))?;
        self.journal_len += line.len() as u64;
        self.journal_dirty = true;
        self.shared
            .bytes
            .fetch_add(line.len() as u64, Ordering::AcqRel);
        Ok(())
    }

    /// Keeps the first failure, so every later call is `Gone`. After a failed append, one last tick makes what reached
    /// the files whole durable; after a failed push or flush nothing more is trusted (the OS may have dropped the data,
    /// and a second `fsync` can then succeed falsely), nor after the journal's own write failed.
    pub(super) fn fail(&mut self, f: Failed) {
        let reason = match classify(&f.error) {
            DiskReason::Full => DiskFailure::Full,
            DiskReason::ReadOnly => DiskFailure::ReadOnly,
            DiskReason::Denied => DiskFailure::Denied,
            DiskReason::Locked | DiskReason::Other => DiskFailure::Io,
        };
        tracing::error!(
            step = ?f.step,
            file = %tilde(&f.path.to_string_lossy()),
            error = %f.error,
            ?reason,
            "writer: a disk step failed; the recorder stops and saves"
        );
        self.failed = true;
        self.shared.state().failed = Some(reason);
        self.shared.failed.store(true, Ordering::Release);
        if f.step == Step::Write
            && f.path != self.journal_path
            && let Err(again) = self.tick(true)
        {
            tracing::warn!(error = %again.error, "writer: the last tick after a failure failed too");
        }
    }
}

impl Failed {
    pub(super) fn new(step: Step, path: &Path, error: io::Error) -> Self {
        Self {
            step,
            path: path.to_path_buf(),
            error,
        }
    }

    /// For `map_err`: `step` on `path` failed.
    pub(super) fn at(step: Step, path: &Path) -> impl FnOnce(io::Error) -> Self + '_ {
        move |error| Self::new(step, path, error)
    }
}
