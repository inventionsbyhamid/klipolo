//! Unfinished exports: the jobs a run kept when it stopped (a failure, a full disk, a pause, Quit, a crash or the Mac
//! shutting down), listed at the top of the export dialog to resume from their finished parts or discard. A job always
//! resumes into the video that was exported: it renders its own frozen copy of the project, never a mix with what
//! changed since. When its project changed (`stale`, or the project is gone), the dialog says so and also offers Start
//! over with the latest changes: an export naming the job it replaces (`start`), which discards it only once the new
//! export is ready to run.
//!
//! A job is kept in `.jobs` beside its files: in the usual folder (`Movies / Klipolo`), the one the person picked for
//! exports, and every folder an export was written in before (remembered across launches: `ExportState::kept_folders`),
//! so all of them are looked in (`roots`).

use std::path::{Path, PathBuf};

use klipolo_engine::api::{ExportJobInfo, ExportJobState, ExportQuality};
use klipolo_engine::export::job::{ExportJob, JOB_FILE};
use klipolo_engine::export::jobs::{UNREADABLE, folder_bytes, jobs_root, list_jobs, valid_job_id};
use klipolo_engine::export::runner::{self, discard_job_atomically};
use klipolo_model::coded::Said;

use super::run::coded;

/// An id that isn't a job's.
const INVALID: &str = "exports.refused.invalid";

/// The `.jobs` folders an unfinished job may be in: the picked export folder's, the usual one's, then those of the
/// folders exports were written in before (`kept`), each once.
pub fn roots(picked: Option<&Path>, usual: Option<&Path>, kept: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let earlier = kept.iter().map(PathBuf::as_path);
    for dir in [picked, usual].into_iter().flatten().chain(earlier) {
        let root = jobs_root(dir);
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// The folder of job `job` (an id from `export_jobs` or `ExportEvent::Started`) in whichever of `roots` holds it; None
/// when none does (discarded meanwhile). An id that isn't a job's is refused before it becomes a path.
pub fn find(roots: &[PathBuf], job: &str) -> Result<Option<PathBuf>, Said> {
    if !valid_job_id(job) {
        return Err(Said::code(INVALID));
    }
    Ok(roots
        .iter()
        .map(|root| root.join(job))
        .find(|dir| dir.is_dir()))
}

/// Unfinished exports in `roots`, newest first, with their progress, whether a run of this app is on them, and whether
/// their project changed since (`stale`: the project as it is now no longer makes what the job renders, or it's gone).
/// Why one stopped is said as a code where the app has one (`run::coded`). One that can't be read can only be
/// discarded: it has no project name, and the reason goes to the log.
pub fn list(roots: &[PathBuf], mut stale: impl FnMut(&ExportJob) -> bool) -> Vec<ExportJobInfo> {
    let mut jobs: Vec<_> = roots
        .iter()
        .flat_map(|root| list_jobs(root).into_iter().map(move |job| (root, job)))
        .collect();
    jobs.sort_by_key(|(_, j)| std::cmp::Reverse(j.as_ref().map_or(0, |j| j.created_ms)));
    jobs.into_iter()
        .map(|(root, job)| match job {
            Ok(job) => {
                let running = runner::is_running(&job.job_id);
                let mut info = job.info(running, !running && stale(&job));
                info.message = info.message.map(coded);
                info
            }
            Err((job_id, reason)) => {
                tracing::warn!(job = %job_id, %reason, "export: an unfinished job can't be read");
                let bytes = folder_bytes(&root.join(&job_id));
                unreadable(job_id, bytes)
            }
        })
        .collect()
}

/// A job that can't be read (its folder taking `bytes`), as the list shows it: only Discard.
fn unreadable(job_id: String, bytes: u64) -> ExportJobInfo {
    ExportJobInfo {
        job_id,
        project_id: String::new(),
        project_name: String::new(),
        quality: ExportQuality::default(),
        files: Vec::new(),
        state: ExportJobState::Failed,
        message: Some(coded(UNREADABLE.into())),
        frame: 0,
        total_frames: 0,
        segments_done: 0,
        segments_total: 0,
        stale: true,
        created_ms: 0,
        bytes,
        other_build: false,
        compatibility: None,
    }
}

/// Deletes the unfinished job `job` (Discard, and Start over once its new job is written). All or nothing: its folder
/// is set aside at once and deleted in the background (`runner::discard_job_atomically`), so a failure leaves it as it
/// was (`exports.discardFailed`). A running one is stopped first and deletes its folder as it stops. One already gone
/// is fine.
pub fn discard(roots: &[PathBuf], job: &str) -> Result<(), Said> {
    let dir = find(roots, job)?;
    if runner::cancel(job) {
        return Ok(());
    }
    let Some(dir) = dir else {
        return Ok(());
    };
    discard_job_atomically(&dir).map_err(|error| {
        tracing::warn!(%job, %error, "export: an unfinished job couldn't be discarded");
        Said::code("exports.discardFailed")
    })
}

/// Whose a job Start over names is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The job of this project (by id).
    Project(String),
    /// No such job any more (discarded meanwhile): nothing to replace.
    Gone,
    /// Its manifest can't be read: never discarded on a guess.
    Unreadable,
}

/// Whose the unfinished job `job` is, from its manifest alone (Start over replaces only a job of the project it
/// exports).
pub fn owner(roots: &[PathBuf], job: &str) -> Owner {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Manifest {
        project_id: String,
    }
    let Ok(found) = find(roots, job) else {
        return Owner::Unreadable;
    };
    let Some(dir) = found else {
        return Owner::Gone;
    };
    std::fs::read(dir.join(JOB_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
        .map_or(Owner::Unreadable, |m| Owner::Project(m.project_id))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use klipolo_engine::api::{ExportEvent, ExportRequest, ResumeMode};
    use klipolo_engine::export::job::JobState;
    use klipolo_engine::export::runner::{Attempt, Launcher, Outcome};
    use klipolo_model::project::{FormatKind, Project};
    use tokio::sync::watch;

    use super::*;

    /// A worker that finishes at once (its files joined, done), keeping the project of each job it ran.
    #[derive(Default)]
    struct Finishes {
        drew: Mutex<Vec<Project>>,
    }

    impl Launcher for Finishes {
        fn launch(&self, manifest: &Path) -> Result<Attempt, String> {
            let job = ExportJob::open(manifest.parent().unwrap())?;
            self.drew.lock().unwrap().push(job.project.clone());
            let (tx, lines) = tokio::sync::mpsc::unbounded_channel();
            for o in &job.outputs {
                std::fs::write(job.path(&o.file), b"video").unwrap();
                tx.send(
                    serde_json::json!({
                        "type": "formatDone", "format": o.format, "tempPath": job.path(&o.file), "bytes": 5,
                    })
                    .to_string(),
                )
                .unwrap();
            }
            tx.send(r#"{"type":"done","elapsedS":1.0}"#.into()).unwrap();
            drop(tx);
            let (exit_tx, exit) = tokio::sync::oneshot::channel();
            exit_tx.send(true).unwrap();
            Ok(Attempt {
                lines,
                exit,
                kill: Box::new(|| {}),
                pause: Box::new(|| {}),
            })
        }
    }

    /// The 60 s test project, and its job for 16:9 written in `dir`, as an export leaves it when the app crashes
    /// mid-run (its manifest still says it runs, and nothing runs it).
    async fn crashed_job(dir: &Path) -> (Project, ExportJob) {
        let (project, speech) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        let req = ExportRequest {
            formats: vec![FormatKind::Landscape],
            quality: ExportQuality::High,
            caption_file: None,
            allow_stand_ins: false,
            replaces: None,
            allow_missing_cursor: false,
        };
        let batch = crate::export::freeze(
            Arc::new(project.clone()),
            &speech,
            "en".into(),
            &req,
            dir,
            None,
        );
        let job = crate::export::create_job(batch).await.unwrap();
        assert_eq!(job.state, JobState::Running);
        (project, job)
    }

    fn roots_of(dir: &Path) -> Vec<PathBuf> {
        roots(None, Some(dir), &[])
    }

    #[test]
    fn jobs_are_looked_for_in_the_picked_folder_the_usual_one_and_those_used_before() {
        let usual = Path::new("/Users/rosa/Movies/Klipolo");
        let picked = Path::new("/Volumes/Card/Exports");
        let earlier = PathBuf::from("/Volumes/Old/Exports");
        assert_eq!(
            roots(
                Some(picked),
                Some(usual),
                &[earlier.clone(), picked.to_path_buf()]
            ),
            [
                picked.join(".jobs"),
                usual.join(".jobs"),
                earlier.join(".jobs")
            ]
        );
        assert_eq!(roots(Some(usual), Some(usual), &[]), [usual.join(".jobs")]);
        assert_eq!(roots(None, None, &[]), Vec::<PathBuf>::new());
        assert_eq!(
            find(&roots(None, Some(usual), &[]), "../elsewhere"),
            Err(Said::code(INVALID)),
            "never a path out of .jobs"
        );
    }

    /// A crash (or a shutdown) mid-export leaves the job saying it runs: at the next launch it is listed as stopped,
    /// with its progress and its project, current while the project is as it was, and Resume finishes it.
    #[tokio::test]
    async fn a_crashs_job_is_offered_at_the_next_launch_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let (project, job) = crashed_job(dir.path()).await;
        crate::export::sweep_folder(dir.path());
        let listed = list(&roots_of(dir.path()), |job| {
            !job.is_current_as_saved(&project)
        });
        assert_eq!(listed.len(), 1);
        let info = &listed[0];
        assert_eq!(info.job_id, job.job_id);
        assert_eq!(info.state, ExportJobState::Interrupted);
        assert_eq!(
            (info.project_id.as_str(), info.project_name.as_str()),
            (project.id.as_str(), project.name.as_str())
        );
        assert_eq!((info.segments_done, info.segments_total), (0, 2));
        assert!(!info.stale, "its project is as it was");
        let dir_of = find(&roots_of(dir.path()), &job.job_id)
            .unwrap()
            .expect("found");
        let finishes = Finishes::default();
        let outcome = crate::export::resume(
            &finishes,
            dir_of,
            ResumeMode::Auto,
            watch::channel(false).1,
            &|_: ExportEvent| {},
        )
        .await;
        assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
        assert!(list(&roots_of(dir.path()), |_| false).is_empty(), "done");
    }

    /// A job whose project changed since is listed as changed (Start over is offered beside it), and when resumed it
    /// renders the project it froze, never the changed one: the file is the version that was exported. A project
    /// that's gone counts as changed too.
    #[tokio::test]
    async fn a_job_whose_project_changed_never_resumes_into_a_mixed_file() {
        let dir = tempfile::tempdir().unwrap();
        let (project, job) = crashed_job(dir.path()).await;
        let mut changed = project.clone();
        changed.timeline.main[0].speed = 2.0;
        let listed = list(&roots_of(dir.path()), |job| {
            !job.is_current_as_saved(&changed)
        });
        assert!(listed[0].stale, "the project changed since");
        let gone = list(&roots_of(dir.path()), |_| true);
        assert!(gone[0].stale);
        let finishes = Finishes::default();
        let dir_of = find(&roots_of(dir.path()), &job.job_id).unwrap().unwrap();
        let outcome = crate::export::resume(
            &finishes,
            dir_of,
            ResumeMode::Auto,
            watch::channel(false).1,
            &|_: ExportEvent| {},
        )
        .await;
        assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
        assert_eq!(
            *finishes.drew.lock().unwrap(),
            std::slice::from_ref(&job.project),
            "the frozen project"
        );
        assert_ne!(job.project.timeline, changed.timeline);
    }

    /// Discard deletes a kept job and its parts all at once (it leaves the list at once); one already gone is fine; an
    /// id that isn't a job's is refused.
    #[tokio::test]
    async fn discard_deletes_a_kept_job() {
        let dir = tempfile::tempdir().unwrap();
        let (_, job) = crashed_job(dir.path()).await;
        let roots = roots_of(dir.path());
        assert_eq!(
            owner(&roots, &job.job_id),
            Owner::Project(job.project_id.clone())
        );
        discard(&roots, &job.job_id).unwrap();
        assert!(!job.dir.exists());
        assert!(list(&roots, |_| false).is_empty());
        assert_eq!(owner(&roots, &job.job_id), Owner::Gone);
        let started = Instant::now();
        while std::fs::read_dir(jobs_root(dir.path())).unwrap().count() > 0 {
            assert!(started.elapsed() < Duration::from_secs(10), "its files go");
            std::thread::sleep(Duration::from_millis(10));
        }
        discard(&roots, &job.job_id).unwrap();
        assert_eq!(discard(&roots, "../x"), Err(Said::code(INVALID)));
    }

    /// A job that can't be read is listed, with no project name, said as unreadable: only Discard. Start over never
    /// replaces it.
    #[test]
    fn an_unreadable_job_can_only_be_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let id = "0b9e6f3c-6d2a-4f3e-9c1b-2a7d5e8f9a10";
        let damaged = jobs_root(dir.path()).join(id);
        std::fs::create_dir_all(&damaged).unwrap();
        std::fs::write(damaged.join(JOB_FILE), b"{ not a job").unwrap();
        let roots = roots_of(dir.path());
        let listed = list(&roots, |_| false);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].job_id, id);
        assert_eq!(listed[0].project_name, "");
        assert_eq!(listed[0].message, Some(Said::code("exports.unreadable")));
        assert_eq!(owner(&roots, id), Owner::Unreadable);
        discard(&roots, id).unwrap();
        assert!(!damaged.exists());
    }

    /// A row says how much disk its kept parts use, an unreadable job's too: Discard frees it.
    #[tokio::test]
    async fn a_row_says_how_much_disk_its_parts_use() {
        let dir = tempfile::tempdir().unwrap();
        let (_, job) = crashed_job(dir.path()).await;
        let empty = list(&roots_of(dir.path()), |_| false)[0].bytes;
        let part = job.dir.join(&job.outputs[0].segments[0].file);
        std::fs::write(&part, vec![7u8; 4096]).unwrap();
        assert_eq!(
            list(&roots_of(dir.path()), |_| false)[0].bytes,
            empty + 4096
        );
        std::fs::write(job.dir.join(JOB_FILE), b"{ damaged").unwrap();
        let unreadable = &list(&roots_of(dir.path()), |_| false)[0];
        assert_eq!(unreadable.project_name, "");
        assert!(unreadable.bytes >= 4096, "{}", unreadable.bytes);
    }

    /// A job another build of Klipolo made can't resume (its file would mix parts drawn by two builds): its row says
    /// so, and it is offered Start over and Discard only.
    #[tokio::test]
    async fn a_job_another_build_made_is_listed_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let (_, job) = crashed_job(dir.path()).await;
        assert!(!list(&roots_of(dir.path()), |_| false)[0].other_build);
        let mut older = ExportJob::open(&job.dir).unwrap();
        older.build = Some("an earlier build".into());
        older.save().unwrap();
        assert!(list(&roots_of(dir.path()), |_| false)[0].other_build);
    }

    /// Jobs in the picked folder and the usual one are listed together, newest first.
    #[tokio::test]
    async fn jobs_of_both_folders_are_listed_newest_first() {
        let (usual, picked) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (_, older) = crashed_job(usual.path()).await;
        std::thread::sleep(Duration::from_millis(5));
        let (_, newer) = crashed_job(picked.path()).await;
        let listed = list(&roots(Some(picked.path()), Some(usual.path()), &[]), |_| {
            false
        });
        let ids: Vec<&str> = listed.iter().map(|j| j.job_id.as_str()).collect();
        assert_eq!(ids, [newer.job_id.as_str(), older.job_id.as_str()]);
    }
}
