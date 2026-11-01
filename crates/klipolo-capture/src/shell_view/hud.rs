//! The HUD's content and timing, shared by the shells that have one (`CaptureCaps::hud`: macOS, Windows). Hidden by
//! default, it flashes for 2 s on a marker, a pause, a resume, a problem or low space (`ShellOutput::Flash`). While a
//! take the person's start key began is counting down, it shows the seconds left (marked "⚠" while a problem
//! flashes), or why the countdown is held (`StatusView::held`: a muted mic, a source not sending) with how to answer
//! it (`answer`: the start key records without it, the item's Cancel ends it), then flashes as the
//! take begins: their editor is hidden, so it is all they see. It is redrawn at most twice a second, showing and
//! hiding excepted. Pure: every time is an argument; a shell draws what `frame` returns and asks again at `next_due`.

use std::time::{Duration, Instant};

use klipolo_model::coded::CodedError;

use super::{Words, clock};
use crate::backend::IndicatorPlace;
use crate::english::code;
use crate::shell::{Flash, StatusView, WorkerState};

/// How long a flash shows.
pub const FLASH_FOR: Duration = Duration::from_secs(2);
/// The HUD's contents change at most this often.
pub const REDRAW_EVERY: Duration = Duration::from_millis(500);
/// A mic peak at or below this (dBFS) shows an empty meter; 0 dBFS a full one.
const METER_FLOOR_DBFS: f32 = -60.0;

/// What the HUD shows.
#[derive(Clone, Debug, PartialEq)]
pub enum HudFrame {
    Hidden,
    Shown {
        /// "● REC 12:34", "❚❚ 12:34", "⚑ 12:34" (a marker), "⚠ 12:34" (a problem), or the countdown's "3".
        text: String,
        /// The mic's meter, 0-1; None without a mic.
        meter: Option<f32>,
        /// The countdown's seconds, drawn large.
        big: bool,
    },
}

/// The HUD's state between a shell's outputs.
#[derive(Debug)]
pub struct HudView {
    view: Option<StatusView>,
    /// The flash under way (None: the take beginning) and when it ends.
    flash: Option<(Option<Flash>, Instant)>,
    /// The person's start key began this take: its countdown shows.
    by_key: bool,
    drawn: HudFrame,
    drawn_at: Option<Instant>,
    /// A change the redraw rate held back.
    held: bool,
    /// How to answer the held countdown's question, in the shell's words (`gate_answer`).
    answer: Option<String>,
}

impl Default for HudView {
    fn default() -> Self {
        Self {
            view: None,
            flash: None,
            by_key: false,
            drawn: HudFrame::Hidden,
            drawn_at: None,
            held: false,
            answer: None,
        }
    }
}

impl HudView {
    /// The session's latest status: the take's state, length, countdown and mic level.
    pub fn status(&mut self, view: StatusView, now: Instant) {
        let was = self.view.as_ref().map(|v| v.state);
        // Start over's next take began without the start key, with the editor hidden as well.
        if view.by_key && matches!(view.state, WorkerState::Starting | WorkerState::Countdown) {
            self.by_key = true;
        }
        if self.by_key
            && was == Some(WorkerState::Countdown)
            && view.state == WorkerState::Recording
        {
            self.flash = Some((None, now + FLASH_FOR));
        }
        if !matches!(
            view.state,
            WorkerState::Armed | WorkerState::Starting | WorkerState::Countdown
        ) {
            self.by_key = false;
        }
        self.view = Some(view);
    }

    pub fn flash(&mut self, kind: Flash, now: Instant) {
        self.flash = Some((Some(kind), now + FLASH_FOR));
    }

    /// How to answer the countdown's question while it holds a take the start key began (`gate_answer`).
    pub fn answer(&mut self, line: String) {
        self.answer = Some(line);
    }

    /// The person pressed their start key: while armed, it begins the take, and the HUD shows the countdown.
    pub fn start_key(&mut self) {
        if (self.view.as_ref()).is_some_and(|v| v.state == WorkerState::Armed) {
            self.by_key = true;
        }
    }

    /// What to draw now, when it differs from what was drawn and the rate allows; None: nothing to draw.
    pub fn frame(&mut self, words: &Words, now: Instant) -> Option<HudFrame> {
        if self.flash.is_some_and(|(_, until)| until <= now) {
            self.flash = None;
        }
        let want = self.want(words, now);
        if want == self.drawn {
            self.held = false;
            return None;
        }
        let both_shown = matches!(
            (&want, &self.drawn),
            (HudFrame::Shown { .. }, HudFrame::Shown { .. })
        );
        if both_shown && self.drawn_at.is_some_and(|at| now < at + REDRAW_EVERY) {
            self.held = true;
            return None;
        }
        (self.drawn, self.drawn_at, self.held) = (want.clone(), Some(now), false);
        Some(want)
    }

    /// When to ask `frame` again: the flash's end, or a change the rate held back.
    pub fn next_due(&self) -> Option<Instant> {
        let ends = self.flash.map(|(_, until)| until);
        let held = (self.drawn_at.filter(|_| self.held)).map(|at| at + REDRAW_EVERY);
        ends.into_iter().chain(held).min()
    }

    fn want(&self, words: &Words, now: Instant) -> HudFrame {
        let Some(view) = &self.view else {
            return HudFrame::Hidden;
        };
        let meter =
            (view.mic_dbfs).map(|db| ((db - METER_FLOOR_DBFS) / -METER_FLOOR_DBFS).clamp(0.0, 1.0));
        let flashing = self
            .flash
            .filter(|&(_, until)| now < until)
            .map(|(kind, _)| kind);
        if self.by_key && matches!(view.state, WorkerState::Starting | WorkerState::Countdown) {
            // Held: why, never a digit that has stopped; a problem's flash marks the seconds.
            if let Some(reason) = &view.held {
                let text = match &self.answer {
                    Some(answer) => format!("⚠ {reason} {answer}"),
                    None => format!("⚠ {reason}"),
                };
                return HudFrame::Shown {
                    text,
                    meter,
                    big: false,
                };
            }
            let seconds = (view.countdown_s).map_or_else(|| "…".to_owned(), |s| s.to_string());
            let problem = matches!(flashing, Some(Some(Flash::Problem | Flash::LowSpace)));
            return HudFrame::Shown {
                text: if problem {
                    format!("⚠ {seconds}")
                } else {
                    seconds
                },
                meter,
                big: true,
            };
        }
        match self.flash {
            Some((kind, until)) if now < until => HudFrame::Shown {
                text: flash_text(kind, view, words),
                meter,
                big: false,
            },
            _ => HudFrame::Hidden,
        }
    }
}

/// The flash's line: a marker's or a problem's mark with the take's length, else the take's state.
fn flash_text(kind: Option<Flash>, view: &StatusView, words: &Words) -> String {
    let time = clock(view.elapsed_us);
    match (kind, view.state) {
        (Some(Flash::Marker), _) => format!("⚑ {time}"),
        (Some(Flash::Problem | Flash::LowSpace), _) => format!("⚠ {time}"),
        (_, WorkerState::Paused) => format!("❚❚ {time}"),
        (_, WorkerState::Recording) => words.say("capture.hud.recording", &[("time", time)]),
        _ => time,
    }
}

/// "Press {keys} to record without it, or Cancel from {place}.": the HUD's answers to the countdown's question in a take
/// the person's start key began. `keys`: the start key as this OS shows shortcuts; `place`: where the item is.
pub fn gate_answer(words: &Words, keys: &str, place: IndicatorPlace) -> String {
    let line = CodedError::new(code!("capture.hud.answer"))
        .with("keys", keys)
        .with("place", place.coded());
    words.word(&line)
}
