//! The heal policy: what the session does about each pipe event, so every healthy source keeps
//! recording and the take stops only when the person asked, permission went, the disk is full or no source is left.
//! Pure: every time is an argument (host ns).
//!
//! | Event | Source | Action |
//! |---|---|---|
//! | `StoppedBySystem` (display sleep, −3821), `Stalled` | any | `Restart` (a new run), at most 3 per 10 min; then `PauseSource` (`capture.health.lost`) |
//! | `StoppedByUser` ("Stop sharing") | any | `StopAndSave("capture.stop.sharing")` |
//! | `PermissionRevoked` | any | `StopAndSave("capture.stop.permission")`; that source never resumes |
//! | `TargetGone` (the app quit) | any | `WaitForTarget`; `TargetBack` then `Restart` |
//! | `DeviceLost` | camera, screen, sound | `WaitForDevice`; after 10 s, `Note(capture.health.lost)`; `DeviceBack` then `Restart` |
//! | `DeviceLost` | mic | `WaitForDevice` for 3 s; then `FallbackMic`. Back within the 3 s: `Restart`; past 3 per 60 s, `Flapping`; after the fallback, `SwitchBack` |
//! | `EncoderFailed` | screen, camera | `RebuildEncoder`, at most 3 per 10 min; then `LowerPreset` once; then `PauseSource` |
//! | `Blank(true)` for > 3 s | any | `Note(capture.health.lost)` with the reason "blank" (`still_blank`) |
//! | (its video given up) | app or game sound | the session ends it too, `capture.health.lost` "withVideo" (`ended_with_its_video`) |
//! | `Note(code)` | any | `Note` with that code, once per take and source |
//!
//! After a fallback, the person's own mic's `DeviceBack` comes only once it has been listed for 3 s straight
//! (`mic::Presence`, in the run controller's look, `run::own_mic`), and at most once per 30 s, so a flapping USB mic
//! churns runs: the take stays on the fallback until it settles, with one mild `capture.note.micFlapping`. A mic back
//! before any fallback (a blip) restarts at once. With nothing to fall back to, it restarts on every return, however
//! often it drops (`mic_dropping`: lost voice is worse than extra runs).
//!
//! Waits are polled: `device_still_missing` and `still_blank` once a second. Notes the policy can't word itself are
//! the session's: `capture.health.appQuit` (the app's name), `capture.health.micFallback` (both mics' names),
//! `capture.health.micBack` (the person's own mic's name).
//! Memory: a few small fields per source, and at most 3 times in each of its two windows.

mod budget;
mod notes;

use std::collections::{BTreeMap, VecDeque};

use budget::{encoder, fits, restart};
use notes::{dropping, lost, pipe_note};

use crate::english::code;
use crate::health::HealthNote;
use crate::media::SourceId;
use crate::pipe::PipeEvent;

pub const HEALS_PER_WINDOW: usize = 3;
pub const HEAL_WINDOW_NS: u64 = 600_000_000_000;
/// A lost mic not back by then is replaced by the built-in one.
pub const MIC_FALLBACK_NS: u64 = 3_000_000_000;
/// Another lost device is noted once it has been gone this long.
pub const LOST_NOTE_NS: u64 = 10_000_000_000;
/// A picture blank for longer than this is noted.
pub const BLANK_NOTE_NS: u64 = 3_000_000_000;
/// A mic back after a blip restarts on its own device at most this often in `BLIP_WINDOW_NS`; past that it is flapping.
pub const BLIPS_PER_WINDOW: usize = 3;
pub const BLIP_WINDOW_NS: u64 = 60_000_000_000;
/// A flapping mic with nothing to fall back to says so at most this often (`mic_dropping`).
pub const DROPPING_NOTE_EVERY_NS: u64 = 60_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Screen,
    Camera,
    Mic,
    /// Game or computer sound.
    Sound,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HealAction {
    /// End the run and restart the pipe; its next sample opens a new run.
    Restart,
    /// The recorded app went away: that source pauses (the session notes `capture.health.appQuit`) until `TargetBack`.
    WaitForTarget,
    /// The device went away: that source waits; call `device_still_missing` once a second.
    WaitForDevice,
    /// Switch the mic to the built-in one, as a new run (the session notes `capture.health.micFallback`).
    FallbackMic,
    /// The person's own mic is back after `FallbackMic`: switch to it again, as a new run (a gamer's
    /// USB mic is better than the built-in one; the session notes `capture.health.micBack`).
    SwitchBack,
    /// Flush and rebuild the same encoder, as a new run.
    RebuildEncoder,
    /// Rebuild the encoder at the next lighter preset, as a new run.
    LowerPreset,
    /// The mic's own device came back once too often (more than `BLIPS_PER_WINDOW` restarts in `BLIP_WINDOW_NS`): a
    /// fallback takes over if there is one (the session notes `capture.note.micFlapping`). With none,
    /// the session restarts it anyway, every time, and says so with `mic_dropping`.
    Flapping,
    /// Stop that source and keep the rest recording.
    PauseSource(HealthNote),
    /// Stop the take and save it, with this `capture.stop.*` code.
    StopAndSave(&'static str),
    Note(HealthNote),
    Ignore,
}

/// One source's heal state.
#[derive(Clone, Debug, Default)]
struct Heal {
    restarts: VecDeque<u64>,
    rebuilds: VecDeque<u64>,
    lowered: bool,
    lost_at: Option<u64>,
    /// The wait for a lost device was acted on (the note given, or the mic swapped).
    missing_done: bool,
    fell_back: bool,
    /// The mic's restarts after a blip, in `BLIP_WINDOW_NS`.
    blips: VecDeque<u64>,
    /// When `mic_dropping` last gave its note.
    dropping_noted_at: Option<u64>,
    /// When a loss with nothing to fall back to last flashed the HUD (`lost_flash`).
    lost_flashed_at: Option<u64>,
    blank_since: Option<u64>,
    blank_noted: bool,
    revoked: bool,
    /// The pipe notes given this take.
    noted: Vec<&'static str>,
}

#[derive(Clone, Debug, Default)]
pub struct HealPolicy {
    sources: BTreeMap<SourceId, Heal>,
}

/// The note for an app's or game's sound that came on a video source's stream when that source was given up for the
/// take (`HealAction::PauseSource`): its stream ended with it. `capture.health.lost` with the
/// reason "withVideo".
pub fn ended_with_its_video(source: &str) -> HealthNote {
    lost(source, "withVideo", None)
}

impl HealPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// `source`'s state, made on its first use. Only that first use allocates: the core polls `device_still_missing` on
    /// every input while a mic waits.
    fn of(&mut self, source: &str) -> &mut Heal {
        if !self.sources.contains_key(source) {
            self.sources.insert(source.to_owned(), Heal::default());
        }
        self.sources.get_mut(source).expect("made above")
    }

    /// The action for `event` from `source` (the module's table).
    pub fn decide(
        &mut self,
        source: &str,
        kind: SourceKind,
        event: &PipeEvent,
        now_ns: u64,
    ) -> HealAction {
        let h = self.of(source);
        match event {
            PipeEvent::StoppedByUser => HealAction::StopAndSave(code!("capture.stop.sharing")),
            PipeEvent::PermissionRevoked => {
                h.revoked = true;
                HealAction::StopAndSave(code!("capture.stop.permission"))
            }
            // Nothing brings back a source whose permission went.
            _ if h.revoked => HealAction::Ignore,
            PipeEvent::StoppedBySystem => restart(h, source, now_ns, "stopped"),
            PipeEvent::Stalled => restart(h, source, now_ns, "stalled"),
            PipeEvent::TargetGone => HealAction::WaitForTarget,
            PipeEvent::TargetBack => HealAction::Restart,
            PipeEvent::DeviceLost => {
                (h.lost_at, h.missing_done, h.fell_back) = (Some(now_ns), false, false);
                HealAction::WaitForDevice
            }
            PipeEvent::DeviceBack => {
                // Back within the 3 s: the same pipe again. After the fallback: the person's own mic again.
                let in_time = h
                    .lost_at
                    .is_some_and(|t| now_ns < t.saturating_add(MIC_FALLBACK_NS));
                let back = kind == SourceKind::Mic && h.fell_back && !in_time;
                (h.lost_at, h.missing_done) = (None, false);
                if back {
                    h.fell_back = false;
                    HealAction::SwitchBack
                } else if kind != SourceKind::Mic
                    || fits(&mut h.blips, now_ns, BLIP_WINDOW_NS, BLIPS_PER_WINDOW)
                {
                    // Another device, or a blip within the budget: the same device again.
                    HealAction::Restart
                } else {
                    // Flapping: the session falls back at once if it can, else restarts anyway.
                    HealAction::Flapping
                }
            }
            PipeEvent::EncoderFailed { code } => match kind {
                SourceKind::Screen | SourceKind::Camera => encoder(h, source, *code, now_ns),
                SourceKind::Mic | SourceKind::Sound => restart(h, source, now_ns, "stopped"),
            },
            PipeEvent::Blank(true) => {
                h.blank_since.get_or_insert(now_ns);
                HealAction::Ignore
            }
            PipeEvent::Blank(false) => {
                (h.blank_since, h.blank_noted) = (None, false);
                HealAction::Ignore
            }
            PipeEvent::Note(c) => pipe_note(h, source, c),
            PipeEvent::RestoreToken(_) => HealAction::Ignore,
        }
    }

    /// Once a second while `source`'s device is gone (since `since_ns`): a mic falls back after 3 s, another device is
    /// noted after 10 s; each once per loss, else `Ignore`.
    pub fn device_still_missing(
        &mut self,
        source: &str,
        kind: SourceKind,
        since_ns: u64,
        now_ns: u64,
    ) -> HealAction {
        let h = self.of(source);
        if h.revoked || h.missing_done {
            return HealAction::Ignore;
        }
        let gone = now_ns.saturating_sub(since_ns);
        match kind {
            SourceKind::Mic if gone >= MIC_FALLBACK_NS => {
                (h.missing_done, h.fell_back) = (true, true);
                HealAction::FallbackMic
            }
            SourceKind::Screen | SourceKind::Camera | SourceKind::Sound if gone >= LOST_NOTE_NS => {
                h.missing_done = true;
                HealAction::Note(lost(source, "missing", None))
            }
            _ => HealAction::Ignore,
        }
    }

    /// `FallbackMic` had nothing to switch to (a Mac mini, Studio or Pro, a desktop whose front jack isn't a device): the
    /// mic keeps waiting for its own device, and `DeviceBack` restarts it whenever it returns.
    pub fn fallback_unavailable(&mut self, source: &str) {
        if let Some(h) = self.sources.get_mut(source) {
            h.fell_back = false;
        }
    }

    /// The person's own mic didn't start after `SwitchBack`: the take records on the fallback again, and a later
    /// `DeviceBack` switches back again.
    pub fn on_fallback(&mut self, source: &str) {
        self.of(source).fell_back = true;
    }

    /// A flapping mic with nothing to fall back to restarts on every return (never trade
    /// voice for fewer runs). This is its mild `capture.note.micDropping`, at most once per `DROPPING_NOTE_EVERY_NS`;
    /// otherwise `Ignore`.
    pub fn mic_dropping(&mut self, source: &str, now_ns: u64) -> HealAction {
        let h = self.of(source);
        let every = DROPPING_NOTE_EVERY_NS;
        if (h.dropping_noted_at).is_some_and(|t| now_ns < t.saturating_add(every)) {
            return HealAction::Ignore;
        }
        h.dropping_noted_at = Some(now_ns);
        HealAction::Note(dropping(source))
    }

    /// A mic with nothing to fall back to is said to be lost on each loss of 3 s or more, but flashes the HUD for it at
    /// most once per `DROPPING_NOTE_EVERY_NS`, like `mic_dropping`'s note: a USB mic that keeps
    /// dropping never flashes over the game every few seconds. True when this loss may flash.
    pub fn lost_flash(&mut self, source: &str, now_ns: u64) -> bool {
        let h = self.of(source);
        let every = DROPPING_NOTE_EVERY_NS;
        if (h.lost_flashed_at).is_some_and(|t| now_ns < t.saturating_add(every)) {
            return false;
        }
        h.lost_flashed_at = Some(now_ns);
        true
    }

    /// Once a second per video source: the note once its picture has been blank (`PipeEvent::Blank(true)`) for more
    /// than 3 s; once per blank stretch, else `Ignore`.
    pub fn still_blank(&mut self, source: &str, now_ns: u64) -> HealAction {
        let Some(h) = self.sources.get_mut(source) else {
            return HealAction::Ignore;
        };
        match h.blank_since {
            Some(since) if !h.blank_noted && now_ns.saturating_sub(since) > BLANK_NOTE_NS => {
                h.blank_noted = true;
                HealAction::Note(lost(source, "blank", None))
            }
            _ => HealAction::Ignore,
        }
    }
}
