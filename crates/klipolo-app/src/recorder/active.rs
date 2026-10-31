//! Whether the recorder is active: a take is the recorder's, from arming through saving. The mic or camera may be held
//! then, and quitting and the engine's background media work wait for it.

use tauri::{AppHandle, Manager};

use super::state::RecorderState;
use super::take::Recorder;

impl Recorder {
    /// A take is the recorder's, armed through saving.
    pub(super) fn active(&self) -> bool {
        self.lock().take.is_some()
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
