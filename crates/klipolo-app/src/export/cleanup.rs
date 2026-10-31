//! Nothing half made outlives an export. Klipolo doesn't resume one, so every job folder that won't finish goes, and
//! only Klipolo's own:
//! - a run that ends without its files deletes its job (and the folder a Cancel set aside) on the spot, then the
//!   `.jobs` folder when it is empty (`finish_run`);
//! - quitting while one runs stops it and deletes its folder on the thread that quits, before the process can end
//!   (`clear_on_quit`): a background delete could be cut off;
//! - a crash or a force quit leaves one behind: the next start, and the next launch for the usual folder, sweep it
//!   (`tidy_jobs`, `sweep_folder`).
//!
//! What counts as Klipolo's leftover job: a folder in `.jobs` named by a job id, holding a `job.json`, that hasn't
//! changed for 10 minutes (a running worker writes its parts and manifest all the time, so another instance's live job
//! is never one). Anything else there (another tool's folder, a file) is left alone, and so are the folders of job
//! creations and discards in progress (`<id>.<pid>.new`, `.gone`): `jobs::tidy_staging` removes those once their
//! process is gone.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use klipolo_engine::export::job::JOB_FILE;
use klipolo_engine::export::jobs::{discarded_name, jobs_root, tidy_staging, valid_job_id};

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

/// Job `job` in `dir` (a job folder), and the folder a Cancel of it set aside in this process (`discarded_name`),
/// deleted now.
fn remove_job(dir: &Path, job: Option<&str>) {
    remove_now(dir);
    if let (Some(root), Some(job)) = (dir.parent(), job) {
        remove_now(&root.join(discarded_name(job)));
    }
}

/// `.jobs` beside `dir`'s job, if nothing is left in it (`remove_dir` refuses a folder that isn't empty).
fn remove_root_if_empty(root: &Path) {
    let _ = std::fs::remove_dir(root);
}

/// After a run that ended (`finished`: its files are in place, and the engine has deleted its job): a run that didn't
/// finish has its job folder `dir` and the folder a Cancel set aside deleted now; then `.jobs` goes if it is empty.
pub fn finish_run(dir: &Path, job: &str, finished: bool) {
    if !finished {
        remove_job(dir, Some(job));
    }
    if let Some(root) = dir.parent() {
        remove_root_if_empty(root);
    }
}

/// Quitting while an export runs (the guard's Quit, or a quit nothing can hold): stops the run, deletes its job folder
/// at once, waits up to `wait` for the run to let go of the slot, and deletes whatever its worker wrote meanwhile.
/// Blocking: it runs on the thread that quits, so the process can't end before the files are gone.
pub fn clear_on_quit(state: &ExportState, wait: Duration) {
    let Some(stopped) = state.stop_for_quit() else {
        return;
    };
    let job = stopped.job.as_deref();
    if let Some(dir) = &stopped.dir {
        remove_job(dir, job);
    }
    let until = Instant::now() + wait;
    while state.is_running() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(dir) = &stopped.dir {
        remove_job(dir, job);
        if let Some(root) = dir.parent() {
            remove_root_if_empty(root);
        }
    }
}

/// The newest change in a job folder: its own, its files' and those of its format folders (where the worker writes
/// its parts).
fn last_change(dir: &Path) -> Option<SystemTime> {
    let mut newest = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
    let mut seen = |m: Option<SystemTime>| {
        if m > newest {
            newest = m;
        }
    };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let meta = entry.metadata().ok();
        seen(meta.as_ref().and_then(|m| m.modified().ok()));
        if meta.is_some_and(|m| m.is_dir()) {
            for inner in std::fs::read_dir(entry.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                seen(inner.metadata().and_then(|m| m.modified()).ok());
            }
        }
    }
    newest
}

/// Whether `entry` (in `.jobs`) is a job of Klipolo's that no one is running any more (the module doc).
fn leftover_job(path: &Path) -> bool {
    let named = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(valid_job_id);
    named
        && path.is_dir()
        && path.join(JOB_FILE).is_file()
        && klipolo_platform::process::is_leftover(None, last_change(path))
}

/// Deletes the jobs runs that never ended left in `dir`'s `.jobs` (the module doc says which), and the folders of job
/// creations and discards a crash cut off (`tidy_staging`). Blocking. Files in place in `dir` are never touched.
pub fn tidy_jobs(dir: &Path) {
    let root = jobs_root(dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if leftover_job(&path) {
            tracing::info!("export: deleting a job an earlier run left");
            remove_now(&path);
        }
    }
    tidy_staging(&root);
}

/// At launch, for the usual export folder: `tidy_jobs`, then `.jobs` itself if nothing is left in it.
pub fn sweep_folder(dir: &Path) {
    tidy_jobs(dir);
    remove_root_if_empty(&jobs_root(dir));
}

/// The job folder `dir` holds for a test, its files' times moved `age` into the past.
#[cfg(test)]
pub(super) fn age(dir: &Path, age: Duration) {
    let then = SystemTime::now() - age;
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    paths.push(dir.to_path_buf());
    for path in paths {
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(then)
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const OLD: Duration = Duration::from_secs(3600);

    /// A job folder `id` in `dir`'s `.jobs`, with a manifest when `manifest`, `age` old.
    fn job(dir: &Path, id: &str, manifest: bool, old: Duration) -> PathBuf {
        let job = jobs_root(dir).join(id);
        std::fs::create_dir_all(job.join("landscape")).unwrap();
        std::fs::write(job.join("landscape").join("0.mp4"), b"part").unwrap();
        if manifest {
            std::fs::write(job.join(JOB_FILE), b"{}").unwrap();
        }
        age(&job.join("landscape"), old);
        age(&job, old);
        job
    }

    /// A job an earlier run left (the app crashed mid-export) goes at the next start or launch; the videos in the
    /// folder stay, and `.jobs` goes once it is empty.
    #[test]
    fn a_job_an_earlier_run_left_goes() {
        let dir = tempfile::tempdir().unwrap();
        let left = job(
            dir.path(),
            "0a1b2c3d-4e5f-6789-abcd-ef0123456789",
            true,
            OLD,
        );
        std::fs::write(dir.path().join("Demo 16x9.mp4"), b"a video").unwrap();
        sweep_folder(dir.path());
        assert!(!left.exists());
        assert!(!jobs_root(dir.path()).exists(), "the empty .jobs goes too");
        assert!(dir.path().join("Demo 16x9.mp4").is_file());
        sweep_folder(&dir.path().join("nowhere"));
    }

    /// Only Klipolo's leftovers: another tool's folder in `.jobs`, a folder without a manifest, a job something wrote
    /// to in the last minutes (another instance's live export) and a job being created by a live process all stay.
    #[test]
    fn only_klipolos_leftover_jobs_go() {
        let dir = tempfile::tempdir().unwrap();
        let foreign = job(dir.path(), "renders", true, OLD);
        let no_manifest = job(dir.path(), "0a1b2c3d", false, OLD);
        let live = job(dir.path(), "1b2c3d4e", true, Duration::ZERO);
        let creating = job(
            dir.path(),
            &format!("2c3d4e5f.{}.new", std::process::id()),
            true,
            OLD,
        );
        let stray = jobs_root(dir.path()).join("notes.txt");
        std::fs::write(&stray, b"mine").unwrap();
        sweep_folder(dir.path());
        for kept in [&foreign, &no_manifest, &live, &creating, &stray] {
            assert!(kept.exists(), "{} stays", kept.display());
        }
        // A creation whose process is gone is the staging sweep's.
        let cut_off = job(dir.path(), "3d4e5f6a.999999999.new", true, OLD);
        tidy_jobs(dir.path());
        assert!(!cut_off.exists());
        assert!(live.exists() && creating.exists());
    }

    #[test]
    fn a_run_that_ends_takes_its_job_and_the_empty_jobs_folder_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let id = "4e5f6a7b";
        let running = job(dir.path(), id, true, Duration::ZERO);
        // Cancel set the job aside (this process's name for it) and is deleting it on its own thread.
        let aside = job(dir.path(), &discarded_name(id), true, Duration::ZERO);
        finish_run(&running, id, false);
        assert!(!running.exists() && !aside.exists());
        assert!(!jobs_root(dir.path()).exists());
        // A finished run's job is the engine's to delete; `.jobs` stays while something else is in it.
        let other = job(dir.path(), "5f6a7b8c", true, OLD);
        let done = jobs_root(dir.path()).join("6a7b8c9d");
        finish_run(&done, "6a7b8c9d", true);
        assert!(other.exists());
    }
}
