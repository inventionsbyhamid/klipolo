//! `SessionCore`: the recorder as a state machine with no threads and no IO. It takes commands, packets, PCM, pipe
//! events, shell inputs and ticks, each with the host time, and returns `Effect`s: files to open, append to and close,
//! journal lines, pipe commands, events for the app and outputs for the shell. So every rule is tested in virtual time
//! (`tests/core.rs`), and `run::threads` only carries effects out.
//!
//! This file holds the states, the commands, the start (countdown and gate) and Stop; `media_in` takes the pipes' starts
//! and media, `source` turns samples into runs and `rules` holds the ticks, heals and shell inputs.
//!
//! - **Start.** `Start` (or the start/stop key while armed) starts every pipe. Once all have started (or failed), a
//!   countdown of `countdown_s` runs while the start gate waits for real samples from each source; a gate question or a
//!   mic rule firing holds it. When both are done, at host time T0, the session clock starts, `Gate` is journaled and
//!   every video pipe is asked for a keyframe. A take whose main picture (its screen, window or game; the camera in
//!   Camera only) didn't start records nothing: `Failed`, and exit 3.
//! - **The start key's take.** When the person's start key began the take (the editor hidden: "Start from my game"),
//!   the same key answers the countdown's question, "{source} isn't sending anything yet.", with "Record without it"
//!   for the source it names; the item's Cancel answers it with no take. Unanswered, the countdown holds: no answer
//!   means no capture. In a take Record began, the screen asks, and the key stops as ever.
//! - **Start over** (the item's or the key's, while recording or paused): `StartOver` for the app, then Stop; the app
//!   trashes the finished take and starts the next with `StartAgain`. Refused (nothing happens) in any other state,
//!   Stopping included.
//! - **Stop** is idempotent. Before the gate it writes nothing and exits 3 (no take). After it, every run is flushed
//!   and ended, `StopRequested` journaled and `Finalize` asks the threads layer to finish the files (`run::finish`).
//! - **Pause** leaves its time out of the session clock; it never starts a run. Samples stamped before the
//!   pause are still written when they arrive after it (sound cut at the pause by its own frames), so no frame or
//!   sample is lost and a sound run spans the pause with no step. Resume asks every video pipe for
//!   a keyframe and video waits for one.

use std::collections::BTreeMap;

use klipolo_model::coded::CodedError;
use klipolo_model::time::Us;

use super::cursor_in::CursorState;
pub use super::effect::{Effect, PipeCmd, RunDone, RunKind, Started};
use super::source::Src;
use crate::backend::DeviceInfo;
use crate::clock::SessionClock;
use crate::disk::{DiskLevel, RateMeter};
use crate::gate::StartGate;
use crate::heal::HealPolicy;
use crate::media::SourceId;
use crate::pipe::VideoStats;
use crate::protocol::Command;
use crate::session::{FileKey, SessionConfig};
use crate::shell::{HotkeyAction, ShellInput, ShellOutput, WorkerState};
use crate::shell_view::Words;
use crate::writer::WriterSnapshot;

/// Why the take is paused: only an automatic pause for focus is resumed by the game coming back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PauseWhy {
    Person,
    Focus,
    Sleep,
    Lock,
}

/// The countdown's progress: it runs only while neither the gate nor a mic rule holds it.
#[derive(Clone, Debug, Default)]
pub(super) struct Countdown {
    pub(super) running_since: Option<u64>,
    pub(super) done_ns: u64,
    pub(super) shown: Option<u32>,
    pub(super) gate_open_at: Option<u64>,
    /// The gate's last question, emitted once per set of missing sources.
    pub(super) asked: Option<Vec<SourceId>>,
    /// A mic rule fired in the countdown: held until that mic sends sound or is left out.
    pub(super) held_by: Option<SourceId>,
    /// That rule's note: the HUD shows it while the countdown is held.
    pub(super) held_note: Option<CodedError>,
    /// Whether the last status went out while held, so a hold or its end sends a new one.
    pub(super) held_shown: bool,
}

#[derive(Clone, Debug)]
pub struct SessionCore {
    pub(super) cfg: SessionConfig,
    pub(super) mics: Vec<DeviceInfo>,
    pub(super) state: WorkerState,
    /// Session time: host time since the gate, pauses left out.
    pub(super) clock: SessionClock,
    /// The rules' clock (`MicRules`, `RateMeter`): host time since the pipes started, pauses left out.
    pub(super) rules_clock: SessionClock,
    pub(super) srcs: Vec<Src>,
    pub(super) gate: Option<StartGate>,
    pub(super) countdown: Countdown,
    pub(super) heal: HealPolicy,
    pub(super) pause: Option<PauseWhy>,
    /// The game lost focus at this host time (auto-pause after 500 ms).
    pub(super) focus_away: Option<u64>,
    pub(super) game_running: bool,
    pub(super) markers: u32,
    /// Every sample written so far was captured before this host time: a pause never starts earlier.
    pub(super) mapped_until: u64,
    pub(super) ticks: u64,
    pub(super) disk: DiskLevel,
    pub(super) minutes_left: Option<u32>,
    pub(super) rate: RateMeter,
    pub(super) slow: bool,
    pub(super) hot: bool,
    pub(super) orphan: bool,
    pub(super) read_back_bad: bool,
    pub(super) display_awake: bool,
    /// The session time the read-back is due at; None once asked.
    pub(super) read_back_at: Option<Us>,
    /// The take's cursor: its writer and watchdog (`cursor_in`).
    pub(super) cursor: CursorState,
    pub(super) timing_open: bool,
    /// The person's start key began this take (not the app's `Start`): it answers the countdown's question.
    pub(super) by_key: bool,
    /// Every run ended so far, for Stop.
    pub(super) done: Vec<RunDone>,
    /// A resumed take's earlier runs, finished by `resume`: their lengths.
    pub(super) earlier: BTreeMap<FileKey, Us>,
    /// The person's words, for the status lines (`SessionConfig::words`).
    pub(super) words: Words,
}

impl SessionCore {
    /// A take that waits, armed, for the person's Record. `mics`: the device list, for the built-in fallback.
    pub fn new(cfg: SessionConfig, mics: Vec<DeviceInfo>, _now_ns: u64) -> Self {
        let srcs = cfg.sources.iter().map(|s| Src::new(s, &mics)).collect();
        Self {
            srcs,
            mics,
            state: WorkerState::Armed,
            clock: SessionClock::new(),
            rules_clock: SessionClock::new(),
            gate: None,
            countdown: Countdown::default(),
            heal: HealPolicy::new(),
            pause: None,
            focus_away: None,
            game_running: true,
            markers: 0,
            mapped_until: 0,
            ticks: 0,
            disk: DiskLevel::Ok,
            minutes_left: None,
            rate: RateMeter::new(),
            slow: false,
            hot: false,
            orphan: false,
            read_back_bad: false,
            display_awake: true,
            read_back_at: None,
            cursor: CursorState::of(&cfg),
            timing_open: false,
            by_key: false,
            done: Vec::new(),
            earlier: BTreeMap::new(),
            words: Words::from(cfg.words.clone()),
            cfg,
        }
    }

    pub fn state(&self) -> WorkerState {
        self.state
    }

    /// The first effects, before any input: the state, the hotkeys and the status; a resumed take starts its pipes.
    pub fn opening(&mut self, now_ns: u64) -> Vec<Effect> {
        let mut fx = vec![Effect::Shell(ShellOutput::Keys(true))];
        if matches!(self.state, WorkerState::Recording | WorkerState::Paused) {
            fx.push(Effect::StartPipes(self.ids()));
            self.cursor_open(now_ns, &mut fx);
        }
        self.emit_state(now_ns, &mut fx);
        fx
    }

    pub fn command(&mut self, cmd: Command, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.advance(now_ns, &mut fx);
        match cmd {
            Command::Start => self.start(now_ns, &mut fx),
            Command::StartAgain => {
                if self.state == WorkerState::Armed {
                    self.by_key = true;
                }
                self.start(now_ns, &mut fx)
            }
            Command::Pause => self.pause_take(now_ns, PauseWhy::Person, &mut fx),
            Command::Resume => self.resume_take(now_ns, &mut fx),
            Command::Marker => self.marker(now_ns, &mut fx),
            Command::Stop | Command::Cancel | Command::Disarm => self.stop(now_ns, None, &mut fx),
            Command::WithoutSource { source } => self.without(&source, now_ns, &mut fx),
            // The live view's, never a take's.
            Command::ViewSize { .. } => {}
        }
        fx
    }

    pub fn shell(&mut self, input: ShellInput, now_ns: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.advance(now_ns, &mut fx);
        match input {
            ShellInput::Hotkey(HotkeyAction::StartStop) if self.state == WorkerState::Armed => {
                self.by_key = true;
                self.start(now_ns, &mut fx)
            }
            // "Record without it", for the source the question names (the module doc).
            ShellInput::Hotkey(HotkeyAction::StartStop) if self.by_key => {
                match self.asked_about() {
                    Some(source) => self.without(&source, now_ns, &mut fx),
                    None => self.stop(now_ns, None, &mut fx),
                }
            }
            ShellInput::Hotkey(HotkeyAction::StartStop) => self.stop(now_ns, None, &mut fx),
            other => self.shell_input(other, now_ns, &mut fx),
        }
        fx
    }

    pub fn config(&self) -> &SessionConfig {
        &self.cfg
    }

    /// Whether the captured display is awake now. The threads layer asks the backend as a pipe's stop or stall (or a
    /// failed restart) arrives, so a stop the display's sleep caused is known at once and never spends a restart;
    /// the tick passes it too.
    pub fn display(&mut self, awake: bool) {
        self.display_awake = awake;
    }

    /// The writer's snapshot, free space, each video pipe's counters and the display, once a second (`rules`).
    pub fn tick(
        &mut self,
        now_ns: u64,
        writer: &WriterSnapshot,
        free_bytes: Option<u64>,
        stats: &BTreeMap<SourceId, VideoStats>,
        display_awake: bool,
    ) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.advance(now_ns, &mut fx);
        self.tick_rules(now_ns, writer, free_bytes, stats, display_awake, &mut fx);
        fx
    }
}
