//! The process plumbing's tests: locks, liveness, a child's memory and start, stopping only the process asked for.
use std::sync::{PoisonError, RwLock};

use super::*;

/// flock is per open file description, and a child another test forks holds a copy of it until it execs (CLOEXEC
/// closes it only then), so a lock test overlapping that window sees its file still locked: tests take this shared
/// while they spawn, a lock test exclusive.
pub(super) static SPAWN: RwLock<()> = RwLock::new(());

/// `command` started, never while a lock test runs ([`SPAWN`]).
fn spawned(command: &mut std::process::Command) -> std::process::Child {
    let _spawning = SPAWN.read().unwrap_or_else(PoisonError::into_inner);
    command.spawn().unwrap()
}

#[test]
fn a_second_exclusive_lock_is_refused_until_the_first_closes() {
    let _alone = SPAWN.write().unwrap_or_else(PoisonError::into_inner);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lock");
    let open = || {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .unwrap()
    };
    let first = open();
    lock_exclusive(&first).unwrap();
    let second = open();
    let refused = lock_exclusive(&second).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);
    drop(first);
    lock_exclusive(&second).unwrap();
}

#[test]
fn this_process_is_alive_and_an_unused_id_is_not() {
    assert!(is_alive(std::process::id()));
    assert!(!is_alive(0));
    // Beyond every OS's process id limit.
    assert!(!is_alive(999_999_999));
    let now = Some(SystemTime::now());
    assert!(!is_leftover(Some(std::process::id()), None));
    assert!(is_leftover(Some(999_999_999), now));
    assert!(!is_leftover(None, now));
    assert!(is_leftover(None, Some(SystemTime::now() - UNOWNED_AFTER)));
}

#[test]
fn this_process_says_its_anonymous_memory() {
    // Benches read it (Linux RssAnon, macOS phys_footprint).
    assert!(own_anonymous_memory().is_some_and(|bytes| bytes > 0));
}

/// A child that is killed and reaped however the test ends, so a failed assertion leaves nothing running.
struct Reaped(std::process::Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_child_has_memory_and_a_start_time_and_can_be_stopped() {
    let mut child = Reaped(spawned(
        std::process::Command::new(if cfg!(windows) { "cmd" } else { "sleep" }).args(
            if cfg!(windows) {
                vec!["/C", "timeout", "30"]
            } else {
                vec!["30"]
            },
        ),
    ));
    let pid = child.0.id();
    if cfg!(unix) {
        let mem = memory(pid).expect("a live child has memory figures");
        assert!(mem.resident > 0);
        let started = start_time(pid).expect("and a start time");
        assert_eq!(
            start_time(pid),
            Some(started),
            "stable for the same process"
        );
        // Absolute (µs since 1970): a child spawned just now started within seconds of the wall clock. A count
        // from boot would be off by the uptime.
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;
        assert!(started.abs_diff(now) < 5_000_000, "{started} vs {now}");
        let boot = boot_id().expect("a boot id");
        assert!(still_running(pid, started, &boot));
        assert!(
            !still_running(pid, started, "an-earlier-boot"),
            "a pidfile from before a reboot never matches"
        );
        // Another start under the same id: neither reprioritised nor signalled.
        let other = started + 1;
        assert!(!still_running(pid, other, &boot));
        let refused = set_background(pid, other, true).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::NotFound);
        terminate(pid, other).unwrap();
        assert!(still_running(pid, started, &boot), "left alone");
        set_background(pid, started, true).unwrap();
        set_background(pid, started, false).unwrap();
        terminate(pid, started).unwrap();
        // Exited, not reaped yet: no figures, it no longer runs, and stopping it again is done at once.
        assert_eq!(memory(pid), None);
        assert!(!still_running(pid, started, &boot));
        terminate(pid, started).unwrap();
        let _ = child.0.wait();
        assert!(!is_alive(pid));
    }
    assert_eq!(start_time(999_999_999), None);
}

#[cfg(unix)]
#[test]
fn a_child_names_its_program_and_is_killed_at_once_only_by_its_own_start() {
    let mut child = Reaped(spawned(
        std::process::Command::new("sh").args(["-c", "trap '' TERM; exec sleep 30"]),
    ));
    let pid = child.0.id();
    // `sh` execs `sleep`: wait for the image to change.
    let mut exe = None;
    for _ in 0..100 {
        exe = exe_path(pid);
        if exe.as_ref().is_some_and(|p| p.ends_with("sleep")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let exe = exe.expect("a path");
    assert!(exe.is_absolute() && exe.ends_with("sleep"), "{exe:?}");
    assert_eq!(exe_path(999_999_999), None);
    let started = start_time(pid).unwrap();
    kill(pid, started + 1).unwrap();
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "another start: left alone"
    );
    let begun = std::time::Instant::now();
    kill(pid, started).unwrap();
    child.0.wait().unwrap();
    assert!(
        begun.elapsed() < Duration::from_secs(1),
        "SIGTERM is ignored, so only SIGKILL ends it this fast"
    );
}

#[cfg(unix)]
#[test]
fn a_child_that_ignores_sigterm_is_forced_after_the_grace_and_nothing_else_is() {
    use std::io::BufRead;
    use std::os::unix::process::ExitStatusExt;
    let mut stubborn = Reaped(spawned(
        std::process::Command::new("sh")
            .args(["-c", "trap '' TERM; echo ready; exec sleep 30"])
            .stdout(std::process::Stdio::piped()),
    ));
    let mut bystander = Reaped(spawned(std::process::Command::new("sleep").arg("30")));
    // SIGTERM is ignored from here on (an ignored signal stays ignored across exec).
    let mut line = String::new();
    io::BufReader::new(stubborn.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");
    let pid = stubborn.0.id();
    let started = start_time(pid).unwrap();
    let begun = std::time::Instant::now();
    terminate(pid, started).unwrap();
    assert!(begun.elapsed() >= Duration::from_secs(2), "after the grace");
    let ended = stubborn.0.wait().unwrap();
    assert_eq!(ended.signal(), Some(libc::SIGKILL));
    let other = bystander.0.id();
    assert!(is_alive(other), "only that child");
    assert!(bystander.0.try_wait().unwrap().is_none());
}
