//! A run: the engine's runner on a written job, with what the dialog hears. The engine keeps a job that stops with
//! its finished parts (a full disk, an encoder that stays busy) so it can resume; Klipolo doesn't resume one, so such
//! a run has failed here, says why as a code, and its job and parts are deleted with it, as Cancel deletes them. A
//! file already in place stays: the dialog lists it.

use klipolo_engine::api::ExportEvent;
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::runner::{
    DECODE_FAILED, DISK_FULL, ENCODER_BUSY, GPU_FAILED, Launcher, Outcome, STOPPED,
    discard_job_atomically, run_created,
};
use klipolo_model::coded::Said;
use tokio::sync::watch;

/// What the dialog hears of `event`: `Paused` (a run that stopped keeping its parts) is a failure, its reason as a
/// code (`exports.diskFull`, `exports.encoderBusy`, `exports.decodeFailed`, `exports.gpuFailed`, `exports.stopped`) or,
/// for a reason only the engine words (a disk that stopped answering, naming its file), as it said it. Every other
/// event is as the engine sent it.
pub fn said(event: ExportEvent) -> ExportEvent {
    let ExportEvent::Paused { message, .. } = event else {
        return event;
    };
    let code = match message.as_deref() {
        Some(DISK_FULL) => "exports.diskFull",
        Some(ENCODER_BUSY) => "exports.encoderBusy",
        Some(DECODE_FAILED) => "exports.decodeFailed",
        Some(GPU_FAILED) => "exports.gpuFailed",
        Some(STOPPED) | None => "exports.stopped",
        Some(other) => {
            return ExportEvent::Failed {
                message: Said::Text(other.to_owned()),
            };
        }
    };
    ExportEvent::Failed {
        message: Said::code(code),
    }
}

/// Runs `job` from its first segment, telling `emit` (`said`) how it goes; `cancel` stops it and deletes its job. A run
/// that doesn't finish leaves no job and no part of one behind.
pub async fn run(
    launcher: &dyn Launcher,
    job: ExportJob,
    cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    let dir = job.dir.clone();
    let outcome = run_created(launcher, job, cancel, &|event: ExportEvent| {
        emit(said(event))
    })
    .await;
    if matches!(outcome, Outcome::Paused | Outcome::Failed(_))
        && let Err(why) = discard_job_atomically(&dir)
    {
        tracing::warn!(%why, "export: a job that didn't finish couldn't be deleted");
    }
    outcome
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use klipolo_engine::api::{ExportQuality, ExportRequest};
    use klipolo_engine::export::runner::Attempt;
    use klipolo_model::project::FormatKind;

    use super::*;

    /// A launcher whose worker never starts (as when the app's binary can't be run).
    struct NoWorker;

    impl Launcher for NoWorker {
        fn launch(&self, _job: &Path) -> Result<Attempt, String> {
            Err("no worker in this test".into())
        }
    }

    fn paused(message: Option<&str>) -> ExportEvent {
        ExportEvent::Paused {
            message: message.map(Into::into),
            compatibility: None,
        }
    }

    fn failed(code: &str) -> ExportEvent {
        ExportEvent::Failed {
            message: Said::code(code),
        }
    }

    #[test]
    fn a_run_that_stops_keeping_its_parts_has_failed_and_says_why() {
        assert_eq!(said(paused(Some(DISK_FULL))), failed("exports.diskFull"));
        assert_eq!(
            said(paused(Some(ENCODER_BUSY))),
            failed("exports.encoderBusy")
        );
        assert_eq!(
            said(paused(Some(DECODE_FAILED))),
            failed("exports.decodeFailed")
        );
        assert_eq!(said(paused(Some(GPU_FAILED))), failed("exports.gpuFailed"));
        assert_eq!(said(paused(Some(STOPPED))), failed("exports.stopped"));
        assert_eq!(said(paused(None)), failed("exports.stopped"));
        let gone = "Waiting for the disk with intro.mov. Check that it's connected and awake.";
        assert_eq!(
            said(paused(Some(gone))),
            ExportEvent::Failed {
                message: Said::Text(gone.into())
            },
            "the engine's own words when only it can say them"
        );
        for event in [
            ExportEvent::Finishing,
            ExportEvent::Cancelled,
            failed("exports.frame.tooManyTexts"),
        ] {
            assert_eq!(said(event.clone()), event);
        }
    }

    #[tokio::test]
    async fn a_run_that_fails_leaves_no_job_behind() {
        let dir = tempfile::tempdir().unwrap();
        let (project, speech) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        let req = ExportRequest {
            formats: vec![FormatKind::Square],
            quality: ExportQuality::High,
            caption_file: None,
            allow_stand_ins: false,
            replaces: None,
            allow_missing_cursor: false,
        };
        let batch = crate::export::freeze(
            Arc::new(project),
            &speech,
            "en".into(),
            &req,
            dir.path(),
            None,
        );
        let job = crate::export::create_job(batch).await.unwrap();
        let job_dir = job.dir.clone();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let events = heard.clone();
        let (_stop, cancel) = watch::channel(false);
        let outcome = run(&NoWorker, job, cancel, &move |e| {
            events.lock().unwrap().push(e)
        })
        .await;
        assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
        assert!(!job_dir.exists(), "its job and parts are gone");
        let heard = heard.lock().unwrap();
        assert!(matches!(heard.first(), Some(ExportEvent::Started { .. })));
        assert!(matches!(heard.last(), Some(ExportEvent::Failed { .. })));
        assert_eq!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().is_file())
                .count(),
            0,
            "no file in the folder"
        );
    }
}
