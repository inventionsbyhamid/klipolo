//! A start on the one export slot. The slot is taken first, so Cancel works from the moment Export is pressed; then the
//! latest edits are saved and the batch frozen (`StartSteps::prepare`), then its job is written to disk, and only then
//! does it run.
//!
//! - **Refused on the way, nothing is left**: another export runs (one a chat started, say); the latest changes
//!   couldn't be saved; a disk, folder or font reason; the job couldn't be written (a full disk, the volume gone). The
//!   slot is free again each time.
//! - **Cancel on the way**: pressed while the changes are saved, nothing is written; pressed while the job is written,
//!   the new job goes. Either way it ends as `Cancelled` and the slot is free.

use std::future::Future;

use klipolo_engine::api::ExportRequest;
use klipolo_engine::export::job::{ExportJob, SEGMENT_SECONDS};
use klipolo_engine::export::runner::{Batch, DISK_FULL};
use klipolo_model::coded::Said;
use tokio::sync::watch;

use super::ExportState;

/// The steps of a start that reach the app: the app's own (`commands::export`), or a test's.
pub trait StartSteps {
    /// The open project's id and name: whose export it is from the moment it holds the slot.
    fn open(&self) -> Option<(String, String)>;
    /// Saves the latest changes and freezes the batch.
    fn prepare(&self, req: &ExportRequest) -> impl Future<Output = Result<Batch, Said>> + Send;
    /// Writes the new job to disk (`create_job`, off the async threads).
    fn create(&self, batch: Batch) -> impl Future<Output = Result<ExportJob, Said>> + Send;
    /// Deletes a job just written that won't run (Cancel pressed while it was written).
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

/// Takes the slot for `req`, prepares its batch and writes its job.
pub async fn start_on_slot(
    state: &ExportState,
    req: &ExportRequest,
    steps: &impl StartSteps,
) -> Result<Started, Said> {
    let cancel = state.take_slot().await?;
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
    if *cancel.borrow() {
        state.release();
        return Ok(Started::Cancelled);
    }
    let job = match steps.create(batch).await {
        Ok(job) => job,
        Err(why) => return refused(state, why),
    };
    // The slot's job from now on: a Cancel naming it reaches it before its first event.
    state.set_job(job.job_id.clone());
    state.set_job_dir(job.dir.clone());
    if *cancel.borrow() {
        steps.drop_new(&job);
        state.release();
        return Ok(Started::Cancelled);
    }
    Ok(Started::Run(Box::new(job), cancel))
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
