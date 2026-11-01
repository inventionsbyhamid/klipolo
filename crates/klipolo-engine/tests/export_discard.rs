//! Discarding an unfinished export is all or nothing: its folder is first renamed, in the same `.jobs` folder, to a
//! name the job list ignores, and only then deleted, in the background. A rename that
//! fails leaves the job exactly as it was (so Discard or Start over is refused with nothing lost), and what a crash
//! left behind is swept at the next start.

use std::path::Path;
use std::time::{Duration, Instant};

use klipolo_engine::export::jobs::{discarded_name, list_jobs, tidy_staging};
use klipolo_engine::export::runner::discard_job_atomically;

const JOB: &str = "0a1b2c3d-4e5f";

/// A job folder with a few parts in it.
fn job_in(root: &Path) -> std::path::PathBuf {
    let dir = root.join(JOB);
    std::fs::create_dir_all(dir.join("landscape")).unwrap();
    std::fs::write(dir.join("job.json"), b"{}").unwrap();
    std::fs::write(dir.join("landscape/seg-00000.mp4"), b"part").unwrap();
    dir
}

#[test]
fn a_discarded_job_leaves_the_list_at_once_and_its_files_go_after() {
    let root = tempfile::tempdir().unwrap();
    let dir = job_in(root.path());
    discard_job_atomically(&dir).unwrap();
    assert!(!dir.exists(), "gone from the list at once");
    assert!(list_jobs(root.path()).is_empty());
    let started = Instant::now();
    while std::fs::read_dir(root.path()).unwrap().count() > 0 {
        assert!(started.elapsed() < Duration::from_secs(10), "its files go");
        std::thread::sleep(Duration::from_millis(10));
    }
    // A job already gone is fine (Try again after a swap).
    discard_job_atomically(&dir).unwrap();
}

#[test]
fn a_discard_whose_rename_fails_leaves_the_job_as_it_was() {
    let root = tempfile::tempdir().unwrap();
    let dir = job_in(root.path());
    // The name it would take is held by a folder with something in it: the rename can't happen.
    let taken = root.path().join(discarded_name(JOB));
    std::fs::create_dir_all(&taken).unwrap();
    std::fs::write(taken.join("x"), b"x").unwrap();
    assert!(discard_job_atomically(&dir).is_err());
    assert_eq!(
        std::fs::read(dir.join("landscape/seg-00000.mp4")).unwrap(),
        b"part",
        "every finished part kept"
    );
}

#[test]
fn a_discard_cut_off_by_a_crash_is_swept_at_the_next_start() {
    let root = tempfile::tempdir().unwrap();
    // A process that is gone renamed this one; this process is renaming the other right now.
    let dead = root.path().join(format!("{JOB}.{}.gone", u32::MAX));
    std::fs::create_dir_all(dead.join("landscape")).unwrap();
    let live = root.path().join(discarded_name("ffff-0000"));
    std::fs::create_dir_all(&live).unwrap();
    tidy_staging(root.path());
    assert!(!dead.exists(), "swept");
    assert!(live.exists(), "a live discard is never touched");
}

/// Only Klipolo's own cut-off folders go: a job id, then the id of a process that is gone. A folder of another name, or
/// one that names no process, stays, however old.
#[test]
fn the_sweep_takes_only_klipolos_own_cut_off_folders() {
    let root = tempfile::tempdir().unwrap();
    let folder = |name: String| {
        let path = root.path().join(name);
        std::fs::create_dir_all(path.join("landscape")).unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(3 * 3600);
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();
        path
    };
    let cut_off = folder(format!("{JOB}.{}.new", u32::MAX));
    let another_tool = folder(format!("renders.{}.new", u32::MAX));
    let no_process = folder(format!("{JOB}.gone"));
    let no_job = folder(format!("{}.gone", u32::MAX));
    tidy_staging(root.path());
    assert!(!cut_off.exists(), "Klipolo's own, its process gone");
    assert!(another_tool.exists() && no_process.exists() && no_job.exists());
}
