//! The editing-copy commands. The list is the open project's, with what holds its copies; the `media-jobs` event
//! repeats it on every change (progress at most 4 times a second). Stop and Retry name a file of the open project: a
//! path the list doesn't have is refused (`NOT_IN_PROJECT`), so they never reach another project's copy. Refusals are
//! codes (`copies.refused.*`), never English. Pause and Resume take who asks (`by`): the person (the top bar's pill,
//! the default) or an action (`by: "action"`), so one's pause and the other's never undo each other.

use klipolo_engine::api::{CodedError, MediaJobList, PauseBy};
use tauri::State;

use super::{Command, MediaJobs};

/// The open project's editing copies, in the order they are made, and who holds them (the person, an action, automatic
/// copies off). Off the main thread: it waits for the schedule's lock, which is never held across a disk access.
#[tauri::command(async)]
pub fn media_jobs_list(jobs: State<'_, MediaJobs>) -> MediaJobList {
    jobs.list()
}

/// Pauses making copies (the running one stops after its current frame, keeping its finished segments) until the same
/// side resumes: `by` the person (the pill; the default) or an action. Each side's pause is its own.
#[tauri::command]
pub async fn media_jobs_pause(
    jobs: State<'_, MediaJobs>,
    by: Option<PauseBy>,
) -> Result<(), CodedError> {
    jobs.command(Command::Pause(by.unwrap_or_default())).await
}

/// Makes copies again, undoing only `by`'s own pause. The person's Resume also restarts copies paused for a full disk,
/// heat or a busy encoder; an action's doesn't (those wait for the person).
#[tauri::command]
pub async fn media_jobs_resume(
    jobs: State<'_, MediaJobs>,
    by: Option<PauseBy>,
) -> Result<(), CodedError> {
    jobs.command(Command::Resume(by.unwrap_or_default())).await
}

/// Stops `path`'s copy and deletes what it made so far (Retry starts over). A finished copy stays.
#[tauri::command]
pub async fn media_job_stop(jobs: State<'_, MediaJobs>, path: String) -> Result<(), CodedError> {
    jobs.command(Command::Stop(path)).await
}

/// Tries `path`'s copy again (failed, stopped or paused).
#[tauri::command]
pub async fn media_job_retry(jobs: State<'_, MediaJobs>, path: String) -> Result<(), CodedError> {
    jobs.command(Command::Retry(path)).await
}

/// Whether copies are made by themselves. Off: the running copy pauses and waiting ones are forgotten; finished
/// copies stay in use, and a change to the project lists no file to copy (only finished copies are looked for). On:
/// the project's files are decided again.
#[tauri::command]
pub async fn media_jobs_set_auto(jobs: State<'_, MediaJobs>, auto: bool) -> Result<(), CodedError> {
    jobs.command(Command::SetAuto(auto)).await
}
