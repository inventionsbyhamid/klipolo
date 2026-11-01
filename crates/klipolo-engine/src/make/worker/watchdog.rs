//! The worker's watchdog: a worker that outlives the app must exit, whatever its loop is
//! doing (a long GPU request included). A thread of its own checks every `TICK` whether the parent is still the process
//! that started it (`parent_id`: once the app exits, the worker is adopted by launchd, init or a subreaper; an adopted
//! worker's parent is 1 on macOS) and whether stdin has closed (the app's end of the pipe is gone). Either way the
//! process exits within 1 s: at once for a new parent, and `GRACE` after stdin closed while no request runs (a request
//! in flight finishes and the loop then reads the end). Its last words can't stop it: a closed
//! stderr (the app's pipe, once the app is gone) is ignored, and it leaves with `_exit`, which runs no handler and
//! can't block on the busy main thread. The app's sweep at its next start stays the backstop.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use klipolo_platform::process::sandbox::parent_id;

/// How often it looks.
const TICK: Duration = Duration::from_millis(200);
/// How long after stdin closed, with no request running, the loop may still run.
const GRACE: Duration = Duration::from_millis(500);

/// Starts the watchdog. `closed` is set by the stdin thread when stdin ends or fails; `busy` while a request runs.
/// Exits the process with `code`.
pub(super) fn start(
    closed: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    code: i32,
) -> std::io::Result<()> {
    let parent = parent_id();
    std::thread::Builder::new()
        .name("render-worker-watchdog".into())
        .spawn(move || {
            let mut idle = Idle::default();
            loop {
                std::thread::sleep(TICK);
                let closed_for = idle.closed_for(
                    closed.load(Ordering::SeqCst),
                    busy.load(Ordering::SeqCst),
                    Instant::now(),
                );
                if let Some(why) = verdict(parent, parent_id(), closed_for) {
                    gone(why, code);
                }
            }
        })
        .map(drop)
}

/// How long stdin has been closed with no request running: a request in flight stops the count, which starts
/// again once the worker is idle.
#[derive(Default)]
struct Idle {
    since: Option<Instant>,
}

impl Idle {
    fn closed_for(&mut self, closed: bool, busy: bool, now: Instant) -> Option<Duration> {
        if closed && !busy {
            Some(now.duration_since(*self.since.get_or_insert(now)))
        } else {
            self.since = None;
            None
        }
    }
}

/// Why the worker must stop now, if it must: it was adopted (its parent is now 1, launchd or init, or isn't the one it
/// started with: the first also covers a parent that exited before the watchdog first looked), or stdin has been
/// closed, with no request running, for `GRACE`.
pub(super) fn verdict(
    start: Option<u32>,
    now: Option<u32>,
    closed_for: Option<Duration>,
) -> Option<&'static str> {
    if now == Some(1) || (start.is_some() && now != start) {
        return Some("its parent exited");
    }
    closed_for
        .filter(|d| *d >= GRACE)
        .map(|_| "its stdin closed")
}

fn gone(why: &str, code: i32) -> ! {
    // Never `eprintln!`, which panics on a closed stderr and would leave the worker running.
    let _ = writeln!(std::io::stderr(), "render worker: {why}, so it stops");
    #[cfg(unix)]
    // SAFETY: `_exit` takes an int and ends the process at once; nothing of this process runs after it.
    unsafe {
        libc::_exit(code)
    }
    #[cfg(not(unix))]
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_stops_when_adopted_or_idle_with_stdin_closed() {
        let ms = Duration::from_millis;
        assert_eq!(verdict(Some(40), Some(40), None), None);
        assert!(
            verdict(Some(40), Some(1), None).is_some(),
            "adopted by launchd or init"
        );
        assert!(
            verdict(Some(40), Some(77), None).is_some(),
            "adopted by a subreaper"
        );
        assert!(
            verdict(Some(1), Some(1), None).is_some(),
            "its parent exited before the first look"
        );
        assert_eq!(
            verdict(Some(40), Some(40), Some(ms(300))),
            None,
            "within the grace"
        );
        assert!(verdict(Some(40), Some(40), Some(ms(600))).is_some());
        assert_eq!(
            verdict(None, None, None),
            None,
            "no parent to read (Windows)"
        );
    }

    /// A request in flight holds the stdin rule off, however long it runs; the grace starts once it ends.
    #[test]
    fn a_request_in_flight_holds_the_stdin_rule_off() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut idle = Idle::default();
        for ms in [0, 200, 400, 600, 5_000] {
            let closed_for = idle.closed_for(true, true, at(ms));
            assert_eq!(
                verdict(Some(40), Some(40), closed_for),
                None,
                "busy at {ms} ms"
            );
        }
        assert_eq!(
            idle.closed_for(true, false, at(5_200)),
            Some(Duration::ZERO)
        );
        let closed_for = idle.closed_for(true, false, at(5_400));
        assert_eq!(
            verdict(Some(40), Some(40), closed_for),
            None,
            "within the grace"
        );
        let closed_for = idle.closed_for(true, false, at(5_800));
        assert!(verdict(Some(40), Some(40), closed_for).is_some());
    }
}
