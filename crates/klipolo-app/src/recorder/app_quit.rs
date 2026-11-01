//! Klipolo's exit request during a take (`quitting`): held until the take has landed, or 30 s, then Klipolo exits with
//! the code it was asked to. Quits a Mac app hears nothing of beforehand (the Dock's Quit, logging out) aren't held: a
//! take that has started records on in its own worker, and one that hasn't is disarmed (`release_on_exit`).

use tauri::{AppHandle, Manager, RunEvent};

use super::active::active;
use super::state::RecorderState;

/// `RunEvent::ExitRequested` while a take is the recorder's: the exit is held, the take stopped, and Klipolo exits once
/// it has landed or after 30 s. True when it held the exit.
pub fn guard_quit(app: &AppHandle, event: &RunEvent) -> bool {
    let RunEvent::ExitRequested { api, code, .. } = event else {
        return false;
    };
    if !active(app) {
        return false;
    }
    let Some(state) = app.try_state::<RecorderState>() else {
        return false;
    };
    let handle = app.clone();
    let code = code.unwrap_or(0);
    let held = state.hold_quit(Box::new(move || handle.exit(code)));
    if held {
        api.prevent_exit();
    }
    held
}
