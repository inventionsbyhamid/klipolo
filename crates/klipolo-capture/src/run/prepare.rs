//! What a worker does with its take before anything runs (`run::threads`): a fresh take is armed; a resumed one is
//! finished first (`run::resume`) and goes on recording, unless the person already stopped it. A journaled Stop (Task
//! or a Stop or Cancel that reached the worker while it prepared, only finishes the
//! take: no pipe, no device, whatever the app asked.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use klipolo_model::coded::CodedError;
use klipolo_platform::clock::host_ns;
use klipolo_platform::durable::Durability;

use super::core::SessionCore;
use super::resume;
use super::threads::{Input, failed_with};
use crate::backend::Backend;
use crate::english::msg;
use crate::io::OsIo;
use crate::journal::Entry;
use crate::protocol::Command;
use crate::recover::RecoverError;
use crate::session::{self, SessionDir, SessionLock};

/// What the worker does with the take (`prepare`).
pub(super) enum Prepared {
    /// Record: a fresh take, armed, or a resumed one, with the take's lock and the commands read while preparing.
    Record(SessionLock, Box<SessionCore>, Vec<Input>),
    /// A resumed take whose Stop was journaled: finished without recording (this session's id).
    Finished(String),
    /// With the exit code.
    Failed(CodedError, i32),
}

/// The app's commands for the session: those read while it prepared, then the rest as they come.
pub(super) struct Commands {
    pub(super) early: Vec<Input>,
    pub(super) rest: Receiver<Input>,
    /// Set once a Stop, Cancel or Disarm was read: no device opens from then on (`run::controller`).
    pub(super) stop_seen: Arc<AtomicBool>,
}

/// The take's lock and its core: a fresh take, armed; or a resumed one (`run::resume`). A resumed take the person
/// already stopped is only finished here, so no device opens after their Stop; and so is one
/// whose Stop or Cancel arrived while it prepared (`commands`).
pub(super) fn prepare(
    backend: &dyn Backend,
    dir: &SessionDir,
    resume: bool,
    commands: &Receiver<Input>,
    stop_seen: &AtomicBool,
) -> Prepared {
    let cfg = match session::read_config(dir) {
        Ok(cfg) => cfg,
        Err(e) => return Prepared::Failed(failed_with(&e.to_string()), 1),
    };
    let now = host_ns();
    let busy = || msg!("capture.busy", place = backend.caps().indicator.coded());
    let clicks = backend.caps().cursor.clicks;
    if !resume {
        return match SessionLock::try_acquire(dir) {
            Ok(Some(lock)) => Prepared::Record(
                lock,
                Box::new(SessionCore::new(cfg, backend.mics(), now).with_cursor_clicks(clicks)),
                Vec::new(),
            ),
            Ok(None) => Prepared::Failed(busy(), 1),
            Err(e) => Prepared::Failed(failed_with(&e.to_string()), 1),
        };
    }
    match resume::resume(dir, now) {
        // A take that never passed its gate has nothing to go on with.
        Ok((_, from)) if !from.clock.started() => {
            Prepared::Failed(failed_with("nothing to resume"), 3)
        }
        Ok((_lock, from)) if from.stopped => {
            match super::finish::finish_stopped(&OsIo, dir, &cfg, &from.done) {
                Ok(()) => Prepared::Finished(cfg.id),
                Err(e) => Prepared::Failed(failed_with(&e.to_string()), 1),
            }
        }
        Ok((lock, from)) => {
            let early: Vec<Input> = commands.try_iter().collect();
            // The flag too: a Stop read between the drain and here is seen.
            let stopped = stop_seen.load(Ordering::Acquire)
                || early.iter().any(|i| {
                    matches!(
                        i,
                        Input::Command(Command::Stop | Command::Cancel | Command::Disarm)
                    )
                });
            if !stopped {
                let core =
                    SessionCore::resumed(cfg, backend.mics(), from, now).with_cursor_clicks(clicks);
                return Prepared::Record(lock, Box::new(core), early);
            }
            // The person stopped it before anything opened: journal the Stop and finish, as for C-1.
            let stop = Entry::StopRequested {
                at_us: from.clock.elapsed_us(host_ns()),
                reason: None,
            };
            let finished = crate::recover::Lines::new(&OsIo, dir.journal())
                .write(&stop, Durability::Full)
                .and_then(|()| super::finish::finish_stopped(&OsIo, dir, &cfg, &from.done));
            match finished {
                Ok(()) => Prepared::Finished(cfg.id),
                Err(e) => Prepared::Failed(failed_with(&e.to_string()), 1),
            }
        }
        Err(RecoverError::Live) => Prepared::Failed(busy(), 1),
        Err(e) => Prepared::Failed(failed_with(&e.to_string()), 1),
    }
}
