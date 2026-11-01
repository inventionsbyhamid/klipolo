//! What the recorder does with its workers' lines, their exits and the clock: each worker's stdout is read on a thread
//! of its own; the supervisor (`supervise`) decides, under the recorder's lock; what it decided is carried out after
//! the lock is let go (`Effect`), so the page, the window and the landing never hold it.

use std::io::{BufRead, Read};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use klipolo_capture::protocol::Event;
use klipolo_capture::run::remove_empty_take;
use klipolo_capture::shell::WorkerState;
use klipolo_model::coded::CodedError;

use super::outcomes::{
    Effect, done, give_up, idle, journal_says_stopped, saved_from_files, take_event,
};
use super::supervise::SupervisorAction;
use super::take::{Recorder, Slot, Take};

/// The longest event line read; a take's lines are a few hundred bytes.
const MAX_LINE: u64 = 4 << 20;

impl Recorder {
    /// Worker `generation`'s stdout, read on a thread of its own until it closes, then its exit.
    #[allow(clippy::type_complexity)]
    pub(super) fn read_in_background(
        self: &Arc<Self>,
        generation: u64,
        stdout: Box<dyn BufRead + Send>,
        wait: Box<dyn FnOnce() -> Option<i32> + Send>,
    ) {
        let me = self.clone();
        let started = std::thread::Builder::new()
            .name("klipolo-record-events".into())
            .spawn(move || me.read(generation, stdout, wait));
        if let Err(error) = started {
            tracing::error!(generation, %error, "recorder: the worker's events can't be read");
        }
    }

    #[allow(clippy::type_complexity)]
    fn read(
        self: &Arc<Self>,
        generation: u64,
        mut stdout: Box<dyn BufRead + Send>,
        wait: Box<dyn FnOnce() -> Option<i32> + Send>,
    ) {
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut stdout).take(MAX_LINE).read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(n) if n as u64 == MAX_LINE && line.last() != Some(&b'\n') => {
                    tracing::warn!(
                        generation,
                        "recorder: an event line too long to read is skipped"
                    );
                    let mut rest = Vec::new();
                    let _ = stdout.read_until(b'\n', &mut rest);
                    self.handled.fetch_add(1, Ordering::SeqCst);
                }
                Ok(_) => match serde_json::from_slice::<Event>(line.trim_ascii()) {
                    Ok(ev) => self.on_event(generation, ev),
                    Err(error) => {
                        tracing::warn!(generation, %error, "recorder: a line from the worker isn't an event");
                        self.handled.fetch_add(1, Ordering::SeqCst);
                    }
                },
                Err(error) => {
                    tracing::warn!(generation, %error, "recorder: the worker's events stopped");
                    break;
                }
            }
        }
        let code = wait();
        tracing::info!(generation, ?code, "recorder: a worker ended");
        self.on_exit(generation, code);
    }

    /// One event from worker `generation`.
    pub(super) fn on_event(self: &Arc<Self>, generation: u64, ev: Event) {
        let now = self.now_ms();
        let effects = {
            let mut slot = self.lock();
            match current(&mut slot, generation) {
                Some(take) => take_event(take, &ev, now),
                // A gone worker's lines never reach the page.
                None => Vec::new(),
            }
        };
        self.carry_out(effects);
        self.handled.fetch_add(1, Ordering::SeqCst);
    }

    /// Worker `generation` ended with `code`.
    pub(super) fn on_exit(self: &Arc<Self>, generation: u64, code: Option<i32>) {
        let now = self.now_ms();
        let mut finishing = None;
        let effects = {
            let mut slot = self.lock();
            if current(&mut slot, generation).is_some() {
                let effects = self.take_exit(&mut slot, code, now, &mut finishing);
                if slot.take.is_some() {
                    self.start_ticker(&mut slot);
                }
                effects
            } else {
                Vec::new()
            }
        };
        self.carry_out(effects);
        // A take finished from its files is the recorder's until it has landed: no second take meanwhile. Then it ends
        // as every take does, with one `Idle` once it is freed.
        if let Some(id) = finishing {
            let mut slot = self.lock();
            if slot.take.as_ref().is_some_and(|t| t.cfg.id == id) {
                slot.take = None;
                self.sync_active(&slot);
            }
            drop(slot);
            self.carry_out(vec![Effect::Emit(id, idle())]);
        }
        self.handled.fetch_add(1, Ordering::SeqCst);
    }

    fn take_exit(
        self: &Arc<Self>,
        slot: &mut Slot,
        code: Option<i32>,
        now: u64,
        finishing: &mut Option<String>,
    ) -> Vec<Effect> {
        let (action, args, recovered, said_failed) = {
            let Some(take) = slot.take.as_mut() else {
                return Vec::new();
            };
            take.worker = None;
            let said_failed = take.sup.said_failed();
            let mut action = take.sup.on_exit(code, now);
            // The worker refuses too; this is the app's half: a take the journal says was stopped, finished or
            // recovered is never recorded again.
            if action == SupervisorAction::Resume && journal_says_stopped(take) {
                action = SupervisorAction::Recover;
            }
            tracing::info!(
                take = take.cfg.id,
                ?code,
                ?action,
                "recorder: the worker's exit"
            );
            let args = [
                "--record-worker".to_string(),
                take.dir.root.to_string_lossy().into_owned(),
                "--resume".to_string(),
            ];
            (action, args, done(take, true), said_failed)
        };
        match action {
            SupervisorAction::Resume => match self.spawn(slot, &args) {
                Ok((worker, stdout, wait)) => {
                    let generation = worker.generation;
                    if let Some(take) = slot.take.as_mut() {
                        take.worker = Some(worker);
                        take.sup.respawned(now);
                    }
                    self.read_in_background(generation, stdout, wait);
                    Vec::new()
                }
                Err(error) => {
                    tracing::error!(%error, "recorder: the take couldn't be resumed");
                    *finishing = Some(recovered.id.clone());
                    give_up(recovered)
                }
            },
            SupervisorAction::GiveUp => {
                *finishing = Some(recovered.id.clone());
                give_up(recovered)
            }
            // The crash loop's end says why itself (`give_up`), as does a worker that said `Failed` (a drive that stopped
            // answering while it saved); else it is said with the landed take.
            SupervisorAction::Recover => {
                *finishing = Some(recovered.id.clone());
                let told = if said_failed {
                    recovered
                } else {
                    saved_from_files(recovered)
                };
                vec![Effect::Finish(told)]
            }
            // Every take ends with one `Idle` once it is freed. No take (Cancel or Disarm before the gate; a worker
            // that ended before it): `Idle` alone, after `Failed` only when the worker didn't say why, and a folder
            // with nothing recorded goes.
            SupervisorAction::Ended { failed } => match slot.take.take() {
                Some(take) => {
                    self.sync_active(slot);
                    ended(take, failed)
                }
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// What the supervisor decided, now the lock is let go: the page hears each event, and Klipolo's window shows
    /// again once the take is saved, has failed or has ended.
    pub(super) fn carry_out(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Emit(take, ev) => {
                    self.env.sink.event(&take, &ev);
                    if shows_window(&ev) {
                        self.env.window.show_and_focus();
                    }
                }
                Effect::Hide => self.env.window.hide(),
                Effect::Finish(done) => self.env.sink.finished(&done),
            }
        }
    }
}

/// The take whose current worker is `generation`.
fn current(slot: &mut Slot, generation: u64) -> Option<&mut Take> {
    (slot.take.as_mut()).filter(|t| {
        t.worker
            .as_ref()
            .is_some_and(|w| w.generation == generation)
    })
}

/// A take that ended with nothing to land (or that landed already): the folder of one that never recorded goes, and
/// the page hears why (when the worker didn't say) and `Idle`.
fn ended(take: Take, failed: Option<CodedError>) -> Vec<Effect> {
    // The worker's own exit 3 removes it first; this is for one that ended some other way. A folder holding anything
    // recorded stays.
    if !take.sup.started() && take.dir.root.exists() {
        remove_empty_take(&take.dir);
    }
    let id = take.cfg.id;
    let failed = failed.map(|error| Effect::Emit(id.clone(), Event::Failed { error }));
    failed
        .into_iter()
        .chain([Effect::Emit(id, idle())])
        .collect()
}

/// The take was saved, failed, or is over: Klipolo comes back.
fn shows_window(ev: &Event) -> bool {
    matches!(
        ev,
        Event::Finished { .. }
            | Event::Failed { .. }
            | Event::State {
                state: WorkerState::Idle,
                ..
            }
    )
}
