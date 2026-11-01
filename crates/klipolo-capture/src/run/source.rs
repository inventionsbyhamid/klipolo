//! Samples into runs (the core's admission). Each source's first admitted sample opens its run: `Open` with the fMP4
//! head or the CAF header, then `RunStarted` at that sample's session time. Before the gate and while paused nothing
//! is written. Rules from Cap's lessons (`track_heal.rs`, `sources/microphone.rs`; lessons only, no code): a time only
//! ever comes from the sample's own stamp, and a gap in a device's samples is never filled with invented silence.
//!
//! - **Video** drops non-key packets until its run's first keyframe (and again after a resume), so a run's first frame
//!   is a keyframe at pts 0; a fragment is appended as the muxer closes it.
//! - **Sound** is cut at the gate and at pauses by its own frames' stamps (never the callback's arrival), so a pause
//!   leaves no step between a run's frames and their times. A block that doesn't follow the last one within a buffer
//!   (or 20 ms) ends the run and opens another, so one drift line per run holds. Blocks are gathered
//!   into appends of at least 100 ms; an anchor goes to `timing.jsonl` once a second; each channel's peak is kept.
//! - **The cursor** goes to `cursor.jsonl` (format 2) through its writer, in appends of at least 100 ms (`cursor_in`).

use klipolo_model::project::SourceRole;
use klipolo_model::time::Us;

use super::core::Started;
use super::effect::Effect;
use crate::backend::{DeviceInfo, PresetId};
use crate::fmp4::Fmp4Writer;
use crate::heal::SourceKind;
use crate::health::{MicRules, VideoRules};
use crate::media::{AudioFormat, Codec, SourceId};
use crate::preset::preset;
use crate::session::{FileKey, SourceConfig, SourceSpec};

/// `timing.jsonl`'s key.
pub(super) const TIMING: &str = "timing.jsonl";
/// Sound is appended once this much is gathered: ten appends a second.
pub(super) const GATHER_US: u64 = 100_000;
/// A sound block that starts this far (or one buffer) from where the last ended is a discontinuity.
pub(super) const LOSS_US: Us = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status {
    /// Its pipe hasn't started, or is restarting.
    Starting,
    Live,
    /// Its pipe failed to start: the gate asks about it.
    Failed,
    /// Waiting for its app to come back (`TargetGone`).
    WaitingTarget,
    /// Waiting for its device since this host time.
    WaitingDevice(u64),
    /// The heal policy stopped it for the rest of the take.
    Stopped,
    /// The person recorded without it.
    Left,
}

#[derive(Clone, Debug)]
pub(super) struct Src {
    pub(super) id: SourceId,
    pub(super) kind: SourceKind,
    pub(super) label: String,
    /// A video source's preset: the config's, then one lighter after each `LowerPreset` (`preset::lighter`).
    pub(super) preset: Option<PresetId>,
    /// Its preset's frame rate (the muxer's fragment cap and the drop rule); Light's for sound.
    pub(super) fps: u32,
    /// The mic's device id (the one it records now).
    pub(super) device: Option<String>,
    /// The person's own mic (the config's), which the take switches back to after a fallback.
    pub(super) own: Option<String>,
    /// Switching back to `own` from this fallback mic: the switch is done once `own` has started.
    pub(super) switch_from: Option<String>,
    pub(super) format: Option<Started>,
    pub(super) status: Status,
    pub(super) run: Option<Run>,
    pub(super) next_run: u32,
    /// A resumed take: where this source's earlier media ended, noted on its first sample.
    pub(super) gap_from: Option<Us>,
    pub(super) blank: bool,
    pub(super) video_rules: Option<VideoRules>,
    pub(super) mic_rules: Option<MicRules>,
    /// The mic rules warned (the read-back doesn't say it again).
    pub(super) mic_warned: bool,
    /// A non-zero sample was written this take.
    pub(super) sounded: bool,
    pub(super) wrote: bool,
    pub(super) partial_noted: bool,
    pub(super) key_asked: bool,
    /// When a restart was asked: its first frame gets a grace before it counts as stalled.
    pub(super) restart_at: Option<u64>,
    /// Stopped while the display slept: restart once it wakes.
    pub(super) wake_restart: bool,
    /// Its capture stops while the display sleeps, so its restarts wait for the wake: a screen, and the computer's
    /// sound (macOS records it from a display's stream).
    pub(super) waits_for_wake: bool,
    /// The video source whose stream this sound comes on (`SourceSpec::AppSound`): it ends when that source is given
    /// up.
    pub(super) comes_with: Option<SourceId>,
    pub(super) dropped_seen: u64,
    pub(super) delivered_seen: u64,
    pub(super) fps_seen: u64,
    pub(super) meter: (f32, u64),
    pub(super) zeros_flash_at: Option<u64>,
}

#[derive(Clone, Debug)]
pub(super) enum Run {
    Video(VideoRun),
    Sound(SoundRun),
}

#[derive(Clone, Debug)]
pub(super) struct VideoRun {
    pub(super) n: u32,
    pub(super) key: FileKey,
    pub(super) mux: Fmp4Writer,
    pub(super) offset_us: Us,
    pub(super) awaiting_key: bool,
}

#[derive(Clone, Debug)]
pub(super) struct SoundRun {
    pub(super) n: u32,
    pub(super) key: FileKey,
    pub(super) format: AudioFormat,
    pub(super) frames: u64,
    pub(super) pending: Vec<u8>,
    pub(super) pending_frames: u64,
    /// The session time just after the last frame written: where the next block should start.
    pub(super) next_us: Us,
    pub(super) next_anchor_us: Us,
    pub(super) peaks: Vec<f32>,
    /// The file length the last running `Levels` covered (`rules::running_levels`).
    pub(super) levels_through: u64,
}

impl Src {
    pub(super) fn new(cfg: &SourceConfig, _mics: &[DeviceInfo]) -> Self {
        let device = match &cfg.spec {
            SourceSpec::Mic { device } => Some(device.clone()),
            _ => None,
        };
        let kind = kind_of(cfg.role);
        let comes_with = match &cfg.spec {
            SourceSpec::AppSound { of } => Some(of.clone()),
            _ => None,
        };
        Self {
            id: cfg.id.clone(),
            kind,
            label: cfg.label.clone(),
            preset: preset_of(&cfg.spec),
            fps: fps_of(preset_of(&cfg.spec)),
            own: device.clone(),
            switch_from: None,
            device,
            format: None,
            status: Status::Starting,
            run: None,
            next_run: 1,
            gap_from: None,
            blank: false,
            video_rules: None,
            mic_rules: None,
            mic_warned: false,
            sounded: false,
            wrote: false,
            partial_noted: false,
            key_asked: false,
            restart_at: None,
            wake_restart: false,
            waits_for_wake: kind == SourceKind::Screen || cfg.spec == SourceSpec::System,
            comes_with,
            dropped_seen: 0,
            delivered_seen: 0,
            fps_seen: 0,
            meter: (0.0, 0),
            zeros_flash_at: None,
        }
    }

    pub(super) fn is_video(&self) -> bool {
        matches!(self.kind, SourceKind::Screen | SourceKind::Camera)
    }

    /// The open run's file.
    pub(super) fn run_key(&self) -> Option<&str> {
        match &self.run {
            Some(Run::Video(r)) => Some(&r.key),
            Some(Run::Sound(r)) => Some(&r.key),
            None => None,
        }
    }

    /// The rules a source starts the countdown (or a resumed take) with.
    pub(super) fn rules_for_start(&mut self, mics: &[DeviceInfo]) {
        match self.kind {
            SourceKind::Mic => {
                self.mic_rules = Some(mic_rules(&self.id, self.device.as_deref(), mics))
            }
            SourceKind::Screen | SourceKind::Camera => {
                self.video_rules = Some(video_rules(&self.id, self.fps))
            }
            SourceKind::Sound => {}
        }
    }
}

/// The gathered sound as one append; the buffer keeps its size for the next.
pub(super) fn flush_sound(r: &mut SoundRun) -> Effect {
    let rate = u64::from(r.format.rate.max(1));
    let media_us = (r.pending_frames * 1_000_000 / rate) as Us;
    r.pending_frames = 0;
    let capacity = r.pending.capacity();
    Effect::Append {
        key: r.key.clone(),
        bytes: std::mem::replace(&mut r.pending, Vec::with_capacity(capacity)),
        media_us,
    }
}

/// The take's main picture, by index: its screen, window or game, else (Camera only) its camera. A take whose main
/// picture didn't start records nothing.
pub(super) fn main_picture(srcs: &[Src]) -> Option<usize> {
    let first = |kind| srcs.iter().position(|s| s.kind == kind);
    first(SourceKind::Screen).or_else(|| first(SourceKind::Camera))
}

/// A source's kind for the heal policy and the rules.
pub(super) fn kind_of(role: SourceRole) -> SourceKind {
    match role {
        SourceRole::Screen | SourceRole::Game => SourceKind::Screen,
        SourceRole::Camera => SourceKind::Camera,
        SourceRole::Mic => SourceKind::Mic,
        _ => SourceKind::Sound,
    }
}

/// A video source's preset; None for sound.
fn preset_of(spec: &SourceSpec) -> Option<PresetId> {
    match spec {
        SourceSpec::Screen(v) => Some(v.preset),
        SourceSpec::Camera { preset, .. } => Some(*preset),
        _ => None,
    }
}

/// A video source's frame rate from its preset (the muxer's fragment cap and the drop rule); Light's without one.
pub(super) fn fps_of(id: Option<PresetId>) -> u32 {
    preset(id.unwrap_or(PresetId::Light), Codec::H264).fps
}

/// The rules for a source as the countdown begins (or a pipe comes back): the mic's on its device's name.
pub(super) fn mic_rules(id: &str, device: Option<&str>, mics: &[DeviceInfo]) -> MicRules {
    let name = device.map(|d| {
        mics.iter()
            .find(|m| m.id == d)
            .map_or(d, |m| m.name.as_str())
    });
    MicRules::new(id, name.unwrap_or(id))
}

pub(super) fn video_rules(id: &str, fps: u32) -> VideoRules {
    VideoRules::new(id, fps)
}
