//! A start on the one export slot, and Start over's swap. The slot is taken first, so Cancel works from the moment Export
//! is pressed; then the latest edits are saved and the batch frozen (`StartSteps::prepare`), then its job is written to
//! disk, and only then does it run. Start over names the kept job it replaces (`ExportRequest::replaces`), and that job
//! is discarded here only once the new export holds the slot, is prepared and its own job is on disk: the kept job
//! survives, or is cleanly replaced, never both lost.
//!
//! - **Refused on the way, nothing new is left and a kept job stays**: another export runs (one a chat started, say);
//!   the latest changes couldn't be saved; a disk, folder or font reason; the job couldn't be written (a full disk, the
//!   volume gone). The slot is free again each time.
//! - **Cancel on the way**: pressed while the changes are saved, nothing is written; pressed while the job is written,
//!   the new job goes. Either way it ends as `Cancelled`, the slot is free, and a kept job stays.
//! - **Only a job of the project being exported is replaced**: another project opened meanwhile is refused, and so is a
//!   job that can't be read. One already gone just starts.
//! - **Quit after the swap pauses the new export**: its job is the slot's as soon as it is written, so Quit pauses it
//!   the moment its run starts, and it resumes next time.
//! - **The discard is all or nothing** (`runner::discard_job_atomically`): one that fails refuses the start with the
//!   kept job as it was, and the new job goes.

use std::future::Future;
use std::path::Path;

use klipolo_engine::api::ExportRequest;
use klipolo_engine::export::job::{ExportJob, SEGMENT_SECONDS};
use klipolo_engine::export::jobs::valid_job_id;
use klipolo_engine::export::runner::{Batch, DISK_FULL};
use klipolo_model::coded::Said;
use tokio::sync::watch;

use super::ExportState;
use super::jobs::Owner;

/// The steps of a start that reach the app: the app's own (`commands::export`), or a test's.
pub trait StartSteps {
    /// The open project's id and name: whose export it is from the moment it holds the slot.
    fn open(&self) -> Option<(String, String)>;
    /// Saves the latest changes and freezes the batch.
    fn prepare(&self, req: &ExportRequest) -> impl Future<Output = Result<Batch, Said>> + Send;
    /// Whose the kept job `job` is (`jobs::owner`).
    fn owner(&self, job: &str) -> Owner;
    /// Writes the new job to disk (`create_job`, off the async threads).
    fn create(&self, batch: Batch) -> impl Future<Output = Result<ExportJob, Said>> + Send;
    /// Discards the kept job `job` all at once (`jobs::discard`).
    fn discard(&self, job: &str) -> Result<(), Said>;
    /// Deletes a job just written that won't run (Cancel pressed while it was written, or the kept one couldn't be
    /// discarded).
    fn drop_new(&self, job: &ExportJob);
}

/// What a start comes to.
pub enum Started {
    /// Its job is written and it runs: the slot stays taken until the run ends.
    Run(Box<ExportJob>, watch::Receiver<bool>),
    /// Stopped before it ran: nothing of it is left, and the slot is free again.
    Cancelled,
}

/// Lets go of the slot and refuses the start with `why`.
fn refused<T>(state: &ExportState, why: Said) -> Result<T, Said> {
    state.release();
    Err(why)
}

/// Takes the slot for `req`, prepares its batch and writes its job; then, for Start over, discards the kept job it
/// replaces (`req.replaces`), the last thing before it runs.
pub async fn start_on_slot(
    state: &ExportState,
    req: &ExportRequest,
    steps: &impl StartSteps,
) -> Result<Started, Said> {
    let invalid = || Said::code("exports.refused.invalid");
    if req
        .replaces
        .as_deref()
        .is_some_and(|job| !valid_job_id(job))
    {
        return Err(invalid());
    }
    let cancel = state.take_slot(None).await?;
    // Whose export this is, from the moment it holds the slot: an early Cancel that doesn't name a job (a reloaded
    // window) during the save below only reaches it from this project.
    if let Some((id, name)) = steps.open() {
        state.set_project(id, name);
    }
    let batch = match steps.prepare(req).await {
        Ok(batch) => batch,
        Err(why) => return refused(state, why),
    };
    state.set_project(batch.project.id.clone(), batch.project.name.clone());
    let replaces = match req.replaces.as_deref().map(|job| (job, steps.owner(job))) {
        None | Some((_, Owner::Gone)) => None,
        Some((job, Owner::Project(id))) if id == batch.project.id => Some(job),
        Some(_) => return refused(state, invalid()),
    };
    if *cancel.borrow() {
        state.release();
        return Ok(Started::Cancelled);
    }
    let job = match steps.create(batch).await {
        Ok(job) => job,
        Err(why) => return refused(state, why),
    };
    // The slot's job from now on: a Cancel naming it reaches it before its first event, and Quit pauses it rather than
    // stopping it.
    state.set_job(job.job_id.clone());
    // Its folder is remembered, so a kept job there is listed after a relaunch whatever folder is picked then.
    if let Some(folder) = job.dir.parent().and_then(Path::parent) {
        state.remember_folder(folder);
    }
    if *cancel.borrow() {
        steps.drop_new(&job);
        state.release();
        return Ok(Started::Cancelled);
    }
    if let Some(old) = replaces
        && let Err(why) = steps.discard(old)
    {
        steps.drop_new(&job);
        return refused(state, why);
    }
    Ok(Started::Run(Box::new(job), cancel))
}

/// Takes the slot to resume the kept job `job` in `dir`, its project the slot's from now on. Refused as busy while
/// another export runs, or this job runs already (Resume pressed twice, or from two places); a job still pausing is
/// waited for (`ExportState::take_slot`). A job that can't be read is `exports.unreadable`, and one another build made
/// is `exports.otherBuild` (Start over or Discard it); neither takes the slot.
pub async fn resume_on_slot(
    state: &ExportState,
    dir: &Path,
    job: &str,
) -> Result<watch::Receiver<bool>, Said> {
    let read = dir.to_path_buf();
    let opened = tokio::task::spawn_blocking(move || ExportJob::open(&read))
        .await
        .map_err(|_| Said::code("exports.notStarted"))?
        .map_err(|reason| {
            tracing::warn!(%job, %reason, "export: the job to resume can't be read");
            Said::code("exports.unreadable")
        })?;
    // Another build's job never resumes: its file would mix parts drawn by two builds.
    if !klipolo_engine::export::job::is_this_build(opened.build.as_deref()) {
        return Err(Said::code("exports.otherBuild"));
    }
    let cancel = state.take_slot(Some(job.to_owned())).await?;
    state.set_project(opened.project_id, opened.project_name);
    if klipolo_engine::export::runner::is_running(job) {
        state.release();
        return Err(Said::code(super::state::BUSY));
    }
    Ok(cancel)
}

/// Writes `batch`'s job (segments of 30 s) off the async threads; a full disk is said as such, anything else as not
/// started.
pub async fn create_job(batch: Batch) -> Result<ExportJob, Said> {
    tokio::task::spawn_blocking(move || ExportJob::create(&batch, SEGMENT_SECONDS))
        .await
        .map_err(|_| Said::code("exports.notStarted"))?
        .map_err(|why| {
            tracing::warn!(%why, "export: the job couldn't be written");
            // Said in English by the engine; a full disk is said as such, anything else as not started.
            if why == DISK_FULL {
                Said::code("exports.folder.diskFull")
            } else {
                Said::code("exports.notStarted")
            }
        })
}
