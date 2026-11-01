//! The worker's shell seam: the OS main loop on the worker's main thread, with the recorder's menu-bar, notification
//! area or tray item, the HUD, the global hotkeys (registered only while armed), power and sleep events, the frontmost
//! app and the parent watch. It turns what happens into `ShellInput`s and shows the session's `ShellOutput`s.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use klipolo_model::time::Us;
use klipolo_platform::caps::OsFamily;
use klipolo_platform::power::Thermal;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::profile::Corner;

pub trait Shell: Send {
    /// The OS main loop, on the main thread, until `ShellOutput::Quit` (or the session's end of the channel closes).
    fn run(self: Box<Self>, io: ShellIo);
}

pub struct ShellIo {
    pub input: SyncSender<ShellInput>,
    pub output: Receiver<ShellOutput>,
    pub keys: KeyBindings,
    /// The session's words, `capture.*` code → template: native menus speak the person's language.
    pub words: BTreeMap<String, String>,
    /// Where the HUD shows (a game's profile; `SessionConfig::hud_corner`).
    pub hud_corner: Corner,
    /// The display the take records (`SessionConfig::captured_display`): the HUD goes there, so a game on a second
    /// display has its HUD over it. None: the screen the person is on.
    pub hud_display: Option<String>,
    /// Set with the session's exit code once it has ended, whether or not `run` has returned.
    pub end: SessionEnd,
    /// The take shows its camera bubble (`SessionConfig::camera_bubble`): a shell that draws one makes its window at the
    /// start, before any capture filter is built, and shows it while `StatusView::bubble` says.
    pub camera_bubble: bool,
}

/// The session's end, with its exit code (`protocol`): set once everything after the shell has run (the core and the
/// files finished, the events written out, an empty take's folder removed). A shell that must end the process itself
/// waits on it, so the worker still exits with the session's own code: macOS's system quit (logout, Activity Monitor),
/// which AppKit would otherwise end with exit 0 mid-take.
#[derive(Clone, Debug, Default)]
pub struct SessionEnd(Arc<(Mutex<Option<i32>>, Condvar)>);

impl SessionEnd {
    pub fn set(&self, code: i32) {
        let (code_of, ended) = &*self.0;
        *code_of.lock().unwrap_or_else(PoisonError::into_inner) = Some(code);
        ended.notify_all();
    }

    /// The exit code, once the session has ended within `timeout`; None if it hasn't by then.
    pub fn wait(&self, timeout: Duration) -> Option<i32> {
        let (code_of, ended) = &*self.0;
        let code = code_of.lock().unwrap_or_else(PoisonError::into_inner);
        let (code, _) = ended
            .wait_timeout_while(code, timeout, |c| c.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        *code
    }

    /// The exit code, however long the session takes.
    pub fn wait_forever(&self) -> i32 {
        let (code_of, ended) = &*self.0;
        let code = code_of.lock().unwrap_or_else(PoisonError::into_inner);
        let code = ended
            .wait_while(code, |c| c.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        code.unwrap_or(1)
    }
}

/// Accelerators in `global-hotkey`'s syntax ("Alt+Super+KeyR"); the Linux portal gets them as preferred triggers. The
/// person can rebind them; they are saved in `<app data>/recorder/keys.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeyBindings {
    pub start_stop: String,
    pub pause: String,
    pub marker: String,
    /// Start over: the take so far is discarded and the countdown runs again. No key unless the person sets
    /// one: a stray press would discard a take, and ⌥⌘N, the obvious one, is Finder's New Smart Folder.
    /// It stays in the item's menu. Missing (`keys.json` and `session.json` from earlier versions): none, so those
    /// takes still read.
    #[serde(default)]
    pub start_over: Option<String>,
}

impl KeyBindings {
    /// ⌥⌘R, ⌥⌘P and ⌥⌘M on macOS; Ctrl+Alt+R, P and M elsewhere; no key for Start over (`start_over`).
    pub fn defaults(os: OsFamily) -> Self {
        let with = match os {
            OsFamily::Macos => "Alt+Super",
            OsFamily::Windows | OsFamily::Linux => "Ctrl+Alt",
        };
        Self {
            start_stop: format!("{with}+KeyR"),
            pause: format!("{with}+KeyP"),
            marker: format!("{with}+KeyM"),
            start_over: None,
        }
    }

    /// Each binding with the action it stands for, in the menu's order; Start over only when it has one.
    pub fn all(&self) -> Vec<(&str, HotkeyAction)> {
        let start_over = (self.start_over.as_deref())
            .filter(|k| !k.is_empty())
            .map(|k| (k, HotkeyAction::StartOver));
        [
            (self.start_stop.as_str(), HotkeyAction::StartStop),
            (self.pause.as_str(), HotkeyAction::Pause),
            (self.marker.as_str(), HotkeyAction::Marker),
        ]
        .into_iter()
        .chain(start_over)
        .collect()
    }

    /// Every action has its own binding (the UI's "Each one needs its own keys."); Start over without one never clashes.
    pub fn distinct(&self) -> bool {
        let all = self.all();
        (all.iter().enumerate()).all(|(i, (a, _))| all[..i].iter().all(|(b, _)| a != b))
    }
}

/// What the shell saw.
#[derive(Clone, Debug, PartialEq)]
pub enum ShellInput {
    Hotkey(HotkeyAction),
    Menu(MenuAction),
    Power(PowerEvent),
    /// The frontmost app changed (None: not an app the OS names).
    Focus {
        app_id: Option<String>,
    },
    AppQuit {
        app_id: String,
    },
    AppLaunched {
        app_id: String,
    },
    /// The app that launched the worker is gone: the worker records on, orphaned (never a Stop).
    ParentGone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    StartStop,
    Pause,
    Marker,
    StartOver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Stop,
    Pause,
    Resume,
    Marker,
    /// The take so far goes to the Trash and the countdown runs again with the same sources.
    StartOver,
}

impl MenuAction {
    const ALL: [Self; 5] = [
        Self::Stop,
        Self::Pause,
        Self::Resume,
        Self::Marker,
        Self::StartOver,
    ];

    /// The id a shell's native menu item carries, so a click is paired with its action by id (`from_id`), never by its
    /// place in the menu.
    pub fn id(self) -> &'static str {
        match self {
            Self::Stop => "klipolo.stop",
            Self::Pause => "klipolo.pause",
            Self::Resume => "klipolo.resume",
            Self::Marker => "klipolo.marker",
            Self::StartOver => "klipolo.startOver",
        }
    }

    /// The action whose `id` this is; None for any other item (a plain line).
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// `slept_ms`: the wall time the shell measured between WillSleep and Woke (`SystemTime`), since the host clock stops
/// during sleep on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerEvent {
    WillSleep,
    Woke { slept_ms: u64 },
    Locked,
    Unlocked,
    PowerOff,
    Thermal(Thermal),
}

/// What the session asks the shell to show.
#[derive(Clone, Debug, PartialEq)]
pub enum ShellOutput {
    /// Register the hotkeys (true: armed) or unregister them.
    Keys(bool),
    Status(StatusView),
    /// The HUD flashes for 2 s.
    Flash(Flash),
    Quit,
}

/// The item's title and menu.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusView {
    pub state: WorkerState,
    /// The take's length so far, paused time left out.
    pub elapsed_us: Us,
    /// One line per source and the space left, already worded in the session's words.
    pub lines: Vec<String>,
    pub level: Level,
    /// The countdown's seconds left while it runs (the HUD shows them when the person's start key began the take).
    pub countdown_s: Option<u32>,
    /// The loudest mic's peak over about the last half second, in dBFS, while counting down or recording (the HUD's
    /// meter); None without a mic, or in any other state.
    pub mic_dbfs: Option<f32>,
    /// Why the countdown is held, worded (a mic rule's note, or "{source} isn't sending anything yet."): the HUD shows
    /// it instead of a digit that has stopped. None unless counting down and held.
    pub held: Option<String>,
    /// The take began without the editor in sight (the person's start key, or Start over's next take): the HUD shows
    /// its countdown and question, and the start key answers it.
    pub by_key: bool,
    /// The camera bubble shows now (`shell_view::bubble_shows`).
    pub bubble: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flash {
    Marker,
    Paused,
    Resumed,
    Problem,
    LowSpace,
}

/// How a take is doing: the item's colour and the notes' weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CaptureLevel")]
pub enum Level {
    Ok,
    Healing,
    Warning,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WorkerState {
    /// Ready for the person's Record (or their hotkey in a game); nothing captures yet.
    Armed,
    /// Sources are starting.
    Starting,
    Countdown,
    Recording,
    Paused,
    /// Finishing the files.
    Stopping,
    /// The take is saved.
    Ready,
    Failed,
    /// Check camera's preview: the camera only, nothing written.
    CheckingCamera,
    /// No take, and nothing captures: the worker ended before the start gate (Cancel, Disarm, or a start that never
    /// got going), or the app gave the take up. Only the app's supervisor says it; the UI goes back to the
    /// recorder's choices.
    Idle,
}
