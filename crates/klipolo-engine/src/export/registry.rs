//! The jobs this process runs, by id: `pause` and `cancel` reach a job whoever started it (the export popover's
//! Start, or Resume).

use std::sync::{Mutex, PoisonError};

use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    Run,
    Pause,
    Cancel,
}

/// The jobs this process runs, by id: `pause` and `cancel` reach them whoever started them.
static RUNNING: Mutex<Vec<(String, watch::Sender<Command>)>> = Mutex::new(Vec::new());
/// Jobs to pause the moment their run starts (`pause_when_it_runs`), at most `PENDING` of them (oldest go first).
/// Locked only while `RUNNING` is held, so a run that starts meanwhile never misses one.
static PAUSE_ON_START: Mutex<Vec<String>> = Mutex::new(Vec::new());
const PENDING: usize = 4;

fn command(job_id: &str, c: Command) -> bool {
    let running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    running
        .iter()
        .find(|(id, _)| id == job_id)
        .is_some_and(|(_, tx)| tx.send(c).is_ok())
}

/// Pause a running job: it stops at a segment boundary, keeping every finished segment. False if it isn't running.
pub fn pause(job_id: &str) -> bool {
    command(job_id, Command::Pause)
}

/// Pause a job now, or the moment its run starts: Quit in the few ms between Start
/// over writing its new job and that job's run starting pauses it there, before its worker launches, rather than
/// missing it. True if it was running.
pub fn pause_when_it_runs(job_id: &str) -> bool {
    let running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, tx)) = running.iter().find(|(id, _)| id == job_id) {
        return tx.send(Command::Pause).is_ok();
    }
    let mut pending = PAUSE_ON_START
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if !pending.iter().any(|id| id == job_id) {
        if pending.len() == PENDING {
            pending.remove(0);
        }
        pending.push(job_id.to_string());
    }
    false
}

/// Lets go of a pause `pause_when_it_runs` queued for a job whose run never started: the slot it was
/// queued for was released, so a later Resume of that job in this process starts as asked.
pub fn forget_pause(job_id: &str) {
    let _running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    PAUSE_ON_START
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|id| id != job_id);
}

/// Stop a running job and delete it. False if it isn't running.
pub fn cancel(job_id: &str) -> bool {
    command(job_id, Command::Cancel)
}

pub fn is_running(job_id: &str) -> bool {
    RUNNING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|(id, _)| id == job_id)
}

/// Removes its job from `RUNNING` when the run ends.
pub(super) struct Registered(String);

impl Registered {
    pub(super) fn new(job_id: &str) -> Option<(Self, watch::Receiver<Command>)> {
        let mut running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        if running.iter().any(|(id, _)| id == job_id) {
            return None;
        }
        // Paused before it started (`pause_when_it_runs`): it starts paused.
        let mut pending = PAUSE_ON_START
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let paused = pending
            .iter()
            .position(|id| id == job_id)
            .map(|i| pending.remove(i));
        drop(pending);
        let (tx, rx) = watch::channel(if paused.is_some() {
            Command::Pause
        } else {
            Command::Run
        });
        running.push((job_id.to_string(), tx));
        Some((Self(job_id.to_string()), rx))
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(id, _)| *id != self.0);
    }
}
