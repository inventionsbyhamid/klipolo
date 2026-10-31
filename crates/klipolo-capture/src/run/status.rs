//! The recorder's item: its title's state and time and its menu's lines, one per source and the space left, in the
//! person's words (`shell_view::Words`, from the session's; this crate's English for a code they don't word), and how
//! the take is doing (its colour); and for the HUD, the countdown's seconds, why it is held, and the mic's level.

use klipolo_model::coded::CodedError;

use super::core::SessionCore;
use super::effect::Effect;
use super::source::Status;
use crate::disk::DiskLevel;
use crate::english::{hours_minutes, msg};
use crate::heal::SourceKind;
use crate::health::dbfs;
use crate::manifest::source_param;
use crate::media::SourceId;
use crate::shell::{Level, ShellOutput, StatusView, WorkerState};

impl SessionCore {
    /// The item's title and menu, in the person's words.
    pub(super) fn status(&self, now: u64) -> Effect {
        let running = matches!(self.state, WorkerState::Recording | WorkerState::Paused);
        let mut lines = Vec::with_capacity(self.srcs.len() + 2);
        let mut level = Level::Ok;
        for src in self.srcs.iter().filter(|s| s.status != Status::Left) {
            let source = source_param(&src.label);
            let (line, worse) = match src.status {
                Status::Live if src.is_video() && running => (
                    msg!(
                        "capture.menu.fps",
                        source = source,
                        fps = src.fps_seen as usize
                    ),
                    Level::Ok,
                ),
                Status::Live | Status::Starting if !running => {
                    (msg!("capture.menu.idle", source = source), Level::Ok)
                }
                Status::Live => (msg!("capture.menu.idle", source = source), Level::Ok),
                Status::Stopped | Status::Failed => {
                    (msg!("capture.health.lost", source = source), Level::Warning)
                }
                _ => (
                    msg!("capture.menu.reconnecting", source = source),
                    Level::Healing,
                ),
            };
            level = level.max_of(worse);
            lines.push(self.words.word(&line));
        }
        if let Some(minutes) = self.minutes_left.filter(|_| running) {
            let left = hours_minutes(u64::from(minutes));
            lines.push(self.words.word(&msg!("capture.menu.space", left = left)));
        }
        // Klipolo is gone and the take goes on (an armed take that isn't a game's was disarmed instead): stop it here.
        let goes_on = matches!(
            self.state,
            WorkerState::Armed
                | WorkerState::Starting
                | WorkerState::Countdown
                | WorkerState::Recording
                | WorkerState::Paused
        );
        if self.orphan && goes_on {
            lines.push(self.words.word(&msg!("capture.orphan")));
        }
        if self.disk == DiskLevel::Warn || self.slow {
            level = level.max_of(Level::Warning);
        }
        if self.read_back_bad {
            level = Level::Critical;
        }
        Effect::Shell(ShellOutput::Status(StatusView {
            state: self.state,
            elapsed_us: self.clock.elapsed_us(now),
            lines,
            level,
            countdown_s: self
                .countdown
                .shown
                .filter(|_| self.state == WorkerState::Countdown),
            mic_dbfs: self.mic_dbfs(now),
            held: self.held_reason().map(|reason| self.words.word(&reason)),
            by_key: self.by_key,
            bubble: self.bubble_shows(),
        }))
    }

    /// The camera bubble shows now (`shell_view::bubble_shows`): the take's camera source is live.
    fn bubble_shows(&self) -> bool {
        let camera_live = (self.srcs.iter())
            .any(|s| s.kind == SourceKind::Camera && s.status == Status::Live && !s.blank);
        crate::shell_view::bubble_shows(self.cfg.camera_bubble, self.state, camera_live)
    }

    /// The source the countdown's question names while it holds the countdown: the mic rule's that fired, else the
    /// first the gate still waits for (`held_reason` words it).
    pub(super) fn asked_about(&self) -> Option<SourceId> {
        let c = &self.countdown;
        if self.state != WorkerState::Countdown {
            return None;
        }
        (c.held_by.clone()).or_else(|| c.asked.as_ref()?.first().cloned())
    }

    /// Why the countdown is held, if it is: the mic rule that fired, else the first source the gate still waits for.
    fn held_reason(&self) -> Option<CodedError> {
        let c = &self.countdown;
        if self.state != WorkerState::Countdown {
            return None;
        }
        if c.held_by.is_some() {
            return c.held_note.clone();
        }
        let first = c.asked.as_ref()?.first()?;
        let src = self.srcs.iter().find(|s| &s.id == first)?;
        Some(msg!(
            "capture.gate.waiting",
            source = source_param(&src.label)
        ))
    }

    /// The loudest mic's recent peak in dBFS, for the HUD's meter: while counting down or recording, from each mic's
    /// rules (`MicRules::level`, the block peaks they already read). −120 is silence.
    fn mic_dbfs(&self, now: u64) -> Option<f32> {
        if !matches!(self.state, WorkerState::Countdown | WorkerState::Recording) {
            return None;
        }
        let at = self.rules_clock.elapsed_us(now);
        let peak = (self.srcs.iter())
            .filter_map(|s| s.mic_rules.as_ref())
            .map(|rules| rules.level(at))
            .reduce(f32::max)?;
        Some(dbfs(peak))
    }
}

trait Worst {
    fn max_of(self, other: Self) -> Self;
}

impl Worst for Level {
    fn max_of(self, other: Self) -> Self {
        let rank = |l: Level| match l {
            Level::Ok => 0,
            Level::Healing => 1,
            Level::Warning => 2,
            Level::Critical => 3,
        };
        if rank(other) > rank(self) {
            other
        } else {
            self
        }
    }
}
