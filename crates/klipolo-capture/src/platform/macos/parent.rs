//! The parent watch: the app that launched the worker going away (an editor crash, a force
//! quit) is seen at once through a kqueue `EVFILT_PROC`/`NOTE_EXIT` on its pid, and the shell sends
//! `ShellInput::ParentGone`: the worker records on, orphaned, with Stop in its menu-bar item. The end of the app's
//! commands on stdin says the same later (`run::feeds::read_commands`); the session takes either.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use crate::shell::ShellInput;

/// Watches the worker's parent on a thread of its own, which sleeps in `kevent` until the parent exits.
pub(super) fn watch(input: SyncSender<ShellInput>) {
    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() };
    let spawned = std::thread::Builder::new()
        .name("klipolo-parent".into())
        .spawn(move || {
            let gone = match ExitWatch::new(parent) {
                // Registered: a parent that exited just before left the worker to launchd (pid 1) already.
                Ok(Some(watch)) => orphaned() || watch.wait(None).unwrap_or(false),
                Ok(None) => true,
                Err(error) => {
                    tracing::warn!(%error, "shell: the parent can't be watched; stdin's end still tells");
                    false
                }
            };
            if gone {
                tracing::info!("shell: the app that started the worker is gone; recording on");
                let _ = input.send(ShellInput::ParentGone);
            }
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "shell: the parent watch didn't start");
    }
}

/// The worker's parent is launchd: the app that started it is gone.
fn orphaned() -> bool {
    // SAFETY: getppid has no preconditions.
    unsafe { libc::getppid() == 1 }
}

/// A kqueue watching one process for its exit.
pub(crate) struct ExitWatch {
    queue: OwnedFd,
}

impl ExitWatch {
    /// Watches `pid`; None when it is already gone (`ESRCH`).
    pub(crate) fn new(pid: libc::pid_t) -> io::Result<Option<Self>> {
        // SAFETY: kqueue takes no arguments; the descriptor it returns is owned below.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a descriptor just made, owned by nothing else.
        let queue = unsafe { OwnedFd::from_raw_fd(fd) };
        let change = libc::kevent {
            ident: pid as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one change in, no events out, no timeout.
        let added = unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if added < 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ESRCH) => Ok(None),
                _ => Err(error),
            };
        }
        Ok(Some(Self { queue }))
    }

    /// Waits for the exit: true once it is seen, false when `timeout` passes first (None: no limit).
    pub(crate) fn wait(&self, timeout: Option<Duration>) -> io::Result<bool> {
        let limit = timeout.map(|t| libc::timespec {
            tv_sec: libc::time_t::try_from(t.as_secs()).unwrap_or(libc::time_t::MAX),
            tv_nsec: libc::c_long::from(t.subsec_nanos()),
        });
        let limit = limit.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
        // SAFETY: an all-zero kevent is a valid out-value.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: no changes in, room for one event out, and a timeout (or null) that outlives the call.
            let n = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &mut event,
                    1,
                    limit,
                )
            };
            match n {
                1 => return Ok(event.fflags & libc::NOTE_EXIT != 0),
                0 => return Ok(false),
                _ => {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn a_watched_process_exit_is_seen() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("0.2")
            .spawn()
            .expect("/bin/sleep runs");
        let pid = libc::pid_t::try_from(child.id()).unwrap();
        let watch = ExitWatch::new(pid).unwrap().expect("still running");
        let started = Instant::now();
        let seen = watch.wait(Some(Duration::from_millis(1_500))).unwrap();
        assert!(seen, "the exit was seen within 1.5 s");
        assert!(started.elapsed() < Duration::from_millis(1_500));
        child.wait().unwrap();
    }
}
