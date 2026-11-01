//! A system quit (logout, restart, shutdown, Activity Monitor's Quit) while the worker runs: the shell
//! asks the session to Stop through the menu's own path, waits for its end within a bound, then ends the process with
//! the session's own exit code, never mid-take with the OS's 0.

use std::time::Duration;

use crate::shell::WorkerState;

/// How long a take may take to stop and save. Past it the journal is durable and recovery finishes the take on the
/// next launch (exit 1, "recover it").
pub const QUIT_TAKE_WAIT: Duration = Duration::from_secs(10);
/// With no take running, the Stop ends the worker at once (a Stop before the gate leaves no take), or it is already
/// ending: a short bound for the session's last steps.
pub const QUIT_IDLE_WAIT: Duration = Duration::from_secs(2);
/// The exit code when the session hasn't ended within the bound (`protocol`: failed, recover it).
pub const QUIT_OVERDUE_CODE: i32 = 1;

/// How long a system quit waits for the session in `state` (None: no status yet) after asking it to stop. `Starting`
/// gets the take's bound: a pipe's start can wait on the system's picker, and its Stop then ends a take whose folder
/// exists.
pub fn quit_wait(state: Option<WorkerState>) -> Duration {
    match state {
        Some(
            WorkerState::Starting
            | WorkerState::Countdown
            | WorkerState::Recording
            | WorkerState::Paused
            | WorkerState::Stopping,
        ) => QUIT_TAKE_WAIT,
        _ => QUIT_IDLE_WAIT,
    }
}
