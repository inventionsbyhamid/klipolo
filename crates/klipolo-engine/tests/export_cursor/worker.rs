//! The export worker run in this process (as the app's `--export-worker` runs it, on a thread and a runtime of its
//! own), behind the runner: a new export or a resume, and every event the runner sent.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use klipolo_engine::api::{CodedError, ErrorParam, ExportEvent, ExportQuality};
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::naming::plan_files;
use klipolo_engine::export::protocol::{Lines, WorkerLine};
use klipolo_engine::export::runner::{
    Attempt, Batch, Launcher, Outcome, resume_job, run_batch_with,
};
use klipolo_engine::export::shared::Control;
use klipolo_engine::export::worker::{self, Ended};
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::project::Project;
use tokio::sync::{mpsc, oneshot, watch};

/// Runs the worker in this process: its lines reach the runner as a child's stdout does. A run that has sent a note
/// is stopped there, as if its parent had gone, so it ends without a word and the runner tries again.
pub struct InProcess;

/// The worker's stdout: each whole line to the runner.
struct Stdout {
    lines: mpsc::UnboundedSender<String>,
    pending: Vec<u8>,
    control: Arc<Control>,
}

impl Write for Stdout {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line).trim().to_owned();
            if line.contains(r#""type":"note""#) {
                self.control.stop();
            }
            let _ = self.lines.send(line);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Launcher for InProcess {
    fn launch(&self, job_path: &Path) -> Result<Attempt, String> {
        let job = ExportJob::open(job_path.parent().unwrap())?;
        let (tx, lines) = mpsc::unbounded_channel();
        let (exit_tx, exit) = oneshot::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let control = Arc::new(Control::default());
            let mut out = Lines::new(Stdout {
                lines: tx,
                pending: Vec::new(),
                control: control.clone(),
            });
            let ended = runtime.block_on(worker::export(job, &mut out, control.clone()));
            // As `--export-worker` says it: nothing once its parent is gone.
            if let Err(e) = &ended
                && !control.stopped()
            {
                let line = WorkerLine::Error {
                    code: e.code,
                    message: e.message.clone(),
                    retryable: e.retryable,
                };
                let _ = out.send(&line);
            }
            drop(out);
            let _ = exit_tx.send(ended.is_ok_and(|e| e == Ended::Done));
        });
        Ok(Attempt {
            lines,
            exit,
            kill: Box::new(|| {}),
            pause: Box::new(|| {}),
        })
    }
}

/// Every event `run` sent through the runner, and how it ended.
async fn recorded<F>(
    run: impl FnOnce(Box<dyn Fn(ExportEvent) + Send + Sync>) -> F,
) -> (Outcome, Vec<ExportEvent>)
where
    F: std::future::Future<Output = Outcome>,
{
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let outcome = run(Box::new(move |e: ExportEvent| sink.lock().unwrap().push(e))).await;
    let events = events.lock().unwrap().clone();
    (outcome, events)
}

/// The batch exporting `p` (landscape, high quality, at 30 fps) into `dir`.
pub fn batch(p: &Project, dir: &Path, allow_missing_cursor: bool) -> Batch {
    Batch {
        project: p.clone(),
        fps: 30,
        quality: ExportQuality::High,
        files: plan_files(p, &[Landscape], ExportQuality::High, dir),
        pcm_dir: None,
        speech: Default::default(),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor,
    }
}

/// Exports `p` into `dir` (segments of 1 s).
pub async fn export(
    p: &Project,
    dir: &Path,
    allow_missing_cursor: bool,
) -> (Outcome, Vec<ExportEvent>) {
    let batch = batch(p, dir, allow_missing_cursor);
    let (_cancel, cancel) = watch::channel(false);
    recorded(|emit| async move { run_batch_with(&InProcess, batch, 1, cancel, &*emit).await }).await
}

/// Resumes the job in `dir`.
pub async fn resume(dir: &Path) -> (Outcome, Vec<ExportEvent>) {
    let (_cancel, cancel) = watch::channel(false);
    recorded(|emit| async move { resume_job(&InProcess, dir, cancel, &*emit).await }).await
}

/// `code` with recording `n`'s name as `recording`.
pub fn named(code: &str, n: usize) -> CodedError {
    let name = CodedError::new("name.recording").with("number", n);
    CodedError::new(code).with("recording", ErrorParam::from(name))
}

/// The one job folder under `dir`'s `.jobs`.
pub fn the_job(dir: &Path) -> std::path::PathBuf {
    let mut jobs = std::fs::read_dir(dir.join(".jobs")).unwrap().flatten();
    let job = jobs.next().unwrap().path();
    assert!(jobs.next().is_none(), "one job");
    job
}

/// Every file under `dir`, relative to it.
pub fn files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            out.extend(files(&path).into_iter().map(|f| format!("{name}/{f}")));
        } else {
            out.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    out.sort();
    out
}
