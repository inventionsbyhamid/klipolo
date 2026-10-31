//! A `SessionCore` driven in virtual time. The harness is the core's whole world: its fake pipes answer
//! `StartPipes` at once (the fixture's stream for video, 48 kHz for sound), send synthetic packets (30 fps, a keyframe a
//! second, flash frames each second) and PCM (10 ms blocks with a blip each second, so flashes and blips line up), and
//! the screen's cursor events (a point a frame); `feed` delivers them in time order with a tick each virtual second, so the rules that
//! poll (`device_still_missing`, `VideoRules`, the read-back) run as they would live. The effects land
//! in an in-memory file map and journal; events, effects and shell outputs are kept for the tests.
//!
//! The screen's packets go through a `PacketGate` on a channel of 2, which `stall_packets` stops draining.

pub mod cursor;
mod feed;
mod inputs;
mod read;
mod setup;
mod threaded;

pub use setup::{Src, cfg, mics, video_spec};

// Each test binary uses some of these.
#[allow(unused_imports)]
pub use threaded::{
    fake_backend, fixture, gate_host_ns, resumed_from, script_start_then_stop_after_ms, session,
};

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, sync_channel};

use klipolo_capture::backend::DeviceInfo;
use klipolo_capture::journal::Entry;
use klipolo_capture::media::EncodedPacket;
use klipolo_capture::pipe::PacketGate;
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::core::{Effect, SessionCore};
use klipolo_capture::run::resume::Resumed;
use klipolo_capture::session::{FileKey, SessionConfig, SourceSpec};
use klipolo_capture::shell::{ShellOutput, WorkerState};
use klipolo_capture::synthetic::{AudioSource, Signal, SyntheticAudioSpec};
use klipolo_model::project::{GameInfo, SourceRole};

pub(super) const S: u64 = 1_000_000_000;
/// The virtual host time `Harness::new` starts at.
pub const BASE_NS: u64 = 1_000 * S;
pub const FPS: u64 = 30;

/// One source's fake pipe.
pub(super) struct Fake {
    pub(super) id: String,
    pub(super) video: bool,
    pub(super) channels: u16,
    pub(super) started: bool,
    pub(super) flowing: bool,
    pub(super) paused: bool,
    pub(super) stopped: bool,
    pub(super) want_key: bool,
    pub(super) since_key: Option<u32>,
    pub(super) next_frame: u64,
    pub(super) delivered: u64,
    pub(super) audio: Option<AudioSource>,
    pub(super) signal: Signal,
    pub(super) drift_ppm: f64,
    /// The screen's packet gate and the channel of 2 behind it, and how many frames it isn't drained for.
    pub(super) gate: Option<(PacketGate, Receiver<EncodedPacket>, u32)>,
    /// After a (re)start, nothing until then (`first_frame_after`).
    pub(super) quiet_until: u64,
    pub(super) hold_ns: u64,
    /// Its starts fail with this (`fails_to_start`).
    pub(super) start_error: Option<klipolo_capture::pipe::PipeError>,
    /// The screen sends the cursor itself, a point a frame (a take whose cursor the test sends has none).
    pub(super) pipe_cursor: bool,
    pub(super) cursor_pressed: bool,
    pub(super) cursor_area_sent: bool,
}

pub struct Harness {
    pub core: SessionCore,
    pub cfg: SessionConfig,
    pub now: u64,
    pub base: u64,
    pub(super) fakes: Vec<Fake>,
    pub files: BTreeMap<FileKey, Vec<u8>>,
    pub(super) journal: Vec<Entry>,
    pub(super) events: Vec<Event>,
    pub(super) effects: Vec<Effect>,
    pub shell_out: Vec<ShellOutput>,
    pub(super) next_tick: u64,
    pub(super) free: Option<u64>,
    pub(super) queued_ms: u32,
    pub display_awake: bool,
}

impl Harness {
    pub fn new(cfg: SessionConfig) -> Self {
        Self::new_with_mics(cfg, mics())
    }

    /// On a computer with these mics (a Mac mini has no built-in one).
    pub fn new_with_mics(cfg: SessionConfig, mics: Vec<DeviceInfo>) -> Self {
        let core = SessionCore::new(cfg.clone(), mics, BASE_NS);
        Self::with_core(core, cfg, BASE_NS)
    }

    /// Start, and feed until the gate opens.
    /// Start, and feed until the gate opens: the harness stops at the input that opened it.
    pub fn start_recording(&mut self) {
        self.command(Command::Start);
        while self.state() != WorkerState::Recording {
            assert!(
                self.now < self.base + 10 * S,
                "the gate never opened: {:?}",
                self.events
            );
            self.step(u64::MAX);
        }
    }

    pub(super) fn with_core(core: SessionCore, cfg: SessionConfig, now: u64) -> Self {
        let fakes = cfg
            .sources
            .iter()
            .map(|s| Fake::new(&s.id, s.role, &s.spec))
            .collect();
        let mut h = Self {
            core,
            cfg,
            now,
            base: now,
            fakes,
            files: BTreeMap::new(),
            journal: vec![],
            events: vec![],
            effects: vec![],
            shell_out: vec![],
            next_tick: now + S,
            free: Some(500_000_000_000),
            queued_ms: 0,
            display_awake: true,
        };
        let fx = h.core.opening(now);
        h.apply(fx);
        h
    }

    /// Started and past the gate (a 3 s countdown), with what came before cleared.
    pub fn recording(srcs: &[Src]) -> Self {
        Self::recording_cfg(cfg(srcs, 3))
    }

    pub fn recording_cfg(cfg: SessionConfig) -> Self {
        let mut h = Self::new(cfg);
        h.start_recording();
        h.clear();
        h
    }

    /// Recording a game (`app_id`) that auto-pauses while it isn't in front.
    pub fn recording_game(app_id: &str, srcs: &[Src]) -> Self {
        let mut cfg = cfg(srcs, 3);
        cfg.auto_pause = Some(app_id.into());
        cfg.game = Some(GameInfo {
            app_id: app_id.into(),
            name: "The Game".into(),
            profile_id: None,
        });
        Self::recording_cfg(cfg)
    }

    /// Recording with the mic's device clock running `ppm` faster than the host's.
    pub fn recording_with_drift(srcs: &[Src], ppm: f64) -> Self {
        let mut h = Self::new(cfg(srcs, 3));
        for f in &mut h.fakes {
            f.drift_ppm = ppm;
        }
        h.start_recording();
        h
    }

    /// A resumed core at host `at_s` seconds.
    pub fn resume(srcs: &[Src], from: Resumed, at_s: f64) -> Self {
        let cfg = cfg(srcs, 3);
        let now = (at_s * 1e9) as u64;
        let core = SessionCore::resumed(cfg.clone(), mics(), from, now);
        Self::with_core(core, cfg, now)
    }

    pub fn state(&self) -> WorkerState {
        self.core.state()
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }

    pub fn journal(&self) -> &[Entry] {
        &self.journal
    }
}

impl Fake {
    fn new(id: &str, role: SourceRole, spec: &SourceSpec) -> Self {
        let video = role.is_video();
        let screen = matches!(spec, SourceSpec::Screen(_));
        Self {
            id: id.into(),
            video,
            channels: if role == SourceRole::Mic { 1 } else { 2 },
            started: false,
            flowing: true,
            paused: false,
            stopped: false,
            want_key: false,
            since_key: None,
            next_frame: 0,
            delivered: 0,
            audio: None,
            signal: Signal::Bursts { dbfs: -20.0 },
            drift_ppm: 0.0,
            gate: screen.then(|| {
                let (tx, rx) = sync_channel(2);
                (PacketGate::new(tx), rx, 0)
            }),
            quiet_until: 0,
            hold_ns: 0,
            start_error: None,
            pipe_cursor: screen,
            cursor_pressed: false,
            cursor_area_sent: false,
        }
    }

    /// The sound on every source's one schedule (device frame 0 at host 0, so blips land on host seconds, as the
    /// video's flash frames do), from `now` on.
    pub(super) fn audio_source(&self, now: u64) -> AudioSource {
        let spec = SyntheticAudioSpec {
            rate: 48_000,
            channels: self.channels,
            block_ms: 10,
            drift_ppm: self.drift_ppm,
            signal: self.signal.clone(),
            blip_every_ms: (self.signal != Signal::Zeros).then_some(1_000),
        };
        let mut source = AudioSource::new(spec, 0, 0);
        source.skip_to(now);
        source
    }
}
