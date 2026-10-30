//! The recorder's process: spawned so it outlives the app, and stopped by the app's supervisor when it
//! hangs.

use std::io;
use std::path::Path;

#[cfg(windows)]
use crate::imp as os;
#[cfg(unix)]
use crate::unix as os;

/// The one spawn rule for the recorder: `klipolo --record-worker` outlives the app that launched it (an
/// editor crash or quit leaves it recording, orphaned, with Stop in its own menu-bar or tray item), so it
/// runs in a session and process group of its own and never dies with its parent.
/// - macOS and Linux: `setsid()` in `pre_exec`: a new session and process group, so the terminal's or the app's
///   group signals (Ctrl-C, a group kill) don't reach it. Never `PR_SET_PDEATHSIG`.
/// - Windows: `CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW`: out of the app's console group
///   and any job that kills its children with it, and no console window.
pub fn recorder_command(exe: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(exe);
    os::recorder_flags(&mut command);
    command
}

/// `recorder_command` for an app inside a job that forbids breaking away: Windows refuses
/// `CREATE_BREAKAWAY_FROM_JOB` there (`CreateProcess` fails with access denied), so the recorder stays in the job,
/// and may end with it (review). Elsewhere it is `recorder_command`.
pub fn recorder_command_in_job(exe: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(exe);
    os::recorder_flags_in_job(&mut command);
    command
}

/// Stops `child`, which the caller spawned and hasn't reaped (so its id can't name another process yet). `hard`: at once
/// (`SIGKILL`; Windows `TerminateProcess`). Otherwise it is asked to stop (`SIGTERM` on macOS and Linux; Windows has no
/// such request for a windowless process, so this does nothing there and the caller's hard stop follows). A child that
/// has already exited is `Ok`.
pub fn stop_child(child: &mut std::process::Child, hard: bool) -> io::Result<()> {
    if hard {
        return match child.kill() {
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => Ok(()),
            other => other,
        };
    }
    os::ask_to_stop(child)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    /// The recorder leads a session of its own (so the app's group signals and its death never reach it), and a stop
    /// request is a `SIGTERM`.
    #[test]
    fn the_recorder_runs_in_a_session_of_its_own_and_is_asked_to_stop() {
        let mut child = {
            // Never while a lock test runs (`process::tests::SPAWN`).
            let _spawning = (crate::process::tests::SPAWN.read())
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            recorder_command(Path::new("sleep"))
                .arg("30")
                .spawn()
                .unwrap()
        };
        let pid = libc::pid_t::try_from(child.id()).unwrap();
        // SAFETY: getsid on our own unreaped child and on this process.
        let (sid, ours) = unsafe { (libc::getsid(pid), libc::getsid(0)) };
        let asked = stop_child(&mut child, false);
        let status = child.wait().unwrap();
        assert_eq!(sid, pid, "a new session, led by the recorder");
        assert_ne!(sid, ours);
        asked.unwrap();
        assert_eq!(status.signal(), Some(libc::SIGTERM));
        stop_child(&mut child, true).unwrap();
    }
}
