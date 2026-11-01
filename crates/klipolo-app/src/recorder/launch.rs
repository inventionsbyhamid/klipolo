//! Starting the recorder's worker: `ProcessLaunch` runs this app's own binary as `klipolo-app --record-worker …` by the
//! one spawn rule (`klipolo_platform::process::recorder_command`: a session and process group of its own, so it
//! outlives the app), with its stdin and stdout as the protocol's JSON lines. Its stderr goes into the app's log by the
//! workers' rule (`klipolo_platform::process::Forward`: repeats collapsed, at most 20 lines a second); a take's worker
//! also writes its own `diagnostics.log` in the take's folder.

use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use klipolo_platform::process::{Forward, recorder_command, stop_child};

/// How the recorder's workers are started (the app's process; tests' in-memory pipes).
pub trait Launch: Send + Sync {
    /// Starts a worker with `args` (`--record-worker …`).
    fn spawn(&self, args: &[String]) -> io::Result<WorkerIo>;
}

/// A running worker's ends.
pub struct WorkerIo {
    /// The app's commands, one JSON line each.
    pub stdin: Box<dyn Write + Send>,
    /// The worker's events, one JSON line each.
    pub stdout: Box<dyn BufRead + Send>,
    pub pid: u32,
    /// Stops the worker: `true` at once, `false` asks it to. A worker that has ended is left alone.
    pub kill: Box<dyn Fn(bool) + Send + Sync>,
    /// Once its stdout has closed: waits for the worker to end and gives its exit code (None: a signal ended it).
    pub wait: Box<dyn FnOnce() -> Option<i32> + Send>,
}

/// This app's binary as the worker.
pub struct ProcessLaunch {
    exe: PathBuf,
}

impl ProcessLaunch {
    pub fn new(exe: PathBuf) -> Self {
        Self { exe }
    }
}

/// How often `wait` looks whether the worker has ended (it is ending: its stdout closed).
const WAIT_STEP: Duration = Duration::from_millis(50);

impl Launch for ProcessLaunch {
    fn spawn(&self, args: &[String]) -> io::Result<WorkerIo> {
        let mut child = recorder_command(&self.exe)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let gone = || io::Error::other("the worker's pipes are missing");
        let stdin = child.stdin.take().ok_or_else(gone)?;
        let stdout = child.stdout.take().ok_or_else(gone)?;
        let pid = child.id();
        if let Some(stderr) = child.stderr.take() {
            forward_stderr(stderr, pid);
        }
        tracing::info!(pid, "recorder: a worker started");
        // The child is reaped only by `wait`, so until then its id names it and nothing else.
        let child = Arc::new(Mutex::new(child));
        let killer = child.clone();
        let kill = move |hard: bool| {
            let mut child = lock(&killer);
            if matches!(child.try_wait(), Ok(None))
                && let Err(error) = stop_child(&mut child, hard)
            {
                tracing::warn!(pid, hard, %error, "recorder: the worker couldn't be stopped");
            }
        };
        let wait = move || {
            loop {
                match lock(&child).try_wait() {
                    Ok(Some(status)) => return status.code(),
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(pid, %error, "recorder: the worker's exit couldn't be read");
                        return None;
                    }
                }
                std::thread::sleep(WAIT_STEP);
            }
        };
        Ok(WorkerIo {
            stdin: Box::new(stdin),
            stdout: Box::new(BufReader::new(stdout)),
            pid,
            kill: Box::new(kill),
            wait: Box::new(wait),
        })
    }
}

fn lock(child: &Mutex<Child>) -> std::sync::MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The worker's stderr into the app's log, on a thread of its own until it closes.
fn forward_stderr(stderr: ChildStderr, pid: u32) {
    let spawned = std::thread::Builder::new()
        .name("klipolo-record-log".into())
        .spawn(move || {
            let mut forward = Forward::default();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                for line in forward.line(line, Instant::now()) {
                    tracing::info!(target: "record_worker", pid, "{line}");
                }
            }
            for line in forward.finish() {
                tracing::info!(target: "record_worker", pid, "{line}");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "recorder: the worker's log isn't forwarded");
    }
}
