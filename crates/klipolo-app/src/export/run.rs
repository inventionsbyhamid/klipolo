//! A run: the engine's runner on a written job, or on a kept job it resumes, with what the dialog hears. A run that
//! stops keeps its job and every finished part (a full disk, an encoder that stays busy, a file that went missing,
//! Quit), so Resume continues it from there; only Cancel deletes it. A file already in place stays: the dialog lists
//! it.

use std::path::PathBuf;

use klipolo_engine::api::{ExportEvent, ResumeMode};
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::jobs::UNREADABLE;
use klipolo_engine::export::runner::{
    DECODE_FAILED, DISK_FULL, ENCODER_BUSY, GPU_FAILED, Launcher, Outcome, STOPPED,
    resume_job_with, run_created,
};
use klipolo_model::coded::Said;
use tokio::sync::watch;

/// Why a job stopped, as the app's code when the engine's English has one (`exports.diskFull`,
/// `exports.encoderBusy`, `exports.decodeFailed`, `exports.gpuFailed`, `exports.stopped`, `exports.unreadable`); a
/// reason only the engine words (a disk that stopped answering, a missing file, naming it) stays as it said it.
pub fn coded(said: Said) -> Said {
    let Said::Text(text) = &said else {
        return said;
    };
    let code = match text.as_str() {
        DISK_FULL => "exports.diskFull",
        ENCODER_BUSY => "exports.encoderBusy",
        DECODE_FAILED => "exports.decodeFailed",
        GPU_FAILED => "exports.gpuFailed",
        STOPPED => "exports.stopped",
        UNREADABLE => "exports.unreadable",
        _ => return said,
    };
    Said::code(code)
}

/// What the dialog hears of `event`: why a run paused or failed as a code (`coded`); every other event as the engine
/// sent it. A pause with no reason is the person's (Pause, or Quit).
pub fn said(event: ExportEvent) -> ExportEvent {
    match event {
        ExportEvent::Paused {
            message,
            compatibility,
        } => ExportEvent::Paused {
            message: message.map(coded),
            compatibility,
        },
        ExportEvent::Failed { message } => ExportEvent::Failed {
            message: coded(message),
        },
        other => other,
    }
}

/// Runs `job` from its first segment, telling `emit` (`said`) how it goes; `cancel` stops it and deletes its job. A run
/// that stops otherwise keeps its job to resume.
pub async fn run(
    launcher: &dyn Launcher,
    job: ExportJob,
    cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    let (dir, id) = (job.dir.clone(), job.job_id.clone());
    let outcome = run_created(launcher, job, cancel, &|event: ExportEvent| {
        emit(said(event))
    })
    .await;
    ended(dir, id, &outcome).await;
    outcome
}

/// Continues the kept job in `dir` from its finished parts (`mode`: compatibility mode, when the person chose it),
/// telling `emit` (`said`) how it goes; `cancel` stops it and deletes its job.
pub async fn resume(
    launcher: &dyn Launcher,
    dir: PathBuf,
    mode: ResumeMode,
    cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(ExportEvent) + Send + Sync),
) -> Outcome {
    let outcome = resume_job_with(launcher, &dir, mode, cancel, &|event: ExportEvent| {
        emit(said(event))
    })
    .await;
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    ended(dir, id, &outcome).await;
    outcome
}

/// What a run that ended leaves (`cleanup::finish_run`), off the async threads: a cancelled job's parts can be
/// gigabytes.
async fn ended(dir: PathBuf, id: String, outcome: &Outcome) {
    let cancelled = matches!(outcome, Outcome::Cancelled);
    let _ = tokio::task::spawn_blocking(move || {
        super::cleanup::finish_run(&dir, &id, cancelled);
    })
    .await;
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use klipolo_engine::api::{ExportQuality, ExportRequest};
    use klipolo_engine::export::job::{JOB_FILE, JobState, PartStatus};
    use klipolo_engine::export::runner::Attempt;
    use klipolo_model::captions::sidecar::CaptionFile;
    use klipolo_model::project::FormatKind;

    use super::*;

    /// A launcher whose worker never starts (as when the app's binary can't be run).
    struct NoWorker;

    impl Launcher for NoWorker {
        fn launch(&self, _job: &Path) -> Result<Attempt, String> {
            Err("no worker in this test".into())
        }
    }

    /// A worker's run, from its lines and how it exits.
    fn attempt(lines: Vec<String>, ok: bool) -> Attempt {
        let (tx, lines_rx) = tokio::sync::mpsc::unbounded_channel();
        for line in lines {
            tx.send(line).unwrap();
        }
        drop(tx);
        let (exit_tx, exit) = tokio::sync::oneshot::channel();
        exit_tx.send(ok).unwrap();
        Attempt {
            lines: lines_rx,
            exit,
            kill: Box::new(|| {}),
            pause: Box::new(|| {}),
        }
    }

    /// A worker that renders the job's parts still to do, each part's bytes naming its format and number, and records
    /// each in the manifest as it finishes. With `disk_full_after`, the disk fills once it has rendered that many in
    /// this run; else it joins each format's parts into its file, in order, and finishes.
    struct Parts {
        disk_full_after: Option<usize>,
        rendered: AtomicUsize,
    }

    impl Parts {
        fn new(disk_full_after: Option<usize>) -> Self {
            Self {
                disk_full_after,
                rendered: AtomicUsize::new(0),
            }
        }
    }

    impl Launcher for Parts {
        fn launch(&self, manifest: &Path) -> Result<Attempt, String> {
            let mut job = ExportJob::open(manifest.parent().unwrap())?;
            for o in 0..job.outputs.len() {
                for i in 0..job.outputs[o].segments.len() {
                    if job.outputs[o].segments[i].status == PartStatus::Done {
                        continue;
                    }
                    if Some(self.rendered.load(Ordering::SeqCst)) == self.disk_full_after {
                        let full = serde_json::json!({
                            "type": "error", "code": "diskFull", "message": "No space left", "retryable": false,
                        });
                        return Ok(attempt(vec![full.to_string()], false));
                    }
                    let format = job.outputs[o].format;
                    let part = &mut job.outputs[o].segments[i];
                    std::fs::write(job.dir.join(&part.file), format!("{format:?} part {i}\n"))
                        .unwrap();
                    part.status = PartStatus::Done;
                    job.save().unwrap();
                    self.rendered.fetch_add(1, Ordering::SeqCst);
                }
            }
            let mut lines = Vec::new();
            for o in &job.outputs {
                let joined: Vec<u8> = o
                    .segments
                    .iter()
                    .flat_map(|s| std::fs::read(job.dir.join(&s.file)).unwrap())
                    .collect();
                std::fs::write(job.path(&o.file), &joined).unwrap();
                lines.push(
                    serde_json::json!({
                        "type": "formatDone", "format": o.format, "tempPath": job.path(&o.file),
                        "bytes": joined.len(),
                    })
                    .to_string(),
                );
            }
            lines.push(r#"{"type":"done","elapsedS":1.0}"#.into());
            Ok(attempt(lines, true))
        }
    }

    /// A worker that runs until it is paused (then stops at its part, as a worker does) or killed, saying nothing.
    struct Hangs;

    impl Launcher for Hangs {
        fn launch(&self, _job: &Path) -> Result<Attempt, String> {
            let (lines_tx, lines) = tokio::sync::mpsc::unbounded_channel();
            let (exit_tx, exit) = tokio::sync::oneshot::channel();
            let held = Arc::new(Mutex::new(Some((lines_tx, exit_tx))));
            let paused = held.clone();
            Ok(Attempt {
                lines,
                exit,
                kill: Box::new(move || {
                    if let Some((lines_tx, exit_tx)) = held.lock().unwrap().take() {
                        drop(lines_tx);
                        let _ = exit_tx.send(false);
                    }
                }),
                pause: Box::new(move || {
                    if let Some((lines_tx, exit_tx)) = paused.lock().unwrap().take() {
                        let _ = lines_tx.send(r#"{"type":"paused"}"#.into());
                        drop(lines_tx);
                        let _ = exit_tx.send(true);
                    }
                }),
            })
        }
    }

    fn paused(message: Option<&str>) -> ExportEvent {
        ExportEvent::Paused {
            message: message.map(Into::into),
            compatibility: None,
        }
    }

    fn paused_as(code: &str) -> ExportEvent {
        ExportEvent::Paused {
            message: Some(Said::code(code)),
            compatibility: None,
        }
    }

    fn request(formats: &[FormatKind], caption_file: Option<CaptionFile>) -> ExportRequest {
        ExportRequest {
            formats: formats.to_vec(),
            quality: ExportQuality::High,
            caption_file,
            allow_stand_ins: false,
            replaces: None,
            allow_missing_cursor: false,
        }
    }

    /// The 60 s test project's job for `formats`, written in `dir`.
    async fn job_in(dir: &Path, formats: &[FormatKind]) -> ExportJob {
        let (project, speech) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        let batch = crate::export::freeze(
            Arc::new(project),
            &speech,
            "en".into(),
            &request(formats, None),
            dir,
            None,
        );
        crate::export::create_job(batch).await.unwrap()
    }

    type Heard = Arc<Mutex<Vec<ExportEvent>>>;

    fn listener() -> (Heard, impl Fn(ExportEvent) + Send + Sync) {
        let heard: Heard = Arc::default();
        let events = heard.clone();
        (heard, move |e| events.lock().unwrap().push(e))
    }

    #[test]
    fn a_run_that_stops_keeping_its_parts_is_paused_and_says_why_as_a_code() {
        assert_eq!(said(paused(Some(DISK_FULL))), paused_as("exports.diskFull"));
        assert_eq!(
            said(paused(Some(ENCODER_BUSY))),
            paused_as("exports.encoderBusy")
        );
        assert_eq!(
            said(paused(Some(DECODE_FAILED))),
            paused_as("exports.decodeFailed")
        );
        assert_eq!(
            said(paused(Some(GPU_FAILED))),
            paused_as("exports.gpuFailed")
        );
        assert_eq!(said(paused(Some(STOPPED))), paused_as("exports.stopped"));
        assert_eq!(said(paused(None)), paused(None), "the person's pause");
        let gone = "Waiting for the disk with intro.mov. Check that it's connected and awake.";
        assert_eq!(
            said(paused(Some(gone))),
            paused(Some(gone)),
            "the engine's own words when only it can say them"
        );
        assert_eq!(
            said(ExportEvent::Failed {
                message: UNREADABLE.into()
            }),
            ExportEvent::Failed {
                message: Said::code("exports.unreadable")
            }
        );
        for event in [
            ExportEvent::Finishing,
            ExportEvent::Cancelled,
            ExportEvent::Failed {
                message: Said::code("exports.frame.tooManyTexts"),
            },
        ] {
            assert_eq!(said(event.clone()), event);
        }
    }

    /// A full disk partway: the job is kept with its finished part, and Resume, once there is space, renders only the
    /// parts still to do and finishes with exactly the file a run that never stopped makes. Its job then goes.
    #[tokio::test]
    async fn a_job_kept_after_a_full_disk_resumes_and_finishes_with_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let job = job_in(dir.path(), &[FormatKind::Landscape]).await;
        let (job_dir, parts) = (job.dir.clone(), job.segments_total() as usize);
        assert!(parts >= 2, "{parts} parts");
        let (heard, emit) = listener();
        let filling = Parts::new(Some(1));
        let outcome = run(&filling, job, watch::channel(false).1, &emit).await;
        assert!(matches!(outcome, Outcome::Paused), "{outcome:?}");
        assert_eq!(
            heard.lock().unwrap().last(),
            Some(&paused_as("exports.diskFull"))
        );
        let kept = ExportJob::open(&job_dir).expect("the job is kept");
        assert_eq!(kept.state, JobState::Paused);
        assert_eq!(kept.segments_done(), 1, "with its finished part");

        let (_, emit) = listener();
        let freed = Parts::new(None);
        let outcome = resume(
            &freed,
            job_dir.clone(),
            ResumeMode::Auto,
            watch::channel(false).1,
            &emit,
        )
        .await;
        let Outcome::Finished(resumed) = outcome else {
            panic!("it finishes: {outcome:?}");
        };
        assert_eq!(
            freed.rendered.load(Ordering::SeqCst),
            parts - 1,
            "only the parts still to do"
        );
        assert!(!job_dir.exists(), "the finished job goes");
        assert!(!dir.path().join(".jobs").exists(), "and .jobs with it");

        let other = tempfile::tempdir().unwrap();
        let once = job_in(other.path(), &[FormatKind::Landscape]).await;
        let (_, emit) = listener();
        let Outcome::Finished(whole) =
            run(&Parts::new(None), once, watch::channel(false).1, &emit).await
        else {
            panic!("a run that never stopped finishes");
        };
        assert_eq!(
            std::fs::read(&resumed[0]).unwrap(),
            std::fs::read(&whole[0]).unwrap(),
            "the same file"
        );
        assert_eq!(resumed[0].file_name(), whole[0].file_name());
    }

    /// A worker that can't start fails the run, and its job stays to resume (Try again), with no file in the folder.
    #[tokio::test]
    async fn a_run_that_fails_keeps_its_job_to_resume() {
        let dir = tempfile::tempdir().unwrap();
        let job = job_in(dir.path(), &[FormatKind::Square]).await;
        let job_dir = job.dir.clone();
        let (heard, emit) = listener();
        let (_stop, cancel) = watch::channel(false);
        let outcome = run(&NoWorker, job, cancel, &emit).await;
        assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
        assert!(job_dir.join(JOB_FILE).is_file(), "its job is kept");
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

    /// Cancel deletes the job and its parts, and the empty `.jobs` with it.
    #[tokio::test]
    async fn a_cancelled_run_leaves_no_job_behind() {
        let dir = tempfile::tempdir().unwrap();
        let job = job_in(dir.path(), &[FormatKind::Landscape]).await;
        let job_dir = job.dir.clone();
        let (stop, cancel) = watch::channel(false);
        let (started_tx, started) = tokio::sync::oneshot::channel();
        let started_tx = Mutex::new(Some(started_tx));
        let task = tokio::spawn(async move {
            run(&Hangs, job, cancel, &move |e| {
                if matches!(e, ExportEvent::Started { .. })
                    && let Some(tx) = started_tx.lock().unwrap().take()
                {
                    let _ = tx.send(());
                }
            })
            .await
        });
        started.await.unwrap();
        stop.send(true).unwrap();
        assert!(matches!(task.await.unwrap(), Outcome::Cancelled));
        assert!(!job_dir.exists());
        assert!(!dir.path().join(".jobs").exists());
    }

    /// A caption file asked for is written with the job, from the finished words, ready to go beside the landscape
    /// video; the job knows its kind.
    #[tokio::test]
    async fn a_job_keeps_the_caption_file_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let (project, speech) =
            klipolo_engine::test_support::captions_project("caption.bold", &[("Hello", 1.0, 1.4)]);
        let batch = crate::export::freeze(
            Arc::new(project),
            &speech,
            "en".into(),
            &request(&[FormatKind::Landscape], Some(CaptionFile::Srt)),
            dir.path(),
            None,
        );
        let job = crate::export::create_job(batch).await.unwrap();
        assert_eq!(job.caption_file, Some(CaptionFile::Srt));
        let frozen = std::fs::read_to_string(job.dir.join("captions.srt")).unwrap();
        assert!(frozen.contains("Hello"), "{frozen}");
    }

    /// Quitting while an export runs (any quit, held by the guard or not, the Mac shutting down included): the run
    /// pauses at its part and lets go of the slot before `pause_on_quit` returns; its job is kept, paused, and resumes
    /// next time to the end.
    #[tokio::test(flavor = "multi_thread")]
    async fn quitting_mid_export_pauses_it_and_it_resumes_next_time() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(crate::export::ExportState::default());
        let cancel = state.reserve(None).unwrap();
        let job = job_in(dir.path(), &[FormatKind::Landscape]).await;
        let job_dir = job.dir.clone();
        state.set_job(job.job_id.clone());
        let (started_tx, started) = tokio::sync::oneshot::channel();
        let started_tx = Mutex::new(Some(started_tx));
        let running = state.clone();
        let task = tokio::spawn(async move {
            let _slot = crate::export::Releases(&running);
            run(&Hangs, job, cancel, &move |e| {
                if matches!(e, ExportEvent::Started { .. })
                    && let Some(tx) = started_tx.lock().unwrap().take()
                {
                    let _ = tx.send(());
                }
            })
            .await
        });
        started.await.unwrap();
        let quitting = state.clone();
        tokio::task::spawn_blocking(move || {
            crate::export::pause_on_quit(&quitting, Duration::from_secs(10))
        })
        .await
        .unwrap();
        assert!(!state.is_running(), "the slot is free as the quit goes on");
        assert!(matches!(task.await.unwrap(), Outcome::Paused));
        assert_eq!(
            ExportJob::open(&job_dir).expect("kept").state,
            JobState::Paused
        );
        // Next time: Resume finishes it.
        let (_, emit) = listener();
        let outcome = resume(
            &Parts::new(None),
            job_dir.clone(),
            ResumeMode::Auto,
            watch::channel(false).1,
            &emit,
        )
        .await;
        assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
        assert!(!job_dir.exists());
    }

    /// A pause Quit queued for a job whose run never started goes with the slot it was queued on, so that job's next
    /// run in this process runs as asked, never paused at once.
    #[tokio::test]
    async fn a_pause_quit_queued_goes_with_its_released_slot() {
        let dir = tempfile::tempdir().unwrap();
        let job = job_in(dir.path(), &[FormatKind::Landscape]).await;
        let state = crate::export::ExportState::default();
        let _stop = state.reserve(Some(job.job_id.clone())).unwrap();
        assert!(state.pause_for_quit().is_some(), "Quit queued its pause");
        state.release();
        let (_, emit) = listener();
        let outcome = run(&NoWorker, job, watch::channel(false).1, &emit).await;
        assert!(
            matches!(outcome, Outcome::Failed(_)),
            "it ran as asked (and met the test's missing worker), never paused at once: {outcome:?}"
        );
    }
}
