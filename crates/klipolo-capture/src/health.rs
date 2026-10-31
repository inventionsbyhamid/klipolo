//! Health while recording: the rules that keep a take from looking fine while a source is silent,
//! stalled or dropping frames. Taxonomy and cadence from Cap's `PipelineHealthEvent` (design only); thresholds are the
//! spec's. Pure: every time is an argument, so they are tested without threads.
//!
//! - **`MicRules`**, fed every mic block. A mic's silence has four patterns, each with its own note:
//!   - exact zeros for 2 s (a muted or blocked mic: macOS and the hardware mute deliver zeros): `micZeros` at once;
//!   - no peak above −70 dBFS for 10 s (the wrong input, a display's mic, gain at 0): `micQuiet`, naming the device;
//!   - no peak above −40 dBFS (speech) in the take's first 60 s: one soft `micNotHeard`, once per take, and not while
//!     one of the first two patterns holds (that note already speaks for it);
//!   - speech, then zeros: `micZeros` again, but only after 30 s of them (`GATED_ZEROS_US`). Gated virtual mics
//!     (Krisp, NVIDIA Broadcast) send exact zeros between phrases, so once the person has been heard a short silence
//!     isn't a muted mic. Before any speech, the 2 s rule stands.
//!
//!   A note is given once and again only after speech: a muted mic's stray ±1 LSB never re-arms it.
//! - **`VideoRules`**, once a second with a video pipe's counters: a stall (no buffer of any status and no answered
//!   nudge for 2 s while the display is awake, or no buffer at all for a minute), and more than 1 % of frames dropped
//!   over 10 s. One dropped frame is a glitch, not a warning.
//!
//! Notes are codes with params (`HealthNote`); the session journals them and words them for the shell.

mod meter;
mod video;

use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::time::Us;

use self::meter::Meter;
pub use self::video::{ANSWERS_ONLY_NS, VideoProblem, VideoRules};
use crate::english::{ENGLISH, code};
use crate::media::{PcmBlock, SourceId};
use crate::shell::Level;

/// A peak at or below this (−70 dBFS) is near-silence, below any working mic's noise floor.
pub const QUIET_PEAK: f32 = 0.000_316_227_8;
/// A peak above this (−40 dBFS) is speech.
pub const SPEECH_PEAK: f32 = 0.01;
pub const ZEROS_US: Us = 2_000_000;
/// Exact zeros after the person was heard: a gated mic's silence between phrases, unless it lasts this long.
pub const GATED_ZEROS_US: Us = 30_000_000;
pub const QUIET_US: Us = 10_000_000;
pub const NOT_HEARD_US: Us = 60_000_000;
/// No buffer of any status for this long, while the display is awake, is a stall.
pub const STALL_NS: u64 = 2_000_000_000;
/// Drops are counted over this window.
pub const DROP_WINDOW_NS: u64 = 10_000_000_000;
/// What the meters show for a peak of nothing (exact zeros), in dBFS.
pub const SILENCE_DBFS: f32 = -120.0;

/// A block's loudest sample (0-1 for a well-formed block): one pass over its samples, no allocation.
pub fn peak_of(block: &PcmBlock) -> f32 {
    block.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// A peak in dBFS, as the meters show it (the countdown's, the HUD's, the Record dialog's): `SILENCE_DBFS` for nothing.
pub fn dbfs(peak: f32) -> f32 {
    if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        SILENCE_DBFS
    }
}

/// Something the person should know about a take: a `capture.*` code, the source it is about, its params and its
/// weight (the item's colour).
#[derive(Clone, Debug, PartialEq)]
pub struct HealthNote {
    pub code: &'static str,
    pub source: Option<SourceId>,
    pub params: Vec<(&'static str, ErrorParam)>,
    pub level: Level,
}

impl HealthNote {
    pub fn coded(&self) -> CodedError {
        self.params
            .iter()
            .fold(CodedError::new(self.code), |e, (k, v)| e.with(k, v.clone()))
    }
}

/// A source's name in a message's `source` param: the nested `capture.source.<id>` code ("Screen", "Mic"), or the
/// id itself. The session swaps in the source's label where the person or the target named it (a game's screen
/// source is "Game").
pub fn source_param(source: &str) -> ErrorParam {
    ENGLISH
        .iter()
        .find(|(c, _)| c.strip_prefix("capture.source.") == Some(source))
        .map_or_else(
            || ErrorParam::from(source),
            |(c, _)| CodedError::new(c).into(),
        )
}

/// The silent-mic rules for one mic (the module doc). `at_us` and `now_us` are on one clock that doesn't run while
/// the take is paused, such as host time since the pipes started with paused time left out. A pattern lasts until
/// `now_us`, so a mic that stops sending blocks altogether is caught by the same rules. The rules' clock starts at the
/// first block or the first `check`, whichever comes first, so a mic that never sends one (a resumed take, a fallback
/// mic) is caught too.
#[derive(Clone, Debug)]
pub struct MicRules {
    source: SourceId,
    device: String,
    started_at: Option<Us>,
    /// The first block of the current run of exact zeros.
    zeros_since: Option<Us>,
    /// The last block peaking above −70 dBFS.
    audible_at: Option<Us>,
    heard: bool,
    zeros_given: bool,
    quiet_given: bool,
    not_heard_given: bool,
    meter: Meter,
}

impl MicRules {
    pub fn new(source: &str, device: &str) -> Self {
        Self {
            source: source.into(),
            device: device.into(),
            started_at: None,
            zeros_since: None,
            audible_at: None,
            heard: false,
            zeros_given: false,
            quiet_given: false,
            not_heard_given: false,
            meter: Meter::default(),
        }
    }

    /// The HUD's meter: the loudest sample (0-1) over about the last half second, as of `now_us`; 0 once no block has
    /// come for a second.
    pub fn level(&self, now_us: Us) -> f32 {
        self.meter.level(now_us)
    }

    /// One pass over the block's samples, no allocation.
    pub fn feed(&mut self, block: &PcmBlock, at_us: Us) {
        if block.samples.is_empty() {
            return;
        }
        self.started_at.get_or_insert(at_us);
        let peak = peak_of(block);
        if peak == 0.0 {
            self.zeros_since.get_or_insert(at_us);
        } else {
            self.zeros_since = None;
        }
        if peak > QUIET_PEAK {
            self.audible_at = Some(at_us);
        }
        if peak > SPEECH_PEAK {
            (self.heard, self.zeros_given, self.quiet_given) = (true, false, false);
        }
        self.meter.feed(peak, at_us);
    }

    /// The next note due, one per call (zeros first, then near-silence, then not heard); each once until speech.
    pub fn check(&mut self, now_us: Us) -> Option<HealthNote> {
        let start = *self.started_at.get_or_insert(now_us);
        let zeros = self.zeros_since.map(|z| now_us - z);
        let quiet = zeros.is_none() && now_us - self.audible_at.unwrap_or(start) >= QUIET_US;
        let zeros_after = if self.heard { GATED_ZEROS_US } else { ZEROS_US };
        if zeros.is_some_and(|z| z >= zeros_after) && !self.zeros_given {
            self.zeros_given = true;
            return Some(self.note(code!("capture.health.micZeros"), false, Level::Warning));
        }
        if quiet && !self.quiet_given {
            self.quiet_given = true;
            return Some(self.note(code!("capture.health.micQuiet"), true, Level::Warning));
        }
        let soft = !self.heard && !self.not_heard_given && now_us - start >= NOT_HEARD_US;
        if soft && zeros.is_none() && !quiet {
            self.not_heard_given = true;
            // A soft notice: it doesn't change the take's colour.
            return Some(self.note(code!("capture.health.micNotHeard"), true, Level::Ok));
        }
        None
    }

    fn note(&self, code: &'static str, device: bool, level: Level) -> HealthNote {
        let params = if device {
            vec![("device", ErrorParam::from(self.device.as_str()))]
        } else {
            vec![]
        };
        HealthNote {
            code,
            source: Some(self.source.clone()),
            params,
            level,
        }
    }
}
