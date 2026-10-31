//! The writer thread's flush tick, the lines that jump the queue, its retried appends and its ring of tick times (the
//! module doc in `writer`).

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use klipolo_platform::durable::{DiskReason, classify};
use klipolo_platform::paths::tilde;

use super::thread::{Failed, Run, Step, Thread};
use crate::fail::{fail_armed, fail_point};
use crate::io::Io;
use crate::journal::{self, Entry};
use crate::recent::Recent;
use crate::session::FileKey;

/// The waits before each retry of an append that failed with an I/O error.
const RETRY_MS: [u64; 3] = [50, 200, 800];
/// The ticks `tick_p99_ms` is taken over.
const TICKS: usize = 300;

impl Thread {
    /// The flush tick (the module doc). Without anything new it does nothing, unless it is the `last`.
    pub(super) fn tick(&mut self, last: bool) -> Result<(), Failed> {
        let closing = self.files.iter().flatten().any(|r| r.closed);
        if !last && self.dirty.is_empty() && !self.journal_dirty && !closing {
            return Ok(());
        }
        let started = Instant::now();
        let journal = &self.journal_path;
        fail_point!("writer.beforePush").map_err(Failed::at(Step::Push, journal))?;
        let files = &mut self.files;
        self.dirty
            .sort_unstable_by(|&a, &b| key_of(files, a).cmp(key_of(files, b)));
        for &slot in &self.dirty {
            if let Some(run) = files[slot].as_ref() {
                self.io
                    .push(&run.file, &run.path)
                    .map_err(Failed::at(Step::Push, &run.path))?;
            }
        }
        let pushed = !self.dirty.is_empty();
        for slot in self.dirty.drain(..) {
            if let Some(run) = files[slot].as_mut() {
                run.dirty = false;
            }
        }
        if pushed {
            self.io
                .order(&self.journal, journal)
                .map_err(Failed::at(Step::Order, journal))?;
        }
        fail_point!("writer.beforeDurable").map_err(Failed::at(Step::Write, journal))?;
        let through: BTreeMap<FileKey, u64> = files
            .iter()
            .flatten()
            .map(|r| (r.key.clone(), r.written))
            .collect();
        if !through.is_empty() {
            let line = journal::line(&Entry::Durable {
                through: through.clone(),
            });
            self.write_journal(&line)?;
        }
        let journal = &self.journal_path;
        self.io
            .push(&self.journal, journal)
            .map_err(Failed::at(Step::Push, journal))?;
        fail_point!("writer.beforeBarrier").map_err(Failed::at(Step::Barrier, journal))?;
        self.io
            .barrier(&self.journal, journal)
            .map_err(Failed::at(Step::Barrier, journal))?;
        fail_point!("writer.afterBarrier").map_err(Failed::at(Step::Barrier, journal))?;
        self.journal_dirty = false;
        for slot in &mut self.files {
            if slot.as_ref().is_some_and(|r| r.closed) {
                *slot = None;
            }
        }
        let ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
        self.ticks.push(ms);
        let mut state = self.shared.state();
        state.durable.extend(through);
        state.last_tick_ms = ms;
        state.tick_p99_ms = self.ticks.quantile(0.99);
        Ok(())
    }
}

impl Thread {
    /// The lines that jump the queue, written and synced at once, ahead of whatever is still queued (the journal's
    /// ordering promise is about `Durable`, which these don't vouch for). After a failure they stay pending.
    pub(super) fn write_urgent(&mut self) {
        if self.failed {
            return;
        }
        let lines = std::mem::take(
            &mut *self
                .shared
                .urgent
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        if lines.is_empty() {
            return;
        }
        let path = self.journal_path.clone();
        let written = (lines.iter())
            .try_for_each(|line| self.write_journal(line))
            .and_then(|()| {
                self.io
                    .push(&self.journal, &path)
                    .map_err(Failed::at(Step::Push, &path))?;
                self.io
                    .barrier(&self.journal, &path)
                    .map_err(Failed::at(Step::Barrier, &path))
            });
        match written {
            Ok(()) => {
                self.shared
                    .urgent_pending
                    .fetch_sub(lines.len(), Ordering::AcqRel);
            }
            Err(f) => self.fail(f),
        }
    }
}

fn key_of(files: &[Option<Run>], slot: usize) -> &str {
    files[slot].as_ref().map_or("", |r| r.key.as_str())
}

/// Appends `bytes` to a file `at` bytes long. An I/O error (EIO, a lock held by another program) is retried after
/// 50, 200 and 800 ms, each time from `at`: what the failed try left is cut off first, so nothing is doubled. A full,
/// read-only or forbidden disk is the answer at once, and so is any error once the writer is `abandoned`: its cut
/// would remove what Stop's own handle appended to the journal meanwhile.
pub(super) fn write_retrying(
    io: &dyn Io,
    abandoned: &AtomicBool,
    file: &mut File,
    path: &Path,
    at: u64,
    bytes: &[u8],
) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut waits = RETRY_MS.iter();
    loop {
        let e = match try_append(io, file, path, bytes) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        let retry = matches!(classify(&e), DiskReason::Other | DiskReason::Locked);
        let Some(&ms) = waits.next().filter(|_| retry) else {
            return Err(e);
        };
        tracing::warn!(file = %tilde(&path.to_string_lossy()), error = %e, ms, "writer: an append failed; retrying");
        std::thread::sleep(Duration::from_millis(ms));
        if abandoned.load(Ordering::Acquire) {
            return Err(e);
        }
        io.truncate(file, path, at)?;
    }
}

fn try_append(io: &dyn Io, file: &mut File, path: &Path, bytes: &[u8]) -> io::Result<()> {
    fail_point!("writer.beforeAppend")?;
    if fail_armed!("writer.midAppend") {
        let (first, rest) = bytes.split_at(bytes.len() / 2);
        io.append(file, path, first)?;
        fail_point!("writer.midAppend")?;
        return io.append(file, path, rest);
    }
    io.append(file, path, bytes)
}

/// The last 300 tick times, in ms.
pub(super) type Ticks = Recent<u32, TICKS>;
