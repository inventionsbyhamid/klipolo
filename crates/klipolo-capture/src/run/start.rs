//! The core's start and stop: Record starts the pipes; once all have started, the countdown runs while the start gate
//! waits for real samples from each source (a gate question, a mic rule firing or a system alert on the screen holds
//! it), and at T0 the session clock starts. Stop is idempotent: before the gate it leaves no take (exit 3); after it
//! every run is ended and the files are finished (`Finalize`). Also the core's small helpers: events, notes and pipe
//! commands.

use klipolo_model::coded::CodedError;
use klipolo_model::project::MarkerKind;

use super::core::SessionCore;
use super::effect::{Effect, PipeCmd};
use super::source::Status;
use crate::gate::{GateState, StartGate};
use crate::journal::Entry;
use crate::media::SourceId;
use crate::protocol::Event;
use crate::shell::{Flash, Level, ShellOutput, WorkerState};

impl SessionCore {
    pub(super) fn start(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if self.state != WorkerState::Armed {
            return;
        }
        fx.push(Effect::Journal(Entry::Started { host_ns: now }));
        fx.push(Effect::StartPipes(self.ids()));
        self.state = WorkerState::Starting;
        self.emit_state(now, fx);
    }

    /// Once every pipe started or failed: the rules' clock starts, the gate is made, the countdown begins.
    pub(super) fn maybe_count_down(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if self.state != WorkerState::Starting
            || self.srcs.iter().any(|s| s.status == Status::Starting)
        {
            return;
        }
        self.rules_clock.start(now);
        // A source that failed to start isn't waited for: its error was noted as it failed.
        let ids: Vec<SourceId> = self
            .srcs
            .iter()
            .filter(|s| !matches!(s.status, Status::Left | Status::Failed))
            .map(|s| s.id.clone())
            .collect();
        self.gate = Some(StartGate::new(&ids, now));
        for src in &mut self.srcs {
            src.rules_for_start(&self.mics);
        }
        self.state = WorkerState::Countdown;
        self.emit_state(now, fx);
        self.advance(now, fx);
    }

    /// The time-driven steps, run before every input: the countdown and the gate, and the focus debounce.
    pub(super) fn advance(&mut self, now: u64, fx: &mut Vec<Effect>) {
        match self.state {
            WorkerState::Countdown => self.countdown_step(now, fx),
            WorkerState::Recording => {
                self.focus_step(now, fx);
                self.mic_fallback_due(now, fx);
            }
            _ => {}
        }
    }

    pub(super) fn countdown_step(&mut self, now: u64, fx: &mut Vec<Effect>) {
        let Some(gate) = &self.gate else { return };
        let c = &mut self.countdown;
        match gate.poll(now) {
            GateState::Open => {
                c.gate_open_at.get_or_insert(now);
                c.asked = None;
            }
            GateState::Ask { missing } => {
                if c.asked.as_ref() != Some(&missing) {
                    fx.push(Effect::Emit(Event::Gate {
                        missing: missing.clone(),
                    }));
                    c.asked = Some(missing);
                }
            }
            GateState::Waiting { .. } => {}
        }
        let held = c.asked.is_some() || c.held_by.is_some() || c.alert;
        let total = u64::from(self.cfg.countdown_s) * 1_000_000_000;
        if held {
            if let Some(from) = c.running_since.take() {
                c.done_ns += now.saturating_sub(from);
            }
        } else if c.running_since.is_none() && c.done_ns < total {
            c.running_since = Some(now);
        }
        let done = c.done_ns + c.running_since.map_or(0, |from| now.saturating_sub(from));
        let left_ns = total.saturating_sub(done);
        let left_s = u32::try_from(left_ns.div_ceil(1_000_000_000)).unwrap_or(u32::MAX);
        if c.shown != Some(left_s) || c.held_shown != held {
            (c.shown, c.held_shown) = (Some(left_s), held);
            fx.push(Effect::Emit(Event::Countdown { left_s }));
            fx.push(self.status(now));
        }
        let c = &self.countdown;
        if let (0, false, Some(open_at)) = (left_ns, held, c.gate_open_at) {
            // T0: when both the countdown and the gate were done, exactly where the countdown ran out.
            let t0 = match c.running_since {
                Some(from) => (from + total - c.done_ns).max(open_at),
                None => now,
            };
            self.open_gate(t0, now, fx);
        }
    }

    pub(super) fn open_gate(&mut self, t0: u64, now: u64, fx: &mut Vec<Effect>) {
        self.clock.start(t0);
        self.countdown.running_since = None;
        self.read_back_at = Some(crate::readback::AFTER_US);
        fx.push(Effect::Journal(Entry::Gate { host_ns: t0 }));
        for i in 0..self.srcs.len() {
            let src = &mut self.srcs[i];
            match src.status {
                Status::Failed => src.status = Status::Left,
                Status::Live if src.is_video() => {
                    src.key_asked = true;
                    fx.push(self.pipe(i, PipeCmd::Keyframe));
                }
                _ => {}
            }
        }
        self.state = WorkerState::Recording;
        self.cursor_open(t0, fx);
        self.emit_state(now, fx);
    }

    /// "Record without it" during the countdown: the gate stops waiting for `source`, and its pipe stops.
    pub(super) fn without(&mut self, source: &str, now: u64, fx: &mut Vec<Effect>) {
        let Some(i) = self.index(source) else { return };
        if !matches!(self.state, WorkerState::Starting | WorkerState::Countdown) {
            return;
        }
        self.srcs[i].status = Status::Left;
        self.leave_gate(source);
        fx.push(self.pipe(i, PipeCmd::Stop));
        if self.srcs.iter().all(|s| s.status == Status::Left) {
            self.end_without_a_take(now, fx);
            return;
        }
        self.maybe_count_down(now, fx);
        self.advance(now, fx);
    }

    /// The start gate stops waiting for `source` (left out, or failed to start), and nothing holds the countdown for it.
    pub(super) fn leave_gate(&mut self, source: &str) {
        if let Some(gate) = &mut self.gate {
            gate.without(source);
        }
        if self.countdown.held_by.as_deref() == Some(source) {
            self.countdown.held_by = None;
        }
        self.countdown.asked = None;
    }

    /// Whether the threads layer reads the system's alerts (`Backend::system_alert`): from the start until the gate.
    pub fn watches_system_alerts(&self) -> bool {
        matches!(self.state, WorkerState::Starting | WorkerState::Countdown)
    }

    /// Whether a system alert is on the screen now (`up`), as the threads layer reads it from the start until the gate
    /// (macOS's own question, such as the one a take's first stream can raise about the private window picker). While
    /// one is up the countdown waits; once it goes the countdown starts again from the top, so the person has the whole
    /// count after answering it and the question is never in the recording. Each change is said once
    /// (`Event::SystemAlert`), and the HUD says why it waits. Any other time it changes nothing.
    pub fn system_alert(&mut self, up: bool, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.advance(now_ns, &mut fx);
        if !self.watches_system_alerts() || self.countdown.alert == up {
            return fx;
        }
        self.countdown.alert = up;
        fx.push(Effect::Emit(Event::SystemAlert { up }));
        if !up {
            let c = &mut self.countdown;
            (c.done_ns, c.running_since) = (0, None);
        }
        // A countdown held or let go says so (its number and the status); one still starting, once it begins.
        self.advance(now_ns, &mut fx);
        fx
    }

    /// Stop from any state (idempotent). Before the gate nothing was written: exit 3, no take. After it: every run
    /// ended, `StopRequested` and `Finalize`.
    pub(super) fn stop(&mut self, now: u64, reason: Option<&'static str>, fx: &mut Vec<Effect>) {
        match self.state {
            WorkerState::Armed | WorkerState::Starting | WorkerState::Countdown => {
                self.end_without_a_take(now, fx)
            }
            WorkerState::Recording | WorkerState::Paused => {
                fx.push(Effect::StopPipes);
                let at_us = self.clock.elapsed_us(now);
                fx.push(Effect::Journal(Entry::StopRequested {
                    at_us,
                    reason: reason.map(str::to_owned),
                }));
                for i in 0..self.srcs.len() {
                    self.end_run(i, fx);
                }
                self.cursor_stop(now, fx);
                self.silent_sound_notes(fx);
                self.state = WorkerState::Stopping;
                fx.push(Effect::Shell(ShellOutput::Keys(false)));
                self.emit_state(now, fx);
                fx.push(Effect::Finalize);
            }
            _ => {}
        }
    }

    /// The person's Start over: while recording or paused, `StartOver` tells the app this take goes to the
    /// Trash once its files are finished, then Stop finishes them. Refused in any other state: before the gate there is
    /// no take to start over (the countdown's Cancel is there), and once Stopping it is saving.
    pub(super) fn start_over(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if !matches!(self.state, WorkerState::Recording | WorkerState::Paused) {
            tracing::info!(state = ?self.state, "core: Start over isn't taken in this state");
            return;
        }
        fx.push(Effect::Emit(Event::StartOver));
        self.stop(now, None, fx);
    }

    /// A stop the take asked for itself (`capture.stop.*`): noted for the person, then Stop.
    pub(super) fn stop_and_save(&mut self, code: &'static str, now: u64, fx: &mut Vec<Effect>) {
        if !matches!(self.state, WorkerState::Recording | WorkerState::Paused) {
            if matches!(self.state, WorkerState::Starting | WorkerState::Countdown) {
                self.end_without_a_take(now, fx);
            }
            return;
        }
        let note = CodedError::new(code);
        self.note(note, Level::Critical, None, true, Some(Flash::Problem), fx);
        self.stop(now, Some(code), fx);
    }

    /// No take: every pipe stops, the hotkeys go, and the worker exits 3 (the threads layer removes the empty folder).
    pub(super) fn end_without_a_take(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if matches!(
            self.state,
            WorkerState::Stopping | WorkerState::Ready | WorkerState::Failed
        ) {
            return;
        }
        fx.push(Effect::StopPipes);
        fx.push(Effect::Shell(ShellOutput::Keys(false)));
        self.state = WorkerState::Stopping;
        self.emit_state(now, fx);
        fx.push(Effect::Shell(ShellOutput::Quit));
        fx.push(Effect::Exit(3));
    }

    pub(super) fn marker(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if !matches!(self.state, WorkerState::Recording | WorkerState::Paused) {
            return;
        }
        self.markers += 1;
        fx.push(Effect::Journal(Entry::Marker {
            id: format!("m{}", self.markers),
            at_us: self.clock.elapsed_us(now),
            kind: MarkerKind::Hotkey,
        }));
        fx.push(Effect::Shell(ShellOutput::Flash(Flash::Marker)));
    }

    pub(super) fn emit_state(&self, now: u64, fx: &mut Vec<Effect>) {
        let elapsed_us = self.clock.elapsed_us(now);
        fx.push(Effect::Emit(Event::State {
            state: self.state,
            elapsed_us,
        }));
        fx.push(self.status(now));
    }

    /// A note for the person: an event, a journal line (`critical`: it reaches the manifest's notes, shown after the
    /// take), and a HUD flash if asked.
    pub(super) fn note(
        &self,
        note: CodedError,
        level: Level,
        source: Option<&str>,
        critical: bool,
        flash: Option<Flash>,
        fx: &mut Vec<Effect>,
    ) {
        fx.push(Effect::Journal(Entry::health(
            &note,
            source.map(str::to_owned),
            critical,
        )));
        fx.push(Effect::Emit(Event::Health {
            note,
            level,
            source: source.map(str::to_owned),
        }));
        if let Some(flash) = flash {
            fx.push(Effect::Shell(ShellOutput::Flash(flash)));
        }
    }

    pub(super) fn pipe(&self, i: usize, cmd: PipeCmd) -> Effect {
        Effect::Pipe {
            source: self.srcs[i].id.clone(),
            cmd,
        }
    }

    pub(super) fn index(&self, source: &str) -> Option<usize> {
        self.srcs.iter().position(|s| s.id == source)
    }

    pub(super) fn ids(&self) -> Vec<SourceId> {
        self.srcs
            .iter()
            .filter(|s| s.status != Status::Left)
            .map(|s| s.id.clone())
            .collect()
    }
}
