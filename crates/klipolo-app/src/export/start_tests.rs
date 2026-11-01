//! A start never leaves anything behind on the way: a start refused or stopped before its job runs writes nothing (or
//! deletes the job it wrote) and lets go of the slot; a start that runs holds the slot with its job on disk.
//!
//! Start over never loses a kept export. The job a new export replaces (`ExportRequest::replaces`) is discarded only
//! once the new export holds the slot, is prepared and its own job is on disk, so a start refused or stopped on the way
//! keeps every finished part of it, and a start that runs discards it exactly once. Only a job of the project being
//! exported is ever replaced.

use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};

use klipolo_engine::api::ExportRequest;
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::runner::Batch;
use klipolo_model::coded::Said;
use klipolo_model::project::FormatKind;

use super::ExportState;
use super::jobs::Owner;
use super::start::{StartSteps, Started, create_job, resume_on_slot, start_on_slot};

/// A kept job's id, as `export_jobs` lists it.
const KEPT: &str = "0a1b2c3d-4e5f";

/// When the fake presses Cancel.
#[derive(Clone, Copy, PartialEq)]
enum Stop {
    /// While the latest changes are saved (before the job is made).
    Saving,
    /// While the job is being written.
    Creating,
}

/// The app's side of a start, faked: what preparing answers, whose the replaced job is, whether making the new job or
/// discarding the kept one fails, and what was done, in order.
struct Steps<'a> {
    prepared: Mutex<Option<Result<Batch, Said>>>,
    owner: Owner,
    create_fails: bool,
    discard_fails: bool,
    stop: Option<(Stop, &'a ExportState)>,
    /// Quit is clicked as the replaced job is discarded: the swap's moment before the new run starts.
    quit: Option<&'a ExportState>,
    done: Mutex<Vec<String>>,
}

impl Steps<'_> {
    fn preparing(answer: Result<Batch, Said>) -> Self {
        Self::replacing(answer, Owner::Gone)
    }

    fn replacing(answer: Result<Batch, Said>, owner: Owner) -> Self {
        Self {
            prepared: Mutex::new(Some(answer)),
            owner,
            create_fails: false,
            discard_fails: false,
            stop: None,
            quit: None,
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

    fn owner(&self, _job: &str) -> Owner {
        self.owner.clone()
    }

    fn discard(&self, job: &str) -> Result<(), Said> {
        self.note(&format!("discard {job}"));
        if let Some(state) = self.quit {
            state.pause_for_quit();
        }
        if self.discard_fails {
            Err(Said::code("exports.discardFailed"))
        } else {
            Ok(())
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

/// Start over's request: the project's 9:16 file, replacing `job`.
fn replacing(job: &str) -> ExportRequest {
    ExportRequest {
        replaces: Some(job.into()),
        ..portrait()
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
    let _other = state.reserve(None).unwrap();
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

/// The id of the test project's batch.
fn project_of(batch: &Batch) -> Owner {
    Owner::Project(batch.project.id.clone())
}

#[tokio::test]
async fn a_start_over_refused_because_another_export_runs_keeps_the_kept_job() {
    let state = ExportState::default();
    let _other = state.reserve(None).unwrap();
    let (prepared, _dir) = batch();
    let owner = project_of(&prepared);
    let steps = Steps::replacing(Ok(prepared), owner);
    let refused = start_on_slot(&state, &replacing(KEPT), &steps)
        .await
        .err()
        .expect("refused");
    assert_eq!(refused, Said::code("exports.refused.busy"));
    assert!(steps.done().is_empty(), "the kept job is kept");
}

#[tokio::test]
async fn a_start_over_whose_latest_changes_cant_be_saved_keeps_the_kept_job() {
    let state = ExportState::default();
    let steps = Steps::replacing(
        Err(Said::code("exports.unsaved")),
        Owner::Project("p".into()),
    );
    let refused = start_on_slot(&state, &replacing(KEPT), &steps)
        .await
        .err()
        .expect("refused");
    assert_eq!(refused, Said::code("exports.unsaved"));
    assert!(steps.done().is_empty(), "the kept job is kept");
    assert!(!state.is_running(), "the slot is free again");
}

/// A write error in `.jobs` (a full disk, the volume gone) while the new job is made.
#[tokio::test]
async fn a_start_over_whose_new_job_cant_be_written_keeps_the_kept_job() {
    let state = ExportState::default();
    let (prepared, _dir) = batch();
    let owner = project_of(&prepared);
    let mut steps = Steps::replacing(Ok(prepared), owner);
    steps.create_fails = true;
    let refused = start_on_slot(&state, &replacing(KEPT), &steps)
        .await
        .err()
        .expect("refused");
    assert_eq!(refused, Said::code("exports.notStarted"));
    assert_eq!(steps.done(), ["create"], "never discarded");
    assert!(!state.is_running());
}

/// Cancel pressed while the changes are saved, or while the new job is written: nothing replaced the kept job, so it
/// stays, and a new job already written goes.
#[tokio::test]
async fn a_start_over_stopped_before_its_new_job_runs_keeps_the_kept_job() {
    for (at, done) in [
        (Stop::Saving, &[][..]),
        (Stop::Creating, &["create", "drop new"][..]),
    ] {
        let state = ExportState::default();
        let (prepared, dir) = batch();
        let owner = project_of(&prepared);
        let mut steps = Steps::replacing(Ok(prepared), owner);
        steps.stop = Some((at, &state));
        let started = start_on_slot(&state, &replacing(KEPT), &steps)
            .await
            .unwrap();
        assert!(matches!(started, Started::Cancelled));
        assert_eq!(steps.done(), done, "the kept job is kept");
        assert_eq!(jobs(dir.path()), 0, "no new job left behind");
        assert!(!state.is_running());
    }
}

#[tokio::test]
async fn a_start_over_that_runs_discards_the_kept_job_once_after_its_new_job_is_made() {
    let state = ExportState::default();
    let (prepared, dir) = batch();
    let owner = project_of(&prepared);
    let steps = Steps::replacing(Ok(prepared), owner);
    let started = start_on_slot(&state, &replacing(KEPT), &steps)
        .await
        .unwrap();
    let Started::Run(job, _) = started else {
        panic!("it runs");
    };
    assert!(job.dir.join("job.json").is_file(), "the new job is on disk");
    assert_eq!(
        steps.done(),
        ["create".to_string(), format!("discard {KEPT}")],
        "discarded once, after the new job was made"
    );
    assert_eq!(jobs(dir.path()), 1);
    assert!(state.is_running(), "the new export holds the slot");
}

/// The kept job couldn't be taken away (its folder couldn't be set aside): the start is refused, the new job goes, the
/// kept one stays as it was, and the slot is free.
#[tokio::test]
async fn a_start_over_whose_kept_job_cant_be_discarded_is_refused() {
    let state = ExportState::default();
    let (prepared, dir) = batch();
    let owner = project_of(&prepared);
    let mut steps = Steps::replacing(Ok(prepared), owner);
    steps.discard_fails = true;
    let refused = start_on_slot(&state, &replacing(KEPT), &steps)
        .await
        .err()
        .expect("refused");
    assert_eq!(refused, Said::code("exports.discardFailed"));
    assert_eq!(
        steps.done(),
        [
            "create".to_string(),
            format!("discard {KEPT}"),
            "drop new".into()
        ]
    );
    assert_eq!(jobs(dir.path()), 0, "the new job went");
    assert!(!state.is_running());
}

/// Start over on project A's job while project B was opened meanwhile never discards A's job; one that can't be read
/// isn't discarded either; an id that isn't a job's is refused before anything. A job already gone just starts.
#[tokio::test]
async fn start_over_replaces_only_a_job_of_the_project_it_exports() {
    for owner in [Owner::Project("another".into()), Owner::Unreadable] {
        let state = ExportState::default();
        let (prepared, dir) = batch();
        let steps = Steps::replacing(Ok(prepared), owner);
        let refused = start_on_slot(&state, &replacing(KEPT), &steps)
            .await
            .err()
            .expect("refused");
        assert_eq!(refused, Said::code("exports.refused.invalid"));
        assert!(steps.done().is_empty(), "nothing made, nothing discarded");
        assert_eq!(jobs(dir.path()), 0);
        assert!(!state.is_running());
    }
    let state = ExportState::default();
    let steps = Steps::replacing(Err(Said::code("exports.unsaved")), Owner::Gone);
    let refused = start_on_slot(&state, &replacing("../other"), &steps)
        .await
        .err()
        .expect("refused");
    assert_eq!(refused, Said::code("exports.refused.invalid"));
    assert!(!state.is_running(), "never took the slot");
    let (prepared, _dir) = batch();
    let steps = Steps::replacing(Ok(prepared), Owner::Gone);
    let started = start_on_slot(&state, &replacing(KEPT), &steps)
        .await
        .unwrap();
    assert!(matches!(started, Started::Run(..)));
    assert_eq!(steps.done(), ["create"], "nothing left to discard");
}

/// A launcher for a run that must never start its worker.
struct NoWorker;

impl klipolo_engine::export::runner::Launcher for NoWorker {
    fn launch(&self, _job: &Path) -> Result<klipolo_engine::export::runner::Attempt, String> {
        Err("no worker in this test".into())
    }
}

/// Quit clicked in the moment between Start over's swap and its new run starting pauses the new export, never stops
/// it (which would delete it, the replaced job already gone): its job is the slot's as soon as it is written, so it is
/// paused the moment its run starts, kept and resumable next time.
#[tokio::test]
async fn quit_in_the_swap_pauses_the_new_export() {
    let state = ExportState::default();
    let (prepared, dir) = batch();
    let owner = project_of(&prepared);
    let mut steps = Steps::replacing(Ok(prepared), owner);
    steps.quit = Some(&state);
    let started = start_on_slot(&state, &replacing(KEPT), &steps)
        .await
        .unwrap();
    let Started::Run(job, cancel) = started else {
        panic!("it runs");
    };
    assert!(!*cancel.borrow(), "Quit didn't stop it");
    let outcome = crate::export::run(
        &NoWorker,
        *job,
        cancel,
        &|_: klipolo_engine::api::ExportEvent| {},
    )
    .await;
    assert!(
        matches!(outcome, klipolo_engine::export::runner::Outcome::Paused),
        "paused as its run starts: {outcome:?}"
    );
    assert_eq!(jobs(dir.path()), 1, "the new job is kept");
}

/// Resume takes the slot for its job with the job's project, so a Cancel that names no job reaches it only from that
/// project. A second Resume of the same job while it runs (pressed twice, or from two places) is refused as busy at
/// once and leaves the first alone; so is any Resume while another export runs. A job that can't be read is said so,
/// and never takes the slot.
#[tokio::test]
async fn two_resumes_at_once_are_refused() {
    let state = ExportState::default();
    let (prepared, _dir) = batch();
    let job = create_job(prepared).await.unwrap();
    let cancel = resume_on_slot(&state, &job.dir, &job.job_id).await.unwrap();
    assert!(state.is_running());
    assert_eq!(
        resume_on_slot(&state, &job.dir, &job.job_id).await.err(),
        Some(Said::code("exports.refused.busy")),
        "the same job, again"
    );
    assert!(state.is_running() && !*cancel.borrow(), "the first goes on");
    assert_eq!(
        state.cancel_for(None, Some("another project")),
        Err(Said::Coded(
            klipolo_model::coded::CodedError::new("exports.refused.otherProject")
                .with("project", job.project_name.as_str())
        ))
    );
    state.release();
    std::fs::write(job.dir.join("job.json"), b"{ damaged").unwrap();
    assert_eq!(
        resume_on_slot(&state, &job.dir, &job.job_id).await.err(),
        Some(Said::code("exports.unreadable"))
    );
    assert!(!state.is_running(), "never took the slot");
}
