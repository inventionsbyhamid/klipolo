//! What the shell reports (keys, the menu, power, focus, the game quitting, the parent going), and pause and resume.

use klipolo_platform::power::Thermal;

use super::core::{PauseWhy, SessionCore};
use super::effect::{Effect, PipeCmd};
use super::source::Status;
use crate::english::msg;
use crate::heal::SourceKind;
use crate::journal::Entry;
use crate::pipe::PipeEvent;
use crate::shell::{Flash, Level, PowerEvent, ShellInput, ShellOutput, WorkerState};

/// Focus elsewhere this long pauses a game's take.
pub const FOCUS_NS: u64 = 500_000_000;
/// A sleep longer than this stops and saves the take.
pub const LONG_SLEEP_MS: u64 = 10 * 60 * 1_000;

impl SessionCore {
    pub(super) fn shell_input(&mut self, input: ShellInput, now: u64, fx: &mut Vec<Effect>) {
        use crate::shell::{HotkeyAction, MenuAction};
        match input {
            ShellInput::Hotkey(HotkeyAction::Pause) if self.state == WorkerState::Paused => {
                self.resume_take(now, fx)
            }
            ShellInput::Hotkey(HotkeyAction::Pause) | ShellInput::Menu(MenuAction::Pause) => {
                self.pause_take(now, PauseWhy::Person, fx)
            }
            ShellInput::Menu(MenuAction::Resume) => self.resume_take(now, fx),
            ShellInput::Hotkey(HotkeyAction::Marker) | ShellInput::Menu(MenuAction::Marker) => {
                self.marker(now, fx)
            }
            ShellInput::Menu(MenuAction::Stop) | ShellInput::Hotkey(HotkeyAction::StartStop) => {
                self.stop(now, None, fx)
            }
            ShellInput::Menu(MenuAction::StartOver)
            | ShellInput::Hotkey(HotkeyAction::StartOver) => self.start_over(now, fx),
            ShellInput::Power(event) => self.power(event, now, fx),
            ShellInput::Focus { app_id } => self.focus(app_id.as_deref(), now, fx),
            ShellInput::AppQuit { app_id } => {
                self.game_event(&app_id, PipeEvent::TargetGone, now, fx)
            }
            ShellInput::AppLaunched { app_id } => {
                self.game_event(&app_id, PipeEvent::TargetBack, now, fx)
            }
            ShellInput::ParentGone => {
                self.orphan = true;
                fx.push(self.status(now));
            }
        }
    }

    pub(super) fn power(&mut self, event: PowerEvent, now: u64, fx: &mut Vec<Effect>) {
        match event {
            PowerEvent::WillSleep => self.pause_take(now, PauseWhy::Sleep, fx),
            PowerEvent::Woke { slept_ms } if slept_ms > LONG_SLEEP_MS => {
                self.stop_and_save("capture.stop.slept", now, fx)
            }
            // Short sleeps and an unlock stay paused and ask: "Resume" or "Stop and save".
            PowerEvent::Woke { .. } if self.pause == Some(PauseWhy::Sleep) => {
                self.health_coded(msg!("capture.paused.sleep"), Level::Warning, fx)
            }
            PowerEvent::Locked => self.pause_take(now, PauseWhy::Lock, fx),
            PowerEvent::Unlocked if self.pause == Some(PauseWhy::Lock) => {
                self.health_coded(msg!("capture.paused.lock"), Level::Warning, fx)
            }
            PowerEvent::PowerOff => self.stop(now, None, fx),
            PowerEvent::Thermal(Thermal::Serious | Thermal::Critical) if !self.hot => {
                self.hot = true;
                self.health_coded(msg!("capture.health.hot"), Level::Warning, fx);
            }
            _ => {}
        }
    }

    /// Focus while a game's take auto-pauses: elsewhere for 500 ms pauses, and the game's return resumes a pause it
    /// made (only that one).
    pub(super) fn focus(&mut self, app_id: Option<&str>, now: u64, fx: &mut Vec<Effect>) {
        let Some(game) = self.cfg.auto_pause.as_deref() else {
            return;
        };
        if app_id == Some(game) {
            self.focus_away = None;
            if self.pause == Some(PauseWhy::Focus) {
                self.resume_take(now, fx);
            }
        } else if self.game_running && self.state == WorkerState::Recording {
            self.focus_away.get_or_insert(now);
        }
    }

    pub(super) fn focus_step(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if let Some(away) = self.focus_away.filter(|&away| now >= away + FOCUS_NS) {
            self.focus_away = None;
            self.pause_take(away + FOCUS_NS, PauseWhy::Focus, fx);
        }
    }

    /// The recorded game quit or came back: its screen source waits for it, the rest record on.
    pub(super) fn game_event(
        &mut self,
        app_id: &str,
        event: PipeEvent,
        now: u64,
        fx: &mut Vec<Effect>,
    ) {
        let game = self
            .cfg
            .game
            .as_ref()
            .map(|g| g.app_id.as_str())
            .or(self.cfg.auto_pause.as_deref());
        if game != Some(app_id) {
            return;
        }
        self.game_running = event == PipeEvent::TargetBack;
        self.focus_away = None;
        if self.state == WorkerState::Paused
            && self.pause == Some(PauseWhy::Focus)
            && !self.game_running
        {
            self.resume_take(now, fx);
        }
        if !matches!(self.state, WorkerState::Recording | WorkerState::Paused) {
            return;
        }
        for i in 0..self.srcs.len() {
            let waiting = self.srcs[i].status == Status::WaitingTarget;
            let wanted = match event {
                PipeEvent::TargetBack => waiting,
                _ => !waiting && self.srcs[i].status != Status::Stopped,
            };
            if self.srcs[i].kind == SourceKind::Screen && wanted {
                self.heal_event(i, &event, now, fx);
            }
        }
    }

    /// Pause (never a new run): the clock stops at `h`, never before a sample already written.
    pub(super) fn pause_take(&mut self, h: u64, why: PauseWhy, fx: &mut Vec<Effect>) {
        if self.state != WorkerState::Recording {
            return;
        }
        let h = h.max(self.mapped_until);
        self.clock.pause(h);
        self.rules_clock.pause(h);
        self.state = WorkerState::Paused;
        self.pause = Some(why);
        self.flush_gathered(fx);
        self.cursor_paused(true, h, fx);
        fx.push(Effect::Journal(Entry::Pause {
            at_us: self.clock.elapsed_us(h),
            host_ns: h,
        }));
        for i in 0..self.srcs.len() {
            fx.push(self.pipe(i, PipeCmd::Pause));
        }
        fx.push(Effect::Shell(ShellOutput::Flash(Flash::Paused)));
        let note = match why {
            PauseWhy::Person => None,
            PauseWhy::Focus => {
                let app = self.cfg.game.as_ref().map_or("", |g| g.name.as_str());
                Some(msg!("capture.paused.focus", app = app))
            }
            PauseWhy::Sleep => Some(msg!("capture.paused.sleep")),
            PauseWhy::Lock => Some(msg!("capture.paused.lock")),
        };
        if let Some(note) = note {
            self.health_coded(note, Level::Warning, fx);
        }
        self.emit_state(h, fx);
    }

    pub(super) fn resume_take(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if self.state != WorkerState::Paused {
            return;
        }
        self.clock.resume(now);
        self.rules_clock.resume(now);
        self.state = WorkerState::Recording;
        self.pause = None;
        self.cursor_paused(false, now, fx);
        fx.push(Effect::Journal(Entry::Resume {
            at_us: self.clock.elapsed_us(now),
            host_ns: now,
        }));
        for i in 0..self.srcs.len() {
            fx.push(self.pipe(i, PipeCmd::Resume));
            if self.srcs[i].is_video() {
                fx.push(self.pipe(i, PipeCmd::Keyframe));
                self.await_keyframe(i);
            }
        }
        self.wake_restarts(now, fx);
        fx.push(Effect::Shell(ShellOutput::Flash(Flash::Resumed)));
        self.emit_state(now, fx);
    }
}
