//! The parent side of export:
//! - plan a job (`ExportJob::create`), or open one to resume;
//! - run the worker (the app binary itself) on it and forward its progress. The time left is measured from the
//!   worker's first progress line;
//! - when the worker fails, crashes or stalls, run it again on the same job. It continues from the finished
//!   segments: in hardware with fresh sessions, then in SoftwareSafe mode only while no format has
//!   finished hardware segments to redo; otherwise the job pauses ("the video encoder is busy"). Every run starts in
//!   hardware, unless the user chose to finish the job in compatibility mode (it stays chosen through pauses);
//! - pause: the worker finishes or drops its current segment and exits. A full disk pauses too, with a message, and
//!   so does a file whose disk doesn't answer (naming it). A missing file fails the job but keeps its segments;
//! - rename the joined files into place, never overwriting, then delete the job folder. Cancel deletes it as well.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use klipolo_model::captions::sidecar::CaptionFile;
use klipolo_model::project::Project;
use klipolo_model::speech::Speech;
use tokio::sync::watch;

use super::attempt::{AttemptResult, run_attempt};
use super::deliver::finish;
use super::job::{ExportJob, ExportMode, JobState, SEGMENT_SECONDS};
use super::jobs::frames_of;
use super::protocol::ErrorCode;
use super::registry::{Command, Registered};
use crate::api::{
    CodedError, ExportCompatibility, ExportEvent, ExportQuality, PlannedFile, ResumeMode, Said,
};

pub use super::launch::{Attempt, Launcher, ProcessLauncher, Supervise, Worker};
pub use super::registry::{cancel, forget_pause, is_running, pause, pause_when_it_runs};

pub struct Batch {
    /// The project as saved when the user pressed Export (flush the save first); later edits don't race the export.
    pub project: Project,
    pub fps: u32,
    pub quality: ExportQuality,
    /// Planned in the exports folder: the job folder goes in `<that folder>/.jobs/<job id>` (same volume).
    pub files: Vec<PlannedFile>,
    /// The preview's PCM cache (decoded audio is reused), or None for a temp folder.
    pub pcm_dir: Option<PathBuf>,
    /// What was said in the project's files: the job freezes its caption pages, for exactly its files' formats, in
    /// `captions.json` (`ExportJob::create`).
    pub speech: Arc<Speech>,
    /// The language the project's video text is in (the feed's `text_language`): the job freezes it.
    pub text_language: Arc<str>,
    /// What the job draws besides the built-ins, as the preview's snapshot does: written into the job
    /// (`export::contrib`) when it is made.
    pub contribs: super::contrib::JobContribs,
    /// The caption file to save beside each landscape video (the export popover's choice): the job makes
    /// it from the pages it freezes, Clean's whatever style is burned in.
    pub caption_file: Option<CaptionFile>,
    /// The person chose "Export with Inter instead": a text in a font that isn't here draws Inter's
    /// nearest face instead of stopping the export. Never CJK as boxes: the worker stops on that whatever this says.
    pub allow_stand_ins: bool,
    /// The editor's screen activity folder (`ExportJob::activity_dir`), or None to plan without the screens' activity.
    pub activity_dir: Option<PathBuf>,
    /// Export without the cursors that can't be read (the person allowed it: `ErrorCode::CursorMissing`).
    pub allow_missing_cursor: bool,
}

#[derive(Debug)]
pub enum Outcome {
    Finished(Vec<PathBuf>),
    /// Stopped with its finished parts kept (paused, or the disk is full); `resume_job` continues it.
    Paused,
    Failed(Said),
    Cancelled,
}

pub const DISK_FULL: &str = "Your disk is full. Free up space, then resume.";
/// The hardware encoder failed twice and switching to the software one would throw away finished segments.
pub const ENCODER_BUSY: &str = "The video encoder is busy. Another app may be using it, like a screen recorder or a video call. Close it, then resume.";
/// Windows and Linux until W and L: Klipolo's LGPL FFmpeg has no software H.264 encoder, and no OS encoder is wired up.
pub const NO_H264: &str =
    "This computer has no H.264 encoder, so this video can't be exported here yet.";
/// A clip failed to decode twice in hardware, where switching would throw away finished segments.
pub const DECODE_FAILED: &str = "A clip couldn't be decoded with hardware. Resume to try again, or finish in compatibility mode.";
/// The GPU failed twice, where switching would throw away finished segments.
pub const GPU_FAILED: &str = "The graphics processor stopped responding. Resume to try again, or finish in compatibility mode.";
/// Anything else failed twice (a crash, a stall), where switching would throw away finished segments.
pub const STOPPED: &str =
    "The export stopped unexpectedly. Resume to try again, or finish in compatibility mode.";

/// The runs a resume climbs. Auto: hardware (each format with the encoder its finished segments came from), hardware
/// again with fresh sessions, then compatibility mode, but that last step only while no format has finished hardware
/// segments to redo (the software encoder's can't join them): otherwise it pauses and the user chooses. Compatible:
/// compatibility mode for every format.
fn ladder(mode: ResumeMode) -> &'static [ExportMode] {
    match mode {
        ResumeMode::Auto => &[
            ExportMode::Hardware,
            ExportMode::Hardware,
            ExportMode::SoftwareSafe,
        ],
        ResumeMode::Compatible => &[ExportMode::SoftwareSafe, ExportMode::SoftwareSafe],
    }
}

/// Why a job with finished hardware segments paused after two failed hardware runs (plain, with what to do).
fn hardware_pause(code: Option<ErrorCode>) -> &'static str {
    match code {
        Some(ErrorCode::EncoderUnavailable | ErrorCode::Encode) => ENCODER_BUSY,
        Some(ErrorCode::Decode) => DECODE_FAILED,
        Some(ErrorCode::Gpu) => GPU_FAILED,
        _ => STOPPED,
    }
}

/// Exports `batch` (segments of 30 s). `cancel` stops it and deletes everything; `pause(job_id)` keeps it.
pub async fn run_batch(
    launcher: &dyn Launcher,
    batch: Batch,
    cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    run_batch_with(launcher, batch, SEGMENT_SECONDS, cancel, emit).await
}

/// `run_batch` with segments of `segment_s` seconds (tests use short ones).
pub async fn run_batch_with(
    launcher: &dyn Launcher,
    batch: Batch,
    segment_s: u32,
    cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    if frames_of(&batch.project, batch.fps.max(1)) == 0 {
        return failed(
            "There's nothing to export yet: add a clip to the timeline.".into(),
            emit,
        );
    }
    match ExportJob::create(&batch, segment_s) {
        Ok(job) => run_job(launcher, job, ResumeMode::Auto, cancel, emit).await,
        Err(message) => failed(message.into(), emit),
    }
}

/// Runs `job`, already written by `ExportJob::create` (the app makes it before Start over discards the job it replaces:
/// replaces), from its first segment.
pub async fn run_created(
    launcher: &dyn Launcher,
    job: ExportJob,
    cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    run_job(launcher, job, ResumeMode::Auto, cancel, emit).await
}

/// Continues the job in `dir` from its finished segments.
pub async fn resume_job(
    launcher: &dyn Launcher,
    dir: &Path,
    cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    resume_job_with(launcher, dir, ResumeMode::Auto, cancel, emit).await
}

/// `resume_job`, or (`ResumeMode::Compatible`) finishing in compatibility mode: slower, and it redoes the finished
/// hardware parts of formats that still have parts to render (they can't join the software encoder's).
pub async fn resume_job_with(
    launcher: &dyn Launcher,
    dir: &Path,
    mode: ResumeMode,
    cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    match ExportJob::open(dir) {
        Ok(job) => run_job(launcher, job, mode, cancel, emit).await,
        Err(reason) => {
            tracing::warn!(%reason, "export: the job couldn't be opened");
            failed(super::jobs::UNREADABLE.into(), emit)
        }
    }
}

/// Deletes a job folder outright (a job just written that won't run). Discard and Stop export use
/// `discard_job_atomically`.
pub fn discard_job(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => {
            tracing::warn!(%e, "export: the job folder couldn't be deleted");
            Err("The export couldn't be discarded. Try again.".into())
        }
    }
}

/// Discards the job in `dir` all at once (Discard, and Start over's swap). Its folder
/// is renamed in place (same folder, so same volume) to `jobs::discarded_name`, which the job list ignores, then
/// deleted in the background; a crash before that ends leaves it for the next start's `tidy_staging`. A rename that
/// fails leaves the job exactly as it was. A job already gone is fine.
pub fn discard_job_atomically(dir: &Path) -> Result<(), String> {
    let Some(id) = dir.file_name().and_then(|n| n.to_str()) else {
        return Err("The export couldn't be discarded. Try again.".into());
    };
    let gone = dir.with_file_name(super::jobs::discarded_name(id));
    match std::fs::rename(dir, &gone) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            tracing::warn!(%e, "export: the job folder couldn't be set aside to discard");
            return Err("The export couldn't be discarded. Try again.".into());
        }
    }
    let deleting = std::thread::Builder::new()
        .name("klipolo-discard".into())
        .spawn({
            let gone = gone.clone();
            move || {
                if let Err(e) = std::fs::remove_dir_all(&gone) {
                    tracing::warn!(%e, "export: a discarded job's files couldn't all be deleted yet");
                }
            }
        });
    // No thread: deleted here instead (the job is already out of the list either way).
    if deleting.is_err() {
        let _ = std::fs::remove_dir_all(&gone);
    }
    Ok(())
}

pub(super) fn failed(message: Said, emit: &(dyn Fn(ExportEvent) + Send + Sync)) -> Outcome {
    emit(ExportEvent::Failed {
        message: message.clone(),
    });
    Outcome::Failed(message)
}

/// What a failed attempt says: a frame it couldn't draw whole (`ErrorCode::Frame`), a cursor it can't read
/// (`ErrorCode::CursorMissing`) or a screen it can't read now (`ErrorCode::ActivityUnreadable`), the questions the UI
/// asks, as its code; anything else as it is.
fn reason(code: Option<ErrorCode>, message: String) -> Said {
    match code {
        Some(ErrorCode::Frame | ErrorCode::CursorMissing | ErrorCode::ActivityUnreadable) => {
            serde_json::from_str::<CodedError>(&message).map_or(Said::Text(message), Said::Coded)
        }
        _ => Said::Text(message),
    }
}

async fn run_job(
    launcher: &dyn Launcher,
    mut job: ExportJob,
    resume: ResumeMode,
    mut cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    let Some((_registered, mut control)) = Registered::new(&job.job_id) else {
        return failed("This export is already running.".into(), emit);
    };
    // Once chosen, compatibility mode stays chosen: a plain Resume of a paused compatibility run continues in it.
    let resume = if job.chosen_compatible {
        ResumeMode::Compatible
    } else {
        resume
    };
    job.chosen_compatible = resume == ResumeMode::Compatible;
    // The fonts of the app running it now (saved with the attempt below): a job paused before the app moved or updated
    // resumes with the fonts that exist, never a folder that's gone.
    job.fonts_dir = crate::render::fonts::dirs().bundled;
    // And its downloads as they are now: a font downloaded since the pause draws, and one removed since is missing, said
    // by its frames (`frame.fontMissing`, which stops the export unless it may use the stand-in).
    job.downloads = crate::render::fonts::downloads();
    // And this computer's fonts as the app's scan found them, so the worker never scans them again, off
    // the async threads: a few hundred KB written.
    let dir = job.dir.clone();
    let _ = tokio::task::spawn_blocking(move || crate::render::fonts::record_scan(&dir)).await;
    emit(ExportEvent::Started {
        job_id: job.job_id.clone(),
        files: job.files(),
        total_frames: job.format_frames(),
        segments_total: job.segments_total(),
    });
    if job.frames_done() > 0 {
        emit(progress(&job, None));
    }
    // Paused before its run started (Quit in Start over's swap: `pause_when_it_runs`): it pauses now, its worker never
    // launched, and resumes next time.
    if *control.borrow() == Command::Pause {
        return paused(&mut job, None, false, emit);
    }
    let ladder = ladder(resume);
    let mut noted = Vec::new();
    for (attempt, &mode) in ladder.iter().enumerate() {
        job.mode = mode;
        job.state = JobState::Running;
        job.message = None;
        if !job.chosen_compatible {
            job.offer_compatible = false;
        }
        if let Err(e) = job.save() {
            if super::shared::is_disk_full(&e) {
                return paused(&mut job, Some(DISK_FULL.into()), false, emit);
            }
            tracing::warn!(%e, "export: the job couldn't be saved");
            return failed(
                "The export couldn't continue: its files couldn't be written. Try again.".into(),
                emit,
            );
        }
        let result = run_attempt(launcher, &job, &mut cancel, &mut control, &mut noted, emit).await;
        // The worker kept its progress in the manifest.
        if let Ok(fresh) = ExportJob::open(&job.dir) {
            job = fresh;
        }
        // Discard pressed while it was winding down: it goes now.
        let result = match result {
            AttemptResult::Paused | AttemptResult::Failed { .. }
                if *control.borrow() == Command::Cancel =>
            {
                AttemptResult::Cancelled
            }
            other => other,
        };
        match result {
            AttemptResult::Done => return finish(&mut job, emit),
            AttemptResult::Paused => return paused(&mut job, None, false, emit),
            // Stop export: the job goes all at once, as Discard does, never half
            // deleted and still listed. One that can't be set aside (a file held open elsewhere) is kept whole and
            // said paused with why, never said stopped while it still lists.
            AttemptResult::Cancelled => {
                if let Err(why) = discard_job_atomically(&job.dir) {
                    return paused(&mut job, Some(why), false, emit);
                }
                emit(ExportEvent::Cancelled);
                return Outcome::Cancelled;
            }
            AttemptResult::Failed {
                code: Some(ErrorCode::DiskFull),
                ..
            } => return paused(&mut job, Some(DISK_FULL.into()), false, emit),
            // A file's disk isn't answering: pause, naming it (another run or compatibility mode can't help; Resume
            // once the disk is back).
            AttemptResult::Failed {
                code: Some(ErrorCode::NotAnswering),
                message,
                ..
            } => return paused(&mut job, Some(message), false, emit),
            // Asked to pause while it failed: pause rather than start again.
            AttemptResult::Failed { .. } if *control.borrow() == Command::Pause => {
                return paused(&mut job, None, false, emit);
            }
            AttemptResult::Failed {
                retryable: true,
                code,
                ..
            } if attempt + 1 < ladder.len() => {
                // The next run would switch to the software encoder and throw away finished hardware segments:
                // pause instead (no "Retrying"), and let the user choose compatibility mode. With nothing to redo
                // (a format complete in hardware joins as it is) it loses nothing, so it just runs.
                let next = ladder[attempt + 1];
                if mode == ExportMode::Hardware
                    && next == ExportMode::SoftwareSafe
                    && job.outputs.iter().any(|o| o.parts_to_redo() > 0)
                {
                    job.mode = ExportMode::Hardware;
                    return paused(&mut job, Some(hardware_pause(code).into()), true, emit);
                }
                emit(ExportEvent::Retrying);
            }
            AttemptResult::Failed { code, message, .. } => {
                let message = reason(code, message);
                job.state = JobState::Failed;
                job.message = Some(message.clone());
                let _ = job.save();
                return failed(message, emit);
            }
        }
    }
    unreachable!("the last attempt always returns")
}

fn progress(job: &ExportJob, eta_s: Option<f64>) -> ExportEvent {
    ExportEvent::Progress {
        frame: job.frames_done(),
        total_frames: job.format_frames(),
        eta_s,
        segments_done: job.segments_done(),
        segments_total: job.segments_total(),
    }
}

/// Pauses the job (`compatible`: after hardware failures, so it may also finish in compatibility mode). A job the
/// user chose to finish in compatibility mode keeps offering it.
fn paused(
    job: &mut ExportJob,
    message: Option<String>,
    compatible: bool,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    job.state = JobState::Paused;
    job.message = message.clone().map(Said::Text);
    job.offer_compatible = compatible || job.chosen_compatible;
    // On a full disk this may not save: the job then shows as interrupted, and still resumes.
    let _ = job.save();
    let compatibility = job.offer_compatible.then(|| ExportCompatibility {
        parts_to_redo: job.outputs.iter().map(|o| o.parts_to_redo()).sum(),
        chosen: job.chosen_compatible,
    });
    emit(ExportEvent::Paused {
        message,
        compatibility,
    });
    Outcome::Paused
}
