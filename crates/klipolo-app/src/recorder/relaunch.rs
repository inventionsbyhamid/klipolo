//! Reopen Klipolo's relaunch. macOS applies a Screen Recording grant only to an app that starts again, so Reopen
//! Klipolo quits with an exit code of its own (`REOPEN_EXIT_CODE`), and as that exit goes ahead a waiter is started
//! that opens Klipolo again once this one has gone.
//!
//! The waiter is `/bin/sh`, outside the bundle, in a process group of its own (a signal to Klipolo's group, such as a
//! terminal's ^C, doesn't take it along) and with no stdio. It waits for this process to end, then LaunchServices opens
//! the bundle by its path (`open -n`), so Klipolo comes back in front, as an app with its own permissions: never as a
//! child of the Klipolo that is quitting, whose grant and focus would go with it. Outside a bundle (a dev build) it
//! runs the binary.
//!
//! The quit is an ordinary one, so the quit guards may hold it (an export that started meanwhile asks first, and its
//! Quit exits with the same code). Only an exit asked with the reopen's code that goes ahead relaunches. One a guard
//! held leaves nothing armed: the reopen ends, so Record works again, and "Keep exporting" followed by any later quit
//! (Command Q, the Dock's Quit, logging out) never reopens Klipolo. An exit asked any other way forgets it too.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager, RunEvent};

use super::state::RecorderState;

/// Reopen Klipolo's exit code: Klipolo's own, never Tauri's restart code.
pub(super) const REOPEN_EXIT_CODE: i32 = 75;

/// Whether the exit under way is Reopen Klipolo's: decided as each exit is asked, carried out at `RunEvent::Exit`.
#[derive(Default)]
pub(super) struct Relaunch {
    due: AtomicBool,
}

impl Relaunch {
    /// An exit was asked with `code` (None: the last window closed), and a quit guard `held` it (an export asks first,
    /// a take lands first) or let it go. Only the reopen's code with an exit that goes ahead relaunches Klipolo: a held
    /// one leaves nothing armed (should the person quit there, the guard asks the exit again with the same code), and
    /// any other code forgets an earlier one.
    pub(super) fn exit_requested(&self, code: Option<i32>, held: bool) {
        self.due
            .store(code == Some(REOPEN_EXIT_CODE) && !held, Ordering::SeqCst);
    }

    /// At exit: whether to relaunch, at most once.
    pub(super) fn due(&self) -> bool {
        self.due.swap(false, Ordering::SeqCst)
    }
}

/// Follows the app's exit: an exit asked with Reopen Klipolo's code that goes ahead (no quit guard `held` it) starts
/// the waiter as the app exits. One a guard held leaves nothing armed (`RecorderState::exit_requested`).
pub fn relaunch_on_exit(app: &AppHandle, event: &RunEvent, held: bool) {
    let Some(state) = app.try_state::<RecorderState>() else {
        return;
    };
    match event {
        RunEvent::ExitRequested { code, .. } => state.exit_requested(*code, held),
        RunEvent::Exit if state.relaunch().due() => start(),
        _ => {}
    }
}

/// Starts the waiter that opens Klipolo again once this process has ended. A failure is logged: the person opens
/// Klipolo themselves.
fn start() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            tracing::warn!(%error, "recorder: this app's binary can't be found, so Klipolo won't reopen");
            return;
        }
    };
    let (program, args) = waiter(&exe, std::process::id());
    let mut waiter = Command::new(&program);
    waiter
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut waiter, 0);
    match waiter.spawn() {
        Ok(_) => tracing::info!("recorder: Klipolo opens again once this one has quit"),
        Err(error) => tracing::warn!(%error, "recorder: Klipolo couldn't be started again"),
    }
}

/// Waits (at most 10 s) for process `$0` to end, then runs the rest of its arguments. `/bin/sleep` by its path (`kill`
/// is the shell's own), so a PATH without `sleep` can't make it spin.
pub(super) const AFTER_EXIT: &str = r#"i=0; while kill -0 "$0" 2>/dev/null && [ "$i" -lt 100 ]; do /bin/sleep 0.1; i=$((i+1)); done; exec "$@""#;

/// The waiter for this binary `exe`, process `pid`: `/bin/sh` runs `AFTER_EXIT`, then LaunchServices opens the bundle
/// `exe` is in by its path (`/usr/bin/open -n`), or, outside a bundle, the binary itself.
pub(super) fn waiter(exe: &Path, pid: u32) -> (PathBuf, Vec<OsString>) {
    let bundle = exe
        .ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"));
    let run: Vec<OsString> = match bundle {
        Some(bundle) => vec!["/usr/bin/open".into(), "-n".into(), bundle.into()],
        None => vec![exe.into()],
    };
    let mut args: Vec<OsString> = vec!["-c".into(), AFTER_EXIT.into(), pid.to_string().into()];
    args.extend(run);
    ("/bin/sh".into(), args)
}
