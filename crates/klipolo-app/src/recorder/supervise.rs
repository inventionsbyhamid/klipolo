//! The supervisor: a pure state machine over what one take's worker says, its exit and the clock, giving what the app
//! does about it. No threads, no IO: `take::Recorder` feeds it and carries the actions out.
//!
//! - **Hangs** are found by the worker's signs of life, never by a time limit on any state: it writes a `Heartbeat`
//!   every second whatever it does, saving included (20 s and more on a slow drive). Silent for 5 s gives
//!   `Kill { hard: false }`, and 2 s later `Kill { hard: true }`. A worker that hasn't said anything yet (a resumed one
//!   repairing its crashed runs) has 30 s first.
//! - **An exit without `Finished`**:
//!   - after the person's Stop (Stop, Cancel or Disarm sent, or the worker said Stopping): never resumed; the take is
//!     finished from its files (`Recover`);
//!   - while the take records (it passed the start gate): `Resume` as a new run, at most 3 times in 10 minutes, then
//!     `GiveUp`;
//!   - before the start gate, or exit 3 (no take): the take is over (`Ended`).

use std::collections::VecDeque;

use klipolo_capture::protocol::Event;
use klipolo_capture::shell::WorkerState;
use klipolo_model::coded::CodedError;

/// Silent this long, the worker is asked to stop.
pub const HANG_MS: u64 = 5_000;
/// Still there this long after being asked, it is killed.
pub const HARD_AFTER_MS: u64 = 2_000;
/// A worker that hasn't said anything yet gets this long.
pub const FIRST_WORDS_MS: u64 = 30_000;
/// At most this many resumes...
pub const RESUMES: usize = 3;
/// ...in this long.
pub const RESUME_WINDOW_MS: u64 = 10 * 60 * 1_000;

/// What the app does.
#[derive(Clone, Debug, PartialEq)]
pub enum SupervisorAction {
    /// Tell the page (`recorder-event`).
    Forward(Event),
    /// The take is recording: hide Klipolo's window (once per take).
    Hide,
    /// Stop the worker: ask (`false`), then force (`true`).
    Kill { hard: bool },
    /// Launch `--record-worker <take> --resume`: a new run of the crashed take.
    Resume,
    /// Crashed too often: the take ends (`capture.stop.crashLoop`) and is finished from its files.
    GiveUp,
    /// The take is complete: land it.
    Import,
    /// The worker ended after the person's Stop without finishing: finish the take from its files, then land it.
    Recover,
    /// No take to land (never started, cancelled, or the worker couldn't run): the page hears `State Idle` (after
    /// `Failed` with `failed` when the worker didn't say why itself).
    Ended { failed: Option<CodedError> },
}

/// One take's supervision, across its workers (each resume is a new worker).
#[derive(Clone, Debug)]
pub struct Supervisor {
    /// What the current worker last said it was doing; None before its first `State`.
    state: Option<WorkerState>,
    /// When the current worker last said anything (ms on the supervisor's clock); its launch until then.
    life_ms: u64,
    /// It has said something.
    spoke: bool,
    /// When it was asked to stop for hanging.
    asked_ms: Option<u64>,
    killed: bool,
    /// The current worker has ended: nothing to watch until a resumed one takes over.
    exited: bool,
    /// The take passed the start gate (Recording or Paused seen).
    started: bool,
    /// The person's Stop, Cancel or Disarm was sent, or a worker said Stopping.
    stopping: bool,
    finished: bool,
    /// The worker said why it failed (`Failed`), so an end needn't.
    said_failed: bool,
    hidden: bool,
    /// When each resume happened.
    resumes: VecDeque<u64>,
}

impl Supervisor {
    pub fn new(now_ms: u64) -> Self {
        Self {
            state: None,
            life_ms: now_ms,
            spoke: false,
            asked_ms: None,
            killed: false,
            exited: false,
            started: false,
            stopping: false,
            finished: false,
            said_failed: false,
            hidden: false,
            resumes: VecDeque::new(),
        }
    }

    /// The take passed the start gate, so it is saved when it stops.
    pub fn started(&self) -> bool {
        self.started
    }

    /// The person's Stop, Cancel or Disarm went to the worker, or it said Stopping.
    pub fn is_stopping(&self) -> bool {
        self.stopping
    }

    /// The person's Stop, Cancel or Disarm went to the worker (or the app stopped the take): it is never resumed after
    /// this.
    pub fn stop_requested(&mut self) {
        self.stopping = true;
    }

    /// One line from the worker: every line is a sign of life.
    pub fn on_event(&mut self, ev: &Event, now_ms: u64) -> Vec<SupervisorAction> {
        self.life_ms = now_ms;
        self.spoke = true;
        self.asked_ms = None;
        let mut out = Vec::new();
        match ev {
            // The supervisor's own while the take records: the page needn't wake once a second while Klipolo is
            // hidden. Once it saves they reach the page, so a long save is heard to be alive.
            Event::Heartbeat { .. } if !self.stopping => return out,
            Event::State { state, .. } => {
                self.state = Some(*state);
                match state {
                    WorkerState::Recording | WorkerState::Paused => self.started = true,
                    WorkerState::Stopping => {
                        self.stopping = true;
                        // Before the start gate there is nothing to save: the page hears `Idle` at the worker's end,
                        // never "Saving the recording".
                        if !self.started {
                            return out;
                        }
                    }
                    _ => {}
                }
                if *state == WorkerState::Recording && !self.hidden {
                    self.hidden = true;
                    out.push(SupervisorAction::Hide);
                }
            }
            Event::Failed { .. } => self.said_failed = true,
            _ => {}
        }
        out.push(SupervisorAction::Forward(ev.clone()));
        if matches!(ev, Event::Finished { .. }) && !self.finished {
            self.finished = true;
            out.push(SupervisorAction::Import);
        }
        out
    }

    /// The clock moved: a silent worker is asked to stop, then killed.
    pub fn on_tick(&mut self, now_ms: u64) -> Option<SupervisorAction> {
        // A finished take's worker is exiting while the take lands (which can take a while), and one finished from its
        // files has none: never "hung".
        if self.killed || self.finished || self.exited {
            return None;
        }
        if let Some(asked) = self.asked_ms {
            if now_ms.saturating_sub(asked) >= HARD_AFTER_MS {
                self.killed = true;
                return Some(SupervisorAction::Kill { hard: true });
            }
            return None;
        }
        let quiet = if self.spoke { HANG_MS } else { FIRST_WORDS_MS };
        if now_ms.saturating_sub(self.life_ms) >= quiet {
            tracing::warn!(
                silent_ms = now_ms.saturating_sub(self.life_ms),
                state = ?self.state,
                "recorder: the worker stopped answering, so it is asked to stop"
            );
            self.asked_ms = Some(now_ms);
            return Some(SupervisorAction::Kill { hard: false });
        }
        None
    }

    /// The current worker ended with `code` (None: a signal ended it).
    pub fn on_exit(&mut self, code: Option<i32>, now_ms: u64) -> SupervisorAction {
        self.exited = true;
        if self.finished || code == Some(3) {
            return SupervisorAction::Ended { failed: None };
        }
        if self.stopping {
            return if self.started {
                SupervisorAction::Recover
            } else {
                SupervisorAction::Ended { failed: None }
            };
        }
        if self.started {
            while (self.resumes.front())
                .is_some_and(|&at| now_ms.saturating_sub(at) >= RESUME_WINDOW_MS)
            {
                self.resumes.pop_front();
            }
            if self.resumes.len() < RESUMES {
                self.resumes.push_back(now_ms);
                return SupervisorAction::Resume;
            }
            return SupervisorAction::GiveUp;
        }
        let failed = (!self.said_failed).then(|| {
            CodedError::new(if code == Some(2) {
                "capture.error.unsupported"
            } else {
                "capture.error.os"
            })
        });
        SupervisorAction::Ended { failed }
    }

    /// A resumed worker took over: its own signs of life from `now_ms`.
    pub fn respawned(&mut self, now_ms: u64) {
        self.state = None;
        self.life_ms = now_ms;
        self.spoke = false;
        self.asked_ms = None;
        self.killed = false;
        self.exited = false;
        self.said_failed = false;
    }
}
