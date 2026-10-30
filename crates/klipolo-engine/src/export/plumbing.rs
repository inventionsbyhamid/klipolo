//! The worker process's plumbing: the parent's commands on stdin, heartbeats while a long set-up step runs,
//! the deadline that ends a worker whose parent is gone even if VideoToolbox won't let go, and the lock that keeps
//! one worker per job.

use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::protocol::{Lines, PAUSE, WorkerLine};
use super::shared::{Control, WorkerError, internal};
use super::worker::send;

/// How long a worker whose parent is gone may take to close its last segment before it exits anyway.
const GONE_GRACE: Duration = Duration::from_secs(20);
/// How long a new worker waits for a job lock that is still held: an orphan worker of an app that quit (its pause
/// timed out) lets go within `GONE_GRACE`.
pub(crate) const LOCK_WAIT: Duration = Duration::from_secs(GONE_GRACE.as_secs() + 5);
/// Set in the real worker process only: tests run `export` in-process and must never be exited.
static DEADLINE_ARMED: AtomicBool = AtomicBool::new(false);

/// This is the real worker process: a gone parent ends it after `GONE_GRACE`.
pub(crate) fn arm_deadline() {
    DEADLINE_ARMED.store(true, Ordering::SeqCst);
}

/// The parent is gone: stop now (finished segments are kept) and, in the real worker, exit after a grace period
/// even if an encoder is stuck.
pub(crate) fn parent_gone(control: &Control) {
    if control.stopped() {
        return;
    }
    control.stop();
    if DEADLINE_ARMED.load(Ordering::SeqCst) {
        let _ = std::thread::Builder::new()
            .name("klipolo-export-deadline".into())
            .spawn(|| {
                std::thread::sleep(GONE_GRACE);
                std::process::exit(3);
            });
    }
}

/// Reads the parent's commands; a closed pipe means the parent is gone.
pub(crate) fn listen(control: Arc<Control>) {
    let _ = std::thread::Builder::new()
        .name("klipolo-export-control".into())
        .spawn(move || {
            let pipe = klipolo_platform::process::stdin_is_pipe();
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(l) if l.trim() == PAUSE => control.pause(),
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            if pipe {
                parent_gone(&control);
            }
        });
}

/// One worker per job: an exclusive lock on the job folder for the process's life (released on any exit). A lock
/// another worker holds is waited for, up to `wait` (`LOCK_WAIT`): after a quit whose pause timed out, a resume can
/// start before the old worker has gone.
pub(crate) async fn lock_job(dir: &Path, wait: Duration) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("lock"))
        .map_err(|e| {
            eprintln!("export: the job lock couldn't be opened: {e}");
            "The export couldn't start: its files couldn't be opened. Try again.".to_string()
        })?;
    let until = Instant::now() + wait;
    loop {
        match klipolo_platform::process::lock_exclusive(&file) {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(_) => return Err("This export is already running.".into()),
        }
    }
}

/// Awaits `task`, telling the parent every few seconds that the worker is alive (a long check or decode would
/// otherwise look like a stall).
pub(super) async fn alive<T, W: std::io::Write>(
    task: tokio::task::JoinHandle<T>,
    out: &mut Lines<W>,
    control: &Control,
) -> Result<T, WorkerError> {
    tokio::pin!(task);
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.tick().await;
    loop {
        tokio::select! {
            done = &mut task => return done.map_err(internal),
            _ = tick.tick() => send(out, &WorkerLine::Preparing, control),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_held_job_lock_is_waited_for_then_refused() {
        let dir = tempfile::tempdir().unwrap();
        let held = lock_job(dir.path(), Duration::ZERO).await.unwrap();
        assert!(
            lock_job(dir.path(), Duration::from_millis(300))
                .await
                .is_err()
        );
        // The old worker exits while the new one waits: the new one takes the lock.
        let exits = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        assert!(lock_job(dir.path(), Duration::from_secs(5)).await.is_ok());
        exits.join().unwrap();
    }
}
