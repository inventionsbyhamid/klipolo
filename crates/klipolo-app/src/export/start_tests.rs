//! A start never leaves anything behind on the way: a start refused or stopped before its job runs writes nothing (or
//! deletes the job it wrote) and lets go of the slot; a start that runs holds the slot with its job on disk.

use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};

use klipolo_engine::api::ExportRequest;
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::runner::Batch;
use klipolo_model::coded::Said;
use klipolo_model::project::FormatKind;

use super::ExportState;
use super::start::{StartSteps, Started, create_job, start_on_slot};

/// When the fake presses Cancel.
#[derive(Clone, Copy, PartialEq)]
enum Stop {
    /// While the latest changes are saved (before the job is made).
    Saving,
    /// While the job is being written.
    Creating,
}

/// The app's side of a start, faked: what preparing answers, whether making the job fails, and what was done, in
/// order.
struct Steps<'a> {
    prepared: Mutex<Option<Result<Batch, Said>>>,
    create_fails: bool,
    stop: Option<(Stop, &'a ExportState)>,
    done: Mutex<Vec<String>>,
}

impl Steps<'_> {
    fn preparing(answer: Result<Batch, Said>) -> Self {
        Self {
            prepared: Mutex::new(Some(answer)),
            create_fails: false,
            stop: None,
            done: Mutex::new(Vec::new()),
        }
    }

    fn done(&self) -> Vec<String> {
        self.done.lock().unwrap().clone()
    }

    fn note(&self, what: &str) {
        self.done.lock().unwrap().push(what.into());
    }

    /// Cancel from the open project `open`, which the export is of (its id is recorded as the slot is taken, then as
    /// the batch is prepared).
    fn press_stop(&self, at: Stop, open: &str) {
        if let Some((when, state)) = self.stop
            && when == at
        {
            state.cancel_for(None, Some(open)).unwrap();
        }
    }
}

impl StartSteps for Steps<'_> {
    fn open(&self) -> Option<(String, String)> {
        Some(("project".into(), "Billing tutorial".into()))
    }

    fn prepare(&self, _req: &ExportRequest) -> impl Future<Output = Result<Batch, Said>> + Send {
        self.press_stop(Stop::Saving, "project");
        let answer = self.prepared.lock().unwrap().take().expect("prepared once");
        async move { answer }
    }

    fn create(&self, batch: Batch) -> impl Future<Output = Result<ExportJob, Said>> + Send {
        self.note("create");
        self.press_stop(Stop::Creating, &batch.project.id);
        let fails = self.create_fails;
        async move {
            if fails {
                Err(Said::code("exports.notStarted"))
            } else {
                create_job(batch).await
            }
        }
    }

    fn drop_new(&self, job: &ExportJob) {
        self.note("drop new");
        klipolo_engine::export::runner::discard_job(&job.dir).unwrap();
    }
}

/// The project's 9:16 file.
fn portrait() -> ExportRequest {
    ExportRequest {
        formats: vec![FormatKind::Portrait],
        quality: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        replaces: None,
        allow_missing_cursor: false,
    }
}

/// A 60 s project's batch, as preparing freezes it, its files planned in a folder of the test's.
fn batch() -> (Batch, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let (project, speech) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
    let exports = dir.path().join("exports");
    let batch = super::freeze(
        Arc::new(project),
        &speech,
        "en".into(),
        &portrait(),
        &exports,
        None,
    );
    (batch, dir)
}

/// The jobs (and job folders being written or removed) in `dir`'s exports folder.
fn jobs(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("exports").join(".jobs")).map_or(0, |d| d.count())
}

#[tokio::test]
async fn a_start_refused_because_another_export_runs_writes_nothing() {
    let state = ExportState::default();
    // Another export holds the slot: one a chat started, or a run from before a reload.
    let _other = state.reserve().unwrap();
    let (prepared, dir) = batch();
    let steps = Steps::preparing(Ok(prepared));
    let refused = start_on_slot(&state, &portrait(), &steps)
        .await
        .err()
        .expect("refused");
    assert_eq!(refused, Said::code("exports.refused.busy"));
    assert!(steps.done().is_empty(), "nothing made");
    assert_eq!(jobs(dir.path()), 0);
    assert!(state.is_running(), "the other export still holds the slot");
}

#[tokio::test]
async fn a_start_whose_latest_changes_cant_be_saved_lets_go_of_the_slot() {
    let state = ExportState::default();
    let steps = Steps::preparing(Err(Said::code("exports.unsaved")));
    let refused = start_on_slot(&state, &portrait(), &steps)
        .await
        .err()
        .expect("refused");
    assert_eq!(refused, Said::code("exports.unsaved"));
    assert!(steps.done().is_empty(), "nothing made");
    assert!(!state.is_running(), "the slot is free again");
}

/// A write error in `.jobs` (a full disk, the volume gone) while the job is made.
#[tokio::test]
async fn a_start_whose_job_cant_be_written_lets_go_of_the_slot() {
    let state = ExportState::default();
    let (prepared, dir) = batch();
    let mut steps = Steps::preparing(Ok(prepared));
    steps.create_fails = true;
    let refused = start_on_slot(&state, &portrait(), &steps)
        .await
        .err()
        .expect("refused");
    assert_eq!(refused, Said::code("exports.notStarted"));
    assert_eq!(steps.done(), ["create"]);
    assert_eq!(jobs(dir.path()), 0);
    assert!(!state.is_running(), "the slot is free again");
}

/// Cancel pressed while the changes are saved, or while the job is written: nothing is left (a job already written
/// goes), and the slot is free again. A Cancel that names no job reaches it from its own project at once: whose export
/// it is is known from the moment it holds the slot.
#[tokio::test]
async fn a_start_stopped_before_its_job_runs_leaves_nothing() {
    for (at, done) in [
        (Stop::Saving, &[][..]),
        (Stop::Creating, &["create", "drop new"][..]),
    ] {
        let state = ExportState::default();
        let (prepared, dir) = batch();
        let mut steps = Steps::preparing(Ok(prepared));
        steps.stop = Some((at, &state));
        let started = start_on_slot(&state, &portrait(), &steps).await.unwrap();
        assert!(matches!(started, Started::Cancelled));
        assert_eq!(steps.done(), done);
        assert_eq!(jobs(dir.path()), 0, "no job left behind");
        assert!(!state.is_running(), "the slot is free again");
    }
}

/// A start that runs holds the slot with its job on disk (beside its files, in `.jobs`), and Cancel naming that job
/// reaches it before its first event.
#[tokio::test]
async fn a_start_that_runs_holds_the_slot_with_its_job_on_disk() {
    let state = ExportState::default();
    let (prepared, dir) = batch();
    let steps = Steps::preparing(Ok(prepared));
    let started = start_on_slot(&state, &portrait(), &steps).await.unwrap();
    let Started::Run(job, cancel) = started else {
        panic!("it runs");
    };
    assert!(job.dir.join("job.json").is_file(), "the job is on disk");
    assert_eq!(
        job.dir.parent(),
        Some(dir.path().join("exports").join(".jobs").as_path())
    );
    assert_eq!(steps.done(), ["create"]);
    assert_eq!(jobs(dir.path()), 1);
    assert!(state.is_running(), "the export holds the slot");
    state.cancel_for(Some(&job.job_id), None).unwrap();
    assert!(*cancel.borrow(), "named by its job");
}
