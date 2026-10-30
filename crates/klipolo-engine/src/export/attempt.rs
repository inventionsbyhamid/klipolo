//! One run of the worker on a job: forward its progress (the time left is measured from its first progress line),
//! pass on a pause, stop it on Cancel, and stop a worker that has gone silent (its finished segments stay; the
//! runner runs it again).

use std::time::{Duration, Instant};

use tokio::sync::watch;

use super::job::ExportJob;
use super::launch::Launcher;
use super::protocol::{ErrorCode, WorkerLine, parse_line};
use super::registry::Command;
use super::runner::DISK_FULL;
use crate::api::{CodedError, ExportEvent, Said};

pub(super) const GENERIC: &str =
    "The export didn't finish. Try again, or restart your Mac if it keeps happening.";
/// The time left is shown only once this run has rendered this long (measured, never guessed).
const ETA_AFTER: Duration = Duration::from_secs(5);
/// A worker silent this long is stuck: it is stopped (its finished segments stay) and run again. A file's disk is waited
/// for less than this (`render::pool::EXPORT_STAT_LIMIT`), so a disk that doesn't answer is named, not a stall.
const STALL: Duration = Duration::from_secs(120);
/// While joining, one call (the faststart rewrite of a large file) may run this long without a word.
const STALL_FINISHING: Duration = Duration::from_secs(15 * 60);

fn user_message(code: ErrorCode, message: &str, retryable: bool) -> String {
    match code {
        // These name the file or say what to do.
        ErrorCode::MissingMedia
        | ErrorCode::InvalidJob
        | ErrorCode::Decode
        | ErrorCode::NotAnswering => message.into(),
        // The coded reason as JSON: the runner shows it coded (`runner::reason`).
        ErrorCode::Frame | ErrorCode::CursorMissing | ErrorCode::ActivityUnreadable => {
            message.into()
        }
        // No encoder on this computer at all (`runner::NO_H264`): another run can't help.
        ErrorCode::EncoderUnavailable if !retryable => message.into(),
        ErrorCode::DiskFull => DISK_FULL.into(),
        _ => GENERIC.into(),
    }
}

pub(super) enum AttemptResult {
    Done,
    Paused,
    Cancelled,
    Failed {
        code: Option<ErrorCode>,
        message: String,
        retryable: bool,
    },
}

/// One run of the worker on `job`. `noted`: the notes this job's run said already, so a retry doesn't say them again
///.
pub(super) async fn run_attempt(
    launcher: &dyn Launcher,
    job: &ExportJob,
    cancel: &mut watch::Receiver<bool>,
    control: &mut watch::Receiver<Command>,
    noted: &mut Vec<CodedError>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> AttemptResult {
    let mut a = match launcher.launch(&job.manifest_path()) {
        Ok(a) => a,
        Err(message) => {
            return AttemptResult::Failed {
                code: None,
                message,
                retryable: false,
            };
        }
    };
    let (mut first_progress, mut finishing, mut pausing) = (None::<Instant>, false, false);
    // `Finishing` shown: the pictures are done (only the sound, then the join, are left).
    let mut finish_shown = false;
    // A dropped sender only means nobody can stop it any more: never a reason to discard finished work.
    let (mut control_open, mut cancel_open) = (true, true);
    let (mut done, mut paused, mut error) = (false, false, None::<(ErrorCode, String, bool)>);
    // The whole percent of the screens' analysis last said ("Analysing the screen…").
    let mut analysing = None::<i32>;
    let mut heard = Instant::now();
    loop {
        let quiet = if finishing { STALL_FINISHING } else { STALL };
        tokio::select! {
            line = a.lines.recv() => {
                heard = Instant::now();
                match line.as_deref().map(parse_line) {
                    Some(Some(WorkerLine::Progress { frame, total_frames, fps, segments_done, segments_total })) => {
                        let since = *first_progress.get_or_insert_with(Instant::now);
                        let eta_s = (fps > 0.0 && since.elapsed() >= ETA_AFTER)
                            .then(|| total_frames.saturating_sub(frame) as f64 / fps);
                        emit(ExportEvent::Progress { frame, total_frames, eta_s, segments_done, segments_total });
                    }
                    // The worker analyses the screens before its first frame: each whole percent once. The line is a
                    // heartbeat too, and the time left still starts at the first frame's progress.
                    Some(Some(WorkerLine::Analysing { done })) => {
                        let percent = (done.clamp(0.0, 1.0) * 100.0).floor() as i32;
                        if analysing.replace(percent) != Some(percent) {
                            emit(ExportEvent::Analysing { done });
                        }
                    }
                    Some(Some(WorkerLine::Finishing)) => {
                        finishing = true;
                        if !std::mem::replace(&mut finish_shown, true) {
                            emit(ExportEvent::Finishing);
                        }
                    }
                    // The sound is still rendering after the pictures: no time left to show, and the line itself
                    // is the worker's heartbeat (every few seconds while the sound advances).
                    Some(Some(WorkerLine::Sound { .. })) => {
                        if !std::mem::replace(&mut finish_shown, true) {
                            emit(ExportEvent::Finishing);
                        }
                    }
                    Some(Some(WorkerLine::Note { message })) if !noted.contains(&message) => {
                        noted.push(message.clone());
                        emit(ExportEvent::Note { message: Said::Coded(message) });
                    }
                    Some(Some(WorkerLine::Paused)) => paused = true,
                    Some(Some(WorkerLine::Done { .. })) => done = true,
                    Some(Some(WorkerLine::Error { code, message, retryable })) => {
                        tracing::warn!(?code, %message, "export worker failed");
                        error = Some((code, user_message(code, &message, retryable), retryable));
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            changed = cancel.changed(), if cancel_open => {
                if changed.is_err() {
                    cancel_open = false;
                } else if *cancel.borrow() {
                    (a.kill)();
                    let _ = a.exit.await;
                    return AttemptResult::Cancelled;
                }
            }
            // Still listening while a pause winds down: Discard then takes effect at once.
            changed = control.changed(), if control_open => {
                match changed.map(|()| *control.borrow()) {
                    Ok(Command::Pause) if pausing => {}
                    Ok(Command::Pause) => {
                        pausing = true;
                        (a.pause)();
                    }
                    Ok(Command::Cancel) => {
                        (a.kill)();
                        let _ = a.exit.await;
                        return AttemptResult::Cancelled;
                    }
                    Ok(Command::Run) => {}
                    Err(_) => control_open = false,
                }
            }
            _ = tokio::time::sleep_until((heard + quiet).into()) => {
                tracing::warn!("export worker silent for {} s: stopping it", quiet.as_secs());
                (a.kill)();
                let _ = a.exit.await;
                if pausing {
                    return AttemptResult::Paused;
                }
                return AttemptResult::Failed { code: None, message: GENERIC.into(), retryable: true };
            }
        }
    }
    let ok = a.exit.await.unwrap_or(false);
    match (ok && done, ok && paused, error) {
        (true, _, _) => AttemptResult::Done,
        (_, true, _) => AttemptResult::Paused,
        (_, _, Some((code, message, retryable))) => AttemptResult::Failed {
            code: Some(code),
            message,
            retryable,
        },
        // Crashed without a word: run again (it continues from its finished segments).
        _ => AttemptResult::Failed {
            code: None,
            message: GENERIC.into(),
            retryable: true,
        },
    }
}
