//! The app's side of an editing copy: it starts the worker on a job folder (`--proxy-worker <job>/proxy.json`; the
//! worker sets its own utility QoS), turns its lines into `ProxyEvent`s, and pauses or stops it. Resuming is starting
//! again on the same folder: the worker continues from its finished segments.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

pub use crate::export::launch::{Attempt, Launcher, ProcessLauncher, Worker};

use super::job::{PROXY_FILE, ProxyJob};
use super::protocol::{ProxyLine, parse_line};
use super::run::{FailCode, PauseReason};

/// How long a paused worker may take to stop after its current frame (closing a VideoToolbox session included)
/// before it is killed. Its finished segments are kept either way.
const PAUSE_GRACE: Duration = Duration::from_secs(20);

/// What the worker reports.
#[derive(Clone, Debug, PartialEq)]
pub enum ProxyEvent {
    /// `done` of `total` frames are copied.
    Progress {
        done: u64,
        total: u64,
    },
    Paused {
        reason: PauseReason,
    },
    /// The copy is at `path`.
    Done {
        path: PathBuf,
    },
    /// The copy can't be made, as the worker said: why (`code`; `missing` is `code == Missing`), in words.
    Failed {
        message: String,
        missing: bool,
        code: FailCode,
    },
    /// The worker ended without a last word of its own: it crashed, was killed, or exited without saying why. Nothing
    /// says the copy can't be made; its finished segments are kept, and starting again resumes it.
    Crashed {
        message: String,
    },
    /// Thumbnail chunk `chunk` of the source was refilled from the copy (`Fill::Full`).
    Thumbs {
        chunk: u32,
    },
}

impl From<ProxyLine> for ProxyEvent {
    fn from(line: ProxyLine) -> Self {
        match line {
            ProxyLine::Progress { done, total } => Self::Progress { done, total },
            ProxyLine::Paused { reason } => Self::Paused { reason },
            ProxyLine::Done { path } => Self::Done { path },
            ProxyLine::Failed {
                message,
                missing,
                code,
            } => Self::Failed {
                message,
                missing,
                code,
            },
            ProxyLine::Thumbs { chunk } => Self::Thumbs { chunk },
        }
    }
}

/// A running worker.
pub struct ProxyRunner {
    dir: PathBuf,
    /// The worker's events, in order; the channel closes after its last one.
    pub events: mpsc::UnboundedReceiver<ProxyEvent>,
    pause: Box<dyn Fn() + Send + Sync>,
    kill: Option<Box<dyn FnOnce() + Send>>,
    /// Fires once the worker has exited (and its last event was sent).
    exited: Option<oneshot::Receiver<()>>,
}

impl ProxyRunner {
    /// Starts the worker on `job_dir` (made by `ProxyJob::create` or `open_for`). Needs a tokio runtime.
    pub fn start(job_dir: &Path, launcher: &dyn Launcher) -> Result<Self, String> {
        let Attempt {
            mut lines,
            exit,
            kill,
            pause,
        } = launcher.launch(&job_dir.join(PROXY_FILE))?;
        let (tx, events) = mpsc::unbounded_channel();
        let (exited_tx, exited) = oneshot::channel();
        tokio::spawn(async move {
            let mut ended = false;
            while let Some(line) = lines.recv().await {
                let Some(line) = parse_line(&line) else {
                    continue;
                };
                ended |= matches!(
                    line,
                    ProxyLine::Paused { .. } | ProxyLine::Done { .. } | ProxyLine::Failed { .. }
                );
                let _ = tx.send(ProxyEvent::from(line));
            }
            let ok = exit.await.unwrap_or(false);
            if !ended {
                // Crashed or killed without a last word: what it finished is kept; starting again resumes it.
                let _ = tx.send(ProxyEvent::Crashed {
                    message: if ok {
                        "the worker exited without saying why".into()
                    } else {
                        "the worker stopped unexpectedly (crashed or killed)".into()
                    },
                });
            }
            let _ = exited_tx.send(());
        });
        Ok(Self {
            dir: job_dir.to_path_buf(),
            events,
            pause,
            kill: Some(kill),
            exited: Some(exited),
        })
    }

    /// The next event (None once the worker has exited and every event was taken).
    pub async fn next(&mut self) -> Option<ProxyEvent> {
        self.events.recv().await
    }

    /// Asks the worker to stop after its current frame and waits for it to exit (killed after `PAUSE_GRACE`).
    /// Finished segments are kept; starting a runner on the same folder resumes.
    pub async fn pause(&mut self) {
        (self.pause)();
        if let Some(exited) = self.exited.take()
            && tokio::time::timeout(PAUSE_GRACE, exited).await.is_err()
        {
            self.kill_now();
        }
    }

    /// Kills the worker and deletes the job folder (the copy is abandoned; a finished copy is never touched).
    pub async fn stop(mut self) {
        self.kill_now();
        if let Some(exited) = self.exited.take() {
            let _ = tokio::time::timeout(PAUSE_GRACE, exited).await;
        }
        remove_job(&self.dir);
    }

    fn kill_now(&mut self) {
        if let Some(kill) = self.kill.take() {
            kill();
        }
    }
}

/// Deletes an editing copy's job folder, only if it is one (`<stem>.job` holding `proxy.json`, or empty).
pub fn remove_job(dir: &Path) {
    let is_job = dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".job"));
    let readable =
        ProxyJob::open(dir).is_ok() || std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none());
    if is_job
        && readable
        && let Err(e) = std::fs::remove_dir_all(dir)
    {
        tracing::warn!(%e, "editing copy: the job folder couldn't be removed");
    }
}
