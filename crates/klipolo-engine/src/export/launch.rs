//! Starting the worker: `Launcher` runs one attempt of a job; `ProcessLauncher` runs the app binary itself as
//! `--export-worker <job folder>/job.json` (or `--proxy-worker <job folder>/proxy.json` for an editing copy),
//! with its stdout as JSON lines, its stderr in the log and its stdin open for `pause` (closing it tells the worker its
//! parent is gone). `klipolo_platform::process::worker_command` adds the OS's background flags (no console window on
//! Windows). Each worker sets its own QoS ceiling at start: user-initiated for export, utility for editing copies.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::forward::Forward;
use super::protocol::PAUSE;

pub struct Attempt {
    /// The worker's stdout, one message per line.
    pub lines: mpsc::UnboundedReceiver<String>,
    /// True when the worker exited successfully.
    pub exit: oneshot::Receiver<bool>,
    /// Stop the worker now.
    pub kill: Box<dyn FnOnce() + Send>,
    /// Ask the worker to pause (it stops at a segment boundary and exits).
    pub pause: Box<dyn Fn() + Send + Sync>,
}

pub trait Launcher: Send + Sync {
    fn launch(&self, job_path: &Path) -> Result<Attempt, String>;
}

/// A supervisor's hold on its worker's life: the environment each launch gets, and what it hears when the
/// worker exits by itself (a kill of ours isn't reported). The app's telemetry implements it: the crash spool for the
/// worker's panic hook (`KLIPOLO_CRASH_SPOOL`), and the exit status for crash detection.
pub trait Supervise: Send + Sync {
    /// Variables to set (`Some`) or remove (`None`) in the worker's environment, read at each launch.
    fn env(&self) -> Vec<(&'static str, Option<OsString>)>;
    /// The worker exited: its pid, its start time (ms since 1970, read right after the spawn; 0 when the OS won't
    /// say) and its status.
    fn exited(&self, pid: u32, started_ms: u64, status: &ExitStatus);
}

/// Which worker the app binary runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Worker {
    /// `--export-worker`: user-initiated QoS (the user waits for it).
    #[default]
    Export,
    /// `--proxy-worker`: an editing copy, utility QoS (nobody waits for it).
    Proxy,
}

impl Worker {
    /// The command-line flag `crates/klipolo-app/src/main.rs` dispatches on.
    pub fn flag(self) -> &'static str {
        match self {
            Self::Export => "--export-worker",
            Self::Proxy => "--proxy-worker",
        }
    }
}

/// `<exe> <worker flag> <job folder>/<manifest>`. In the app, `exe` is `std::env::current_exe()`.
pub struct ProcessLauncher {
    pub exe: PathBuf,
    pub worker: Worker,
    /// The supervisor's hooks, if any ([`Supervise`]).
    pub supervise: Option<Arc<dyn Supervise>>,
}

impl ProcessLauncher {
    /// The export worker (`--export-worker`).
    pub fn export(exe: PathBuf) -> Self {
        Self {
            exe,
            worker: Worker::Export,
            supervise: None,
        }
    }

    /// The editing-copy worker (`--proxy-worker`).
    pub fn proxy(exe: PathBuf) -> Self {
        Self {
            exe,
            worker: Worker::Proxy,
            supervise: None,
        }
    }

    /// The same, with a supervisor's hooks.
    pub fn supervised(self, supervise: Arc<dyn Supervise>) -> Self {
        Self {
            supervise: Some(supervise),
            ..self
        }
    }
}

impl Launcher for ProcessLauncher {
    fn launch(&self, job_path: &Path) -> Result<Attempt, String> {
        let mut command = klipolo_platform::process::worker_command(&self.exe);
        for (key, value) in self.supervise.iter().flat_map(|s| s.env()) {
            match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        let mut child = command
            .arg(self.worker.flag())
            .arg(job_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| match self.worker {
                Worker::Export => format!("The export couldn't start: {e}"),
                Worker::Proxy => format!("The editing copy couldn't start: {e}"),
            })?;
        // Read now, while the worker runs: its start time names it in crash detection.
        let pid = child.id();
        let started_ms = pid
            .and_then(klipolo_platform::process::start_time)
            .map_or(0, |us| us / 1000);
        let supervise = self.supervise.clone();
        let (stdin, stdout, stderr) = (
            child.stdin.take(),
            child.stdout.take().expect("piped"),
            child.stderr.take().expect("piped"),
        );
        let (tx, lines) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        // Into the app's log, tagged with the job (its folder's name).
        let job = job_path
            .parent()
            .and_then(Path::file_name)
            .map(|id| id.to_string_lossy().into_owned())
            .unwrap_or_default();
        let worker = self.worker;
        let log = move |line: &str| match worker {
            Worker::Export => tracing::info!(target: "export_worker", %job, "{line}"),
            Worker::Proxy => tracing::info!(target: "proxy_worker", %job, "{line}"),
        };
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            let mut forward = Forward::default();
            while let Ok(Some(line)) = reader.next_line().await {
                for line in forward.line(line, Instant::now()) {
                    log(&line);
                }
            }
            for line in forward.finish() {
                log(&line);
            }
        });
        // The worker's stdin stays open while the attempt lives: its closing tells the worker its parent is gone.
        let (pause_tx, mut pause_rx) = mpsc::unbounded_channel::<()>();
        tokio::spawn(async move {
            let mut stdin = stdin;
            while pause_rx.recv().await.is_some() {
                if let Some(s) = stdin.as_mut() {
                    let _ = s.write_all(format!("{PAUSE}\n").as_bytes()).await;
                    let _ = s.flush().await;
                }
            }
        });
        let (exit_tx, exit) = oneshot::channel();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let ok = tokio::select! {
                status = child.wait() => {
                    if let (Ok(status), Some(supervise), Some(pid)) = (&status, &supervise, pid) {
                        supervise.exited(pid, started_ms, status);
                    }
                    status.map(|s| s.success()).unwrap_or(false)
                }
                _ = kill_rx => {
                    let _ = child.kill().await;
                    false
                }
            };
            let _ = exit_tx.send(ok);
        });
        Ok(Attempt {
            lines,
            exit,
            kill: Box::new(move || {
                let _ = kill_tx.send(());
            }),
            pause: Box::new(move || {
                let _ = pause_tx.send(());
            }),
        })
    }
}
