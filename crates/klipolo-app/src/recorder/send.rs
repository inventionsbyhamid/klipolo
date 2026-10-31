//! Commands to the take's worker: the page's, by the take's id (`send`), and "Stop the running recording" to whichever
//! take runs (`stop_running`). The id check and the write are one step under the recorder's lock, and nothing records
//! after the person's Stop, Cancel or Disarm.

use std::io::Write;
use std::sync::TryLockError;

use klipolo_capture::protocol::Command;
use klipolo_model::coded::CodedError;

use super::take::{Recorder, Slot, Worker};
use super::types::StoppedTake;
use super::words;

impl Recorder {
    /// One command to take `take`'s worker (`ArmedTake.id`). The page may send a take's own commands only (Start,
    /// Pause, Resume, Stop, Cancel, Disarm, WithoutSource): the others are the app's or the worker's
    /// (`capture.error.unsupported`). A take that isn't the current one (it ended, or a newer one was armed) is refused
    /// with `capture.error.staleTake`, so a late Cancel meant for an earlier take never reaches the new one.
    pub fn send(&self, take: &str, cmd: Command) -> Result<(), CodedError> {
        if matches!(
            cmd,
            Command::Marker | Command::StartAgain | Command::ViewSize { .. }
        ) {
            return Err(words::refused());
        }
        let mut slot = self.lock();
        let Some(current) = slot.take.as_mut().filter(|t| t.cfg.id == take) else {
            tracing::info!(
                take,
                ?cmd,
                "recorder: a command for a take that isn't the current one"
            );
            return Err(words::stale_take());
        };
        match cmd {
            // Nothing records after the person's Stop, Cancel or Disarm.
            Command::Start if current.sup.is_stopping() => {
                tracing::info!(take, "recorder: a Start after Stop isn't passed on");
                return Ok(());
            }
            Command::Stop | Command::Cancel | Command::Disarm => current.sup.stop_requested(),
            _ => {}
        }
        if let Some(worker) = current.worker.as_mut() {
            write_line(worker, &cmd);
        }
        Ok(())
    }

    /// "Stop the running recording": Stop for whatever take the recorder runs, whichever id the page lost. Its id and
    /// whether it had started (only then is it saved), or None when there is no take. Stop is safe in every state: it
    /// never starts capture.
    pub fn stop_running(&self) -> Option<StoppedTake> {
        stop_in(&mut self.lock())
    }

    /// `stop_running` only when the recorder's lock is free now, so a caller that mustn't wait for file work under it
    /// (the main thread) never does: false when it is held, and nothing was sent.
    pub(super) fn stop_running_if_free(&self) -> bool {
        let mut slot = match self.slot.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::Poisoned(slot)) => slot.into_inner(),
            Err(TryLockError::WouldBlock) => return false,
        };
        stop_in(&mut slot);
        true
    }
}

/// Stop for the slot's take, whatever its id (`Recorder::stop_running`).
fn stop_in(slot: &mut Slot) -> Option<StoppedTake> {
    let take = slot.take.as_mut()?;
    take.sup.stop_requested();
    if let Some(worker) = take.worker.as_mut() {
        write_line(worker, &Command::Stop);
    }
    Some(StoppedTake {
        id: take.cfg.id.clone(),
        started: take.sup.started(),
    })
}

/// `cmd` as one line on the worker's stdin. A worker that has gone can't hear it: its exit is supervised.
pub(super) fn write_line(worker: &mut Worker, cmd: &Command) {
    let mut line = serde_json::to_vec(cmd).unwrap_or_default();
    line.push(b'\n');
    let written = (worker.stdin.write_all(&line)).and_then(|()| worker.stdin.flush());
    if let Err(error) = written {
        tracing::warn!(generation = worker.generation, ?cmd, %error, "recorder: a command didn't reach the worker");
    }
}
