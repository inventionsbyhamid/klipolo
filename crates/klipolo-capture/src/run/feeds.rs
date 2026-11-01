//! The worker's small threads (`run::threads`'s module doc): the app's commands in, events out, the ticker and the
//! shell's inputs, and the session's end: the last events out and the clean-up of a take that never started.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::Instant;

use klipolo_platform::durable::free_space;

use super::threads::{Input, POLL, TICK};
use crate::protocol::{self, Command, Event};
use crate::session::SessionDir;
use crate::shell::ShellInput;

/// The app's commands, one per line, into `core`. A Stop, Cancel or Disarm sets `stop_seen` before it is passed on, so
/// no device opens from then on, whatever is still on its way to the core.
pub(super) fn read_commands(
    input: Box<dyn BufRead + Send>,
    core: SyncSender<Input>,
    stop_seen: Arc<AtomicBool>,
) -> impl FnOnce() + Send {
    move || {
        for line in input.lines() {
            let Ok(line) = line else { break };
            let Some(cmd) = protocol::decode_command(&line) else {
                continue;
            };
            if matches!(cmd, Command::Stop | Command::Cancel | Command::Disarm) {
                stop_seen.store(true, Ordering::Release);
            }
            if core.send(Input::Command(cmd)).is_err() {
                return;
            }
        }
        let _ = core.send(Input::ParentGone);
    }
}

pub(super) fn forward_shell(
    rx: Receiver<ShellInput>,
    core: SyncSender<Input>,
) -> impl FnOnce() + Send {
    move || {
        for input in rx {
            if core.send(Input::Shell(input)).is_err() {
                return;
            }
        }
    }
}

/// A tick once a second, with the free space read every other second, and, while a separate cursor is watched
/// (`polls`), a poll every `POLL` between (other takes don't wake the core ten times a second). Each wake is
/// at its own deadline from the start, so ten sleeps never stretch the second the durable lengths ride on.
pub(super) fn tick(
    root: PathBuf,
    core: SyncSender<Input>,
    ticking: Arc<AtomicBool>,
    polls: bool,
) -> impl FnOnce() + Send {
    let step = if polls { POLL } else { TICK };
    let per_tick = (TICK.as_millis() / step.as_millis()).max(1) as u64;
    move || {
        let (mut n, mut free) = (0u64, None);
        let start = Instant::now();
        while ticking.load(Ordering::Acquire) {
            n += 1;
            let due = start + step * u32::try_from(n).unwrap_or(u32::MAX);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            let input = if n.is_multiple_of(per_tick) {
                if (n / per_tick) % 2 == 1 {
                    free = free_space(&root);
                }
                Input::Tick { free }
            } else {
                Input::Poll
            };
            if core.send(input).is_err() {
                return;
            }
        }
    }
}

pub(super) fn write_events(mut output: Box<dyn Write + Send>, rx: Receiver<Event>) {
    let mut gone = false;
    for ev in rx {
        if !gone
            && output
                .write_all(&protocol::encode(&ev))
                .and_then(|()| output.flush())
                .is_err()
        {
            // The parent is gone: the worker records on, orphaned; its events have nowhere to go.
            gone = true;
        }
    }
}

/// A take with no run in it (stopped before its gate): its folder goes. Anything recorded keeps it. The worker runs it
/// at exit 3; the app's supervisor runs it too when a worker ended before the gate some other way.
pub fn remove_empty_take(dir: &SessionDir) {
    let recorded = |p: &Path| {
        std::fs::read_dir(p).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.path().is_dir()
                    && std::fs::read_dir(e.path()).is_ok_and(|mut f| f.next().is_some())
            })
        })
    };
    if recorded(&dir.root) {
        tracing::warn!("worker: a take without a gate holds files, so it is kept");
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(&dir.root) {
        tracing::warn!(error = %e, "worker: an empty take's folder wasn't removed");
    }
}

/// What is left once the session's core has ended: the app's last events written out, and the folder of a take that
/// never began (exit 3), which holds no run, removed.
pub(super) struct Ending {
    pub(super) events: SyncSender<Event>,
    pub(super) writer: std::io::Result<JoinHandle<()>>,
    pub(super) dir: SessionDir,
}

impl Ending {
    /// Returns `code`, the worker's exit code.
    pub(super) fn end(self, code: i32) -> i32 {
        drop(self.events);
        if let Ok(writer) = self.writer {
            let _ = writer.join();
        }
        if code == 3 {
            remove_empty_take(&self.dir);
        }
        code
    }
}
