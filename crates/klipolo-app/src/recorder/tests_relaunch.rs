//! Reopen Klipolo's relaunch: the waiter opens the bundle by its path once this Klipolo has gone (the binary outside a
//! bundle), and only an exit asked with the reopen's code that goes ahead starts it.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use super::relaunch::{AFTER_EXIT, REOPEN_EXIT_CODE, Relaunch, waiter};

fn strs(args: &[std::ffi::OsString]) -> Vec<&str> {
    args.iter().map(|a| a.to_str().unwrap()).collect()
}

#[test]
fn the_relaunch_opens_the_bundle_by_its_path_once_this_klipolo_has_gone() {
    let exe = Path::new("/Applications/Klipolo.app/Contents/MacOS/klipolo-app");
    let (program, args) = waiter(exe, 4242);
    assert_eq!(program, Path::new("/bin/sh"));
    assert_eq!(
        strs(&args),
        [
            "-c",
            AFTER_EXIT,
            "4242",
            "/usr/bin/open",
            "-n",
            "/Applications/Klipolo.app"
        ]
    );
}

#[test]
fn outside_a_bundle_the_relaunch_runs_the_binary() {
    let exe = Path::new("/src/target/debug/klipolo-app");
    let (program, args) = waiter(exe, 7);
    assert_eq!(program, Path::new("/bin/sh"));
    assert_eq!(
        strs(&args),
        ["-c", AFTER_EXIT, "7", "/src/target/debug/klipolo-app"]
    );
}

/// The script itself, with a real shell: it runs its command only once the process it waits for has ended, and not
/// after its 10 s cap.
#[test]
fn the_waiter_runs_its_command_only_once_the_process_has_gone() {
    let mut klipolo = Command::new("/bin/sleep").arg("0.4").spawn().unwrap();
    let pid = klipolo.id();
    // Reaped as soon as it ends, as launchd does for an app: a zombie still answers `kill -0`.
    let reaped = std::thread::spawn(move || klipolo.wait());
    let started = Instant::now();
    let status = Command::new("/bin/sh")
        .args([
            "-c",
            AFTER_EXIT,
            &pid.to_string(),
            "/bin/sh",
            "-c",
            "exit 7",
        ])
        .status()
        .unwrap();
    let took = started.elapsed();
    reaped.join().unwrap().unwrap();
    assert_eq!(status.code(), Some(7), "the command ran");
    assert!(took >= Duration::from_millis(350), "it waited: {took:?}");
    assert!(took < Duration::from_secs(5), "not the cap: {took:?}");
}

#[test]
fn only_an_exit_asked_by_reopen_relaunches_and_only_once() {
    let relaunch = Relaunch::default();
    // Reopen's exit, held by a quit guard (an export started meanwhile; the person kept exporting). Later they quit
    // with Command Q, or close the window: Klipolo doesn't come back.
    relaunch.exit_requested(Some(REOPEN_EXIT_CODE));
    relaunch.exit_requested(Some(0));
    assert!(!relaunch.due());
    relaunch.exit_requested(Some(REOPEN_EXIT_CODE));
    relaunch.exit_requested(None);
    assert!(!relaunch.due());
    // Held, then let go with its own code (the export's Quit): it relaunches, once.
    relaunch.exit_requested(Some(REOPEN_EXIT_CODE));
    relaunch.exit_requested(Some(REOPEN_EXIT_CODE));
    assert!(relaunch.due());
    assert!(!relaunch.due(), "once");
    // An ordinary quit never relaunches.
    relaunch.exit_requested(Some(0));
    assert!(!relaunch.due());
}
