//! The take's cursor in the core: its events into the writer on the session clock, and
//! the watchdog on the core's poll (ten times a second) and its tick. The burn-in and the cursor's notes are
//! `cursor_burn`'s.

use klipolo_cursor::ClickSource;
use klipolo_model::time::Us;

use super::core::SessionCore;
use super::cursor_burn::Burn;
use super::effect::Effect;
use super::resume::ResumedCursor;
use super::source::Status;
use crate::cursor::{CursorEvent, CursorWriter, Watch};
use crate::journal::Entry;
use crate::preflight::cursor::CursorPlan;
use crate::session::{SessionConfig, SourceSpec};
use crate::shell::WorkerState;

/// The take's cursor: its screen source, plan, writer and watchdog.
#[derive(Clone, Debug)]
pub(super) struct CursorState {
    /// The screen source whose cursor is recorded (its plan isn't None), by index.
    pub(super) src: Option<usize>,
    pub(super) plan: CursorPlan,
    pub(super) writer: CursorWriter,
    pub(super) watch: Watch,
    /// The clicks the take starts with (`CursorCaps::clicks`).
    pub(super) clicks: ClickSource,
    pub(super) burn: Burn,
    /// A resumed take journaled `CursorBurned` at this time, but its file has no `x` line: written at the opening.
    pub(super) burned_at: Option<Us>,
}

impl CursorState {
    pub(super) fn of(cfg: &SessionConfig) -> Self {
        let screen = cfg
            .sources
            .iter()
            .enumerate()
            .find_map(|(i, s)| match &s.spec {
                SourceSpec::Screen(v) if v.cursor != CursorPlan::None => Some((i, v.cursor)),
                _ => None,
            });
        let plan = screen.map_or(CursorPlan::None, |(_, plan)| plan);
        Self {
            src: screen.map(|(i, _)| i),
            plan,
            writer: CursorWriter::new(),
            watch: Watch::new(plan),
            clicks: ClickSource::Sampled,
            burn: Burn::None,
            burned_at: None,
        }
    }
}

impl SessionCore {
    /// The clicks this computer's cursor gives (`CursorCaps::clicks`), for the file's header. Default: sampled.
    pub fn with_cursor_clicks(mut self, clicks: ClickSource) -> Self {
        self.cursor.clicks = clicks;
        self
    }

    /// The watchdog, for tests and resume.
    pub fn cursor_watch(&self) -> &Watch {
        &self.cursor.watch
    }

    /// How the screen records the cursor now: the config's plan, or Burned after a burn-in (live, healed or resumed).
    pub fn cursor_plan(&self) -> CursorPlan {
        self.cursor.plan
    }

    /// The config the pipes are built from: the screen's plan as it is now (a resumed take that burned in starts its
    /// screen with the cursor in its frames).
    pub fn pipe_config(&self) -> SessionConfig {
        let mut cfg = self.cfg.clone();
        if let Some(SourceSpec::Screen(v)) = self.cursor.src.map(|i| &mut cfg.sources[i].spec) {
            v.cursor = self.cursor.plan;
        }
        cfg
    }

    /// One cursor event, arriving at `now_ns`. Its time is its own stamp (never later than `now_ns`) on the session
    /// clock; before the gate and while paused it is dropped (an area is kept for the header).
    pub fn cursor(&mut self, ev: CursorEvent, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        let ended = matches!(
            self.state,
            WorkerState::Stopping | WorkerState::Ready | WorkerState::Failed
        );
        if self.cursor.src.is_none() || ended {
            return fx;
        }
        if ev == CursorEvent::Failed {
            tracing::warn!("cursor: the source failed");
            let action = self.cursor.watch.on_failed();
            self.watch_action(action, now_ns, &mut fx);
            return fx;
        }
        self.cursor.watch.on_event(now_ns);
        let t = ev
            .host_ns()
            .and_then(|h| self.clock.session_us(h.min(now_ns)));
        let note = self.cursor.writer.event(&ev, t, &mut fx);
        let at_us = t.unwrap_or_else(|| self.clock.elapsed_us(now_ns));
        self.cursor_noted(note, at_us, &mut fx);
        fx
    }

    /// The core's poll, ten times a second: the watchdog, and the cursor's lines that left the reorder window.
    pub fn poll(&mut self, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.cursor_tick(now_ns, false, &mut fx);
        fx
    }

    /// At the gate (or a resumed take's opening): the file opened (its header unless it has one), the click mode
    /// journaled with a new header, and the watchdog's clock started.
    pub(super) fn cursor_open(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if self.cursor.src.is_none() {
            return;
        }
        let fresh = !self.cursor.writer.has_header();
        let (plan, clicks) = (self.cursor.plan, self.cursor.clicks);
        self.cursor.writer.open(plan, clicks, (0.0, 0.0), fx);
        if fresh {
            let at_us = self.clock.elapsed_us(now);
            fx.push(Effect::Journal(Entry::CursorClicks { at_us, clicks }));
        }
        // What the source said before the gate (its shape, a hidden cursor, sampled clicks) shows from here.
        let at_us = self.clock.elapsed_us(now);
        let note = self.cursor.writer.restate(at_us);
        self.cursor_noted(note, at_us, fx);
        if let Some(at) = self.cursor.burned_at.take() {
            self.cursor.writer.burned(at, fx);
        }
        self.cursor.watch.start(now);
    }

    /// A resumed take's cursor (`run::resume`): the rebuilt writer, and a burn-in the journal holds.
    pub(super) fn resume_cursor(&mut self, from: ResumedCursor) {
        if let Some(writer) = from.writer {
            self.cursor.writer = writer;
        }
        if !from.decided || self.cursor.src.is_none() {
            return;
        }
        self.cursor.plan = CursorPlan::Burned;
        self.cursor.watch = Watch::resumed_burned();
        self.cursor.burn = match from.burned {
            Some(at) => {
                self.cursor.burned_at = (!from.file_burned).then_some(at);
                Burn::Done
            }
            None => Burn::AwaitRun,
        };
    }

    /// The poll's and the tick's part: the writer's gather (always at the tick, before its flush) and rest point, then
    /// the watchdog.
    pub(super) fn cursor_tick(&mut self, now: u64, tick: bool, fx: &mut Vec<Effect>) {
        let Some(i) = self.cursor.src else {
            return;
        };
        if !matches!(self.state, WorkerState::Recording | WorkerState::Paused) {
            return;
        }
        let at = self.clock.elapsed_us(now);
        self.cursor.writer.tick(at, tick, fx);
        let live = self.state == WorkerState::Recording
            && self.srcs[i].status == Status::Live
            && self.display_awake;
        let action = self.cursor.watch.tick(now, live);
        self.watch_action(action, now, fx);
    }

    /// A pause releases the buttons still down (their ups would come while nothing is written) and flushes
    /// what waits; its end writes what the source said meanwhile. The watchdog doesn't count paused time.
    pub(super) fn cursor_paused(&mut self, paused: bool, now: u64, fx: &mut Vec<Effect>) {
        let at = self.clock.elapsed_us(now);
        if paused {
            self.cursor.writer.release(at);
            self.cursor.writer.flush(fx);
        } else {
            let note = self.cursor.writer.restate(at);
            self.cursor_noted(note, at, fx);
        }
        self.cursor.watch.set_paused(paused, now);
    }

    /// Stop: the buttons still down released, and everything appended; lines that came too late for the window are
    /// logged once (the reorder window is tuned by it).
    pub(super) fn cursor_stop(&mut self, now: u64, fx: &mut Vec<Effect>) {
        self.cursor.writer.release(self.clock.elapsed_us(now));
        self.cursor.writer.flush(fx);
        let late = self.cursor.writer.late();
        if late > 0 {
            tracing::warn!(
                late,
                "cursor: lines that came later than the reorder window were dropped"
            );
        }
    }
}
