//! The take's cursor burned into the frames, and its notes (split from `cursor_in`).
//!
//! **The burn-in.** When the watchdog decides: `CursorBurnDecided` is journaled, the writer flushes (the buttons still
//! down released) and the screen's pipe is asked to show the cursor (`PipeCmd::CursorInFrames`). Until the pipe
//! answers, the file has no `x` line, so the editor holds the last cursor: never a moment with neither.
//! The pipe's Ok gives the `x` line, `CursorBurned` and the note at its time; its Err heals: the screen restarts
//! with the cursor in its frames (a new run, gap ≤ 3 s; one per take, so outside the restart budget: a
//! cursor problem never costs the screen), and that run's first frame gives them. It never switches back.

use klipolo_cursor::ClickSource;
use klipolo_model::time::Us;

use super::core::SessionCore;
use super::effect::{Effect, PipeCmd};
use super::source::Status;
use crate::cursor::{CursorNote, WatchAction};
use crate::english::msg;
use crate::journal::Entry;
use crate::preflight::cursor::CursorPlan;
use crate::shell::{Level, WorkerState};

/// Where the burn-in is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Burn {
    None,
    /// Decided; the pipe was asked.
    Decided,
    /// The pipe couldn't switch live: the screen's next run shows the cursor from its first frame.
    AwaitRun,
    Done,
}

impl SessionCore {
    /// What the writer tells, at `at_us`: a switch to sampled clicks is journaled (the manifest takes the weakest) and
    /// noted; a full shape store is noted.
    pub(super) fn cursor_noted(
        &mut self,
        note: Option<CursorNote>,
        at_us: Us,
        fx: &mut Vec<Effect>,
    ) {
        match note {
            Some(CursorNote::Sampled) => {
                let clicks = ClickSource::Sampled;
                fx.push(Effect::Journal(Entry::CursorClicks { at_us, clicks }));
                self.cursor_note(msg!("capture.note.cursorSampled", atUs = at_us), fx);
            }
            Some(CursorNote::ShapesFull) => self.cursor_note(msg!("capture.note.cursorShapes"), fx),
            None => {}
        }
    }

    /// The screen pipe's answer to `CursorInFrames`: Ok at `host_ns` (the stream confirmed it), else heal.
    pub fn cursor_in_frames(&mut self, ok: bool, host_ns: u64, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(i) = self
            .cursor
            .src
            .filter(|_| self.cursor.burn == Burn::Decided)
        else {
            return fx;
        };
        if !matches!(self.state, WorkerState::Recording | WorkerState::Paused) {
            return fx;
        }
        if ok {
            let at = self.clock.session_us(host_ns.min(now_ns));
            let at = at.unwrap_or_else(|| self.clock.elapsed_us(now_ns));
            self.cursor_burned(at, &mut fx);
            return fx;
        }
        tracing::warn!("cursor: the screen couldn't show the cursor live, so it restarts with it");
        (self.cursor.plan, self.cursor.burn) = (CursorPlan::Burned, Burn::AwaitRun);
        // Never a restart while the display sleeps (the heal rule): it restarts once the display wakes.
        if self.srcs[i].status == Status::Live && self.display_awake {
            self.restart(i, PipeCmd::Restart, now_ns, &mut fx);
        } else if self.srcs[i].status == Status::Live {
            self.srcs[i].wake_restart = true;
        }
        fx
    }

    /// Source `i` opened a run at `t`: a healed (or resumed) screen's first frame shows the cursor.
    pub(super) fn cursor_run_opened(&mut self, i: usize, t: Us, fx: &mut Vec<Effect>) {
        if self.cursor.src == Some(i) && self.cursor.burn == Burn::AwaitRun {
            self.cursor_burned(t, fx);
        }
    }

    pub(super) fn watch_action(&mut self, action: WatchAction, now: u64, fx: &mut Vec<Effect>) {
        let (WatchAction::BurnIn(reason), Some(i)) = (action, self.cursor.src) else {
            return;
        };
        tracing::warn!(
            ?reason,
            "cursor: lost the cursor's source, so it is burned into the frames"
        );
        let at_us = self.clock.elapsed_us(now);
        fx.push(Effect::Journal(Entry::CursorBurnDecided { at_us }));
        self.cursor.writer.release(at_us);
        self.cursor.writer.flush(fx);
        self.cursor.burn = Burn::Decided;
        fx.push(self.pipe(i, PipeCmd::CursorInFrames));
    }

    /// The frames show the cursor from `at`: the `x` line, `CursorBurned` and the note.
    fn cursor_burned(&mut self, at: Us, fx: &mut Vec<Effect>) {
        self.cursor.writer.burned(at, fx);
        fx.push(Effect::Journal(Entry::CursorBurned { at_us: at }));
        (self.cursor.plan, self.cursor.burn) = (CursorPlan::Burned, Burn::Done);
        self.cursor.watch.set_burned();
        self.cursor_note(msg!("capture.note.cursorBurned", atUs = at), fx);
    }

    /// A note about the cursor, kept for after the take (the manifest's notes), on the screen source.
    fn cursor_note(&self, note: klipolo_model::coded::CodedError, fx: &mut Vec<Effect>) {
        let source = self.cursor.src.map(|i| self.srcs[i].id.clone());
        self.note(note, Level::Ok, source.as_deref(), true, None, fx);
    }
}
