//! Whether the recorder is active (a take is the recorder's, from arming through saving: the mic or camera may be held
//! then, and quitting and the engine's background media work wait for it), and what a take that hasn't started becomes
//! when Klipolo exits: disarmed, so no armed worker outlives the app with its global key.

use std::sync::atomic::Ordering;

use klipolo_capture::protocol::Command;
use tauri::{AppHandle, Manager, RunEvent};

use super::send::write_line;
use super::state::RecorderState;
use super::take::Recorder;

impl Recorder {
    /// A take is the recorder's, armed through saving. Read without the slot's lock.
    pub(super) fn active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Klipolo exits: a take that hasn't started is disarmed (no take, its folder gone, its key unregistered). A take
    /// past its start records on in its own worker, orphaned, and is stopped from its menu-bar item. True when a
    /// Disarm was sent.
    pub(super) fn disarm_unstarted(&self) -> bool {
        let mut slot = self.lock();
        let Some(take) = slot.take.as_mut() else {
            return false;
        };
        if take.sup.started() || take.sup.is_stopping() {
            return false;
        }
        tracing::info!(
            take = take.cfg.id,
            "recorder: Klipolo exits, so the armed take is disarmed"
        );
        take.sup.stop_requested();
        if let Some(worker) = take.worker.as_mut() {
            write_line(worker, &Command::Disarm);
        }
        true
    }
}

/// A take is armed, records or saves. Never in a test run, nor before the recorder is managed.
#[allow(
    dead_code,
    reason = "read by the quit guard and the engine's hold, which come with the take's landing"
)]
pub fn active(app: &AppHandle) -> bool {
    app.try_state::<RecorderState>()
        .is_some_and(|state| state.active())
}

/// On Klipolo's exit, a take that hasn't started is disarmed (`Recorder::disarm_unstarted`). Should Klipolo crash
/// instead, the worker disarms itself when its parent goes.
pub fn release_on_exit(app: &AppHandle, event: &RunEvent) {
    if let RunEvent::Exit = event
        && let Some(state) = app.try_state::<RecorderState>()
    {
        state.disarm_unstarted();
    }
}
