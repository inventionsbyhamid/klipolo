//! What an export leaves on the disk. An export that doesn't finish keeps its job and every finished part, so it can
//! resume from them: one that fails or pauses (a full disk, a disk gone, a busy encoder, a graphics processor that
//! stopped), one Quit pauses, and one a crash, a force quit or the Mac shutting down cut off. Only what can't resume
//! goes, and only Klipolo's own:
//! - Cancel discards its job all at once (the engine sets it aside and deletes it on its own thread); as the run ends
//!   the folder it set aside goes here too, then `.jobs` when it is empty (`finish_run`);
//! - a finished run's job is the engine's to delete; `.jobs` goes when it is empty;
//! - Discard deletes a kept job (`jobs::discard`);
//! - at the next start, and the next launch for the usual folder, the folders of job creations and discards a crash
//!   cut off (`<id>.<pid>.new`, `.gone`), once their process is gone (`jobs::tidy_staging`).
//!
//! Kept jobs are never swept, a crash's or one whose project changed since included: the export dialog lists them to
//! resume or discard.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use klipolo_engine::export::jobs::{discarded_name, jobs_root, tidy_staging};

use super::ExportState;

/// `dir` and everything in it, now; a folder already gone is fine. Tried a few times: a delete running beside it (the
/// engine's own, on its thread) can make one pass meet files that just went.
fn remove_now(dir: &Path) {
    for _ in 0..3 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => tracing::debug!(%e, "export: a job folder is still being deleted"),
        }
    }
    if dir.exists() {
        tracing::warn!("export: a job folder couldn't be deleted");
    }
}

/// `.jobs` beside `dir`'s job, if nothing is left in it (`remove_dir` refuses a folder that isn't empty).
fn remove_root_if_empty(root: &Path) {
    let _ = std::fs::remove_dir(root);
}

/// After a run that ended: a cancelled one (`cancelled`) has its job folder `dir` and the folder Cancel set aside in
/// this process (`discarded_name`) deleted now; any other keeps its job (finished, the engine has deleted it; paused or
/// failed, it resumes). Then `.jobs` goes if it is empty.
pub fn finish_run(dir: &Path, job: &str, cancelled: bool) {
    if cancelled {
        remove_now(dir);
        if let Some(root) = dir.parent() {
            remove_now(&root.join(discarded_name(job)));
        }
    }
    if let Some(root) = dir.parent() {
        remove_root_if_empty(root);
    }
}

/// Quitting while an export runs (the guard's Quit, or a quit nothing can hold, the Mac shutting down included): pauses
/// the run, so its job and finished parts are kept and it resumes next time (`ExportState::pause_for_quit`), and waits up
/// to `wait` for it to stop at a part and let go of the slot. Past that the app goes anyway: its worker stops with it,
/// and the job lists as stopped, its finished parts intact. Blocking: it runs on the thread that quits.
pub fn pause_on_quit(state: &ExportState, wait: Duration) {
    if state.pause_for_quit().is_none() {
        return;
    }
    let until = Instant::now() + wait;
    while state.is_running() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Deletes the folders of job creations and discards a crash cut off in `dir`'s `.jobs` (`tidy_staging`); every job
/// stays. Blocking. Files in place in `dir` are never touched.
pub fn tidy_jobs(dir: &Path) {
    tidy_staging(&jobs_root(dir));
}

/// At launch, for the usual export folder: `tidy_jobs`, then `.jobs` itself if nothing is left in it.
pub fn sweep_folder(dir: &Path) {
    tidy_jobs(dir);
    remove_root_if_empty(&jobs_root(dir));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use klipolo_engine::export::job::JOB_FILE;

    use super::*;

    const OLD: Duration = Duration::from_secs(3600);

    /// A job folder `id` in `dir`'s `.jobs` with a part and a manifest, its files' times `old` in the past.
    fn job(dir: &Path, id: &str, old: Duration) -> PathBuf {
        let job = jobs_root(dir).join(id);
        std::fs::create_dir_all(job.join("landscape")).unwrap();
        std::fs::write(job.join("landscape").join("0.mp4"), b"part").unwrap();
        std::fs::write(job.join(JOB_FILE), b"{}").unwrap();
        let then = SystemTime::now() - old;
        for path in [job.join("landscape").join("0.mp4"), job.join(JOB_FILE)] {
            std::fs::File::open(&path)
                .unwrap()
                .set_modified(then)
                .unwrap();
        }
        job
    }

    /// A job a crash or a shutdown left, however old, stays at the next start and launch with every part: it is
    /// offered to resume. What a crash cut off while a job was being written or discarded goes once its process is
    /// gone; a live process's never does. The videos in the folder are never touched.
    #[test]
    fn a_crashs_job_stays_to_resume_and_only_cut_off_writes_go() {
        let dir = tempfile::tempdir().unwrap();
        let left = job(dir.path(), "0a1b2c3d-4e5f-6789-abcd-ef0123456789", OLD);
        let creating = job(
            dir.path(),
            &format!("2c3d4e5f.{}.new", std::process::id()),
            OLD,
        );
        let cut_off = job(dir.path(), "3d4e5f6a.999999999.new", Duration::ZERO);
        let discard_cut_off = job(dir.path(), "4e5f6a7b.999999999.gone", Duration::ZERO);
        let foreign = job(dir.path(), "renders", OLD);
        std::fs::write(dir.path().join("Demo 16x9.mp4"), b"a video").unwrap();
        sweep_folder(dir.path());
        assert!(left.join("landscape").join("0.mp4").is_file(), "kept whole");
        assert!(creating.exists() && foreign.exists());
        assert!(!cut_off.exists() && !discard_cut_off.exists());
        assert!(dir.path().join("Demo 16x9.mp4").is_file());
        tidy_jobs(dir.path());
        assert!(left.exists(), "a start keeps it too");
        // Nothing left in `.jobs`: it goes at launch; a folder that isn't there is fine.
        let empty = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(jobs_root(empty.path())).unwrap();
        sweep_folder(empty.path());
        assert!(!jobs_root(empty.path()).exists());
        sweep_folder(&dir.path().join("nowhere"));
    }

    /// A run that ends without its files keeps its job to resume; a cancelled one takes its job and the folder Cancel
    /// set aside with it, then the empty `.jobs`.
    #[test]
    fn a_run_that_stops_keeps_its_job_and_a_cancelled_one_takes_it() {
        let dir = tempfile::tempdir().unwrap();
        let id = "4e5f6a7b";
        let stopped = job(dir.path(), id, Duration::ZERO);
        finish_run(&stopped, id, false);
        assert!(
            stopped.join("landscape").join("0.mp4").is_file(),
            "kept to resume"
        );
        // Cancel set the job aside (this process's name for it) and is deleting it on its own thread.
        let aside = job(dir.path(), &discarded_name(id), Duration::ZERO);
        std::fs::remove_dir_all(&stopped).unwrap();
        finish_run(&stopped, id, true);
        assert!(!aside.exists());
        assert!(!jobs_root(dir.path()).exists());
        // A finished run's job is the engine's to delete; `.jobs` stays while something else is in it.
        let other = job(dir.path(), "5f6a7b8c", OLD);
        let done = jobs_root(dir.path()).join("6a7b8c9d");
        finish_run(&done, "6a7b8c9d", false);
        assert!(other.exists());
    }
}
