//! Deleting a worker's session holds a bounded number of descriptors however deep the worker nested
//! its folders (std's `remove_dir_all` keeps one open per level). A child process (this test binary) lowers its own
//! open-files limit to 32, makes a 200-deep chain in a session's `tmp/` and deletes the session.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use klipolo_engine::make::session::Session;

const MARK: &str = "--klipolo-fds-child";
const LIMIT: libc::rlim_t = 32;

#[test]
fn fds_child() {
    let args: Vec<String> = std::env::args().collect();
    let Some(at) = args.iter().position(|a| a == MARK) else {
        return;
    };
    let root = Path::new(&args[at + 1]);
    let session = Session::create(root).unwrap();
    let mut deep = session.dir().join("tmp");
    for _ in 0..200 {
        deep.push("d");
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("f"), b"x").unwrap();
    let limit = libc::rlimit {
        rlim_cur: LIMIT,
        rlim_max: LIMIT,
    };
    // SAFETY: sets this (child) process's own limit from a value on the stack.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
    session.remove();
    std::process::exit(if session.dir().exists() { 1 } else { 0 });
}

#[test]
fn a_200_deep_session_is_deleted_within_32_descriptors() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fds_child",
            "--nocapture",
            "--test-threads=1",
            "--",
            MARK,
        ])
        .arg(&root)
        .status()
        .unwrap();
    assert!(status.success(), "{status:?}");
}
