//! The seam each OS implements once: what this computer can capture with, its devices and permissions
//! (asked without capture or prompts), the pipes, Check camera's preview, the live view and the Record dialog's stills,
//! the encoder probe and the worker's shell. The UI and the presets read `CaptureCaps` and never branch on the OS.

use std::time::Duration;

use klipolo_model::coded::CodedError;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

mod preview;

pub use crate::cursor::CursorCaps;
use crate::cursor::CursorSource;
pub use crate::devices::{
    AppInfo, DeviceInfo, Devices, DisplayInfo, Permission, Permissions, Transport,
};
use crate::english::msg;
use crate::media::{Codec, StreamInfo};
use crate::pipe::{AudioPipe, PipeError, VideoPipe};
use crate::preflight::cursor::CursorPlan;
use crate::shell::Shell;
pub use preview::{PreviewFrame, PreviewPipe};

pub trait Backend: Send + Sync {
    fn caps(&self) -> CaptureCaps;
    /// No capture: names, transports and running apps from the OS's registries (never cpal's listing).
    fn devices(&self) -> Devices;
    /// No capture: the microphones alone, from the OS's registry. A take on a fallback mic asks once a second whether
    /// the person's own one is back (`run::controller`), so a backend whose `devices` lists more (running apps,
    /// displays) can answer this from its mic listing alone.
    fn mics(&self) -> Vec<DeviceInfo> {
        self.devices().mics
    }
    /// No prompt.
    fn permissions(&self) -> Permissions;
    /// The mic's permission alone, read as `permissions` reads it (no prompt): the Record dialog's meter reads it again
    /// once a second while it runs. Default: `permissions().mic`.
    fn mic_permission(&self) -> Permission {
        self.permissions().mic
    }
    /// Builds the pipes without starting them; `start` (on its own thread, holding no lock) may wait for a system
    /// picker (`pipe`'s "Starting").
    fn video(&self, spec: &VideoSpec) -> Result<VideoPipes, PipeError>;
    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError>;
    /// The cursor source for a screen target whose cursor doesn't come with its frames (macOS, Windows' WGC); None
    /// where the video pipe sends `CursorEvent`s itself (portal metadata, DDA). Builds without starting.
    fn cursor_source(&self, _target: &Target) -> Result<Option<Box<dyn CursorSource>>, PipeError> {
        Ok(None)
    }
    /// Check camera: frames for the framing preview, no mic, nothing written.
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError>;
    /// The recorder's live view of `target` (the one exception to capture only after a press): a small, slow picture of
    /// a screen, window or game, no sound, nothing written. Building it opens nothing, and its `start` never asks for a
    /// permission: it runs only while screen recording is allowed already. Default: Unsupported (Windows' capture will
    /// add it; Linux's ScreenCast portal would ask at every opening).
    fn live_view(&self, _target: &Target) -> Result<Box<dyn PreviewPipe>, PipeError> {
        Err(PipeError::Unsupported)
    }
    /// A still of `target` (a display, or an app's windows on its display) `width` pixels wide in its own shape, as a
    /// JPEG: the Record dialog's cards (`run::thumbs`). Nothing is written, and it never asks for a permission: it runs
    /// only while screen recording is allowed already. Default: Unsupported.
    fn still(&self, _target: &Target, _width: u32) -> Result<PreviewFrame, PipeError> {
        Err(PipeError::Unsupported)
    }
    /// The encoders this computer has, measured with generated frames only, within `budget`.
    fn probe(&self, budget: Duration) -> ProbeReport;
    fn shell(&self) -> Box<dyn Shell>;
    /// Whether the captured display is awake (a stall only counts while it is). Default: true.
    fn display_awake(&self) -> bool {
        true
    }
    /// Whether a system alert is on the screen now: one of the OS's own questions, which a take must not record (on
    /// macOS, a permission prompt, or the question a take's first stream can raise about the private window picker).
    /// A take reads it from the start until the gate, and its countdown waits while one is up
    /// (`SessionCore::system_alert`). Read without capture or a prompt. Default: none.
    fn system_alert(&self) -> bool {
        false
    }
    /// Whether this keyframe decodes (the 10 s read-back); None where there's no decoder to ask.
    /// macOS: VTDecompressionSession; Windows and Linux: FFmpeg.
    fn decodes(&self, _info: &StreamInfo, _keyframe: &[u8]) -> Option<bool> {
        None
    }
    /// Asks the OS to let Klipolo record the screen, only on the person's press of "Allow screen recording":
    /// macOS lists Klipolo in its Screen Recording settings and asks the first time, and a grant applies once
    /// Klipolo reopens. Default: nothing to ask (an OS without that permission, or one whose picker asks per take).
    fn request_screen_permission(&self) {}
}

/// A video source's pipe, and its app's or the computer's sound when that was asked for and comes with the video
/// (ScreenCaptureKit; Windows' process loopback).
pub struct VideoPipes {
    pub video: Box<dyn VideoPipe>,
    pub sound: Option<Box<dyn AudioPipe>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSpec {
    pub target: Target,
    pub preset: PresetId,
    /// How the cursor is recorded (screens only; `preflight::cursor::cursor_plan`).
    pub cursor: CursorPlan,
    /// Also capture the target's sound.
    pub sound: bool,
    /// The encoders to record with, tried in order (`encoder_pick`): the probe's choice for this preset, then the
    /// others that hold it (`probe::encoders_for`), so a take never records without video while another encoder works.
    /// Empty: the backend's own pick, its first hardware H.264 encoder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub encoders: Vec<EncoderPick>,
    /// Cameras: the session also feeds the take's camera bubble (`SessionConfig::camera_bubble`), a live layer on the
    /// same running session (no second open, no frame copied), made before it starts running.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bubble: bool,
    /// The bubble shows the camera as in a mirror (the recorder's "Mirror my camera"). The file never is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bubble_mirrored: bool,
}

/// An encoder by the backend's id ("h264.gva", "hevc.ave", "h264_nvenc") and the codec it records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderPick {
    pub codec: Codec,
    pub id: String,
}

/// What a video pipe captures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Target {
    Display {
        id: String,
    },
    /// An app's windows on one display (games).
    App {
        app_id: String,
        display: String,
    },
    Window {
        id: String,
    },
    /// A whole display without Klipolo's own windows.
    DisplayWithoutKlipolo {
        id: String,
    },
    Camera {
        id: String,
    },
    /// The system's picker chooses (Linux's ScreenCast portal). `restore_token` restores the last choice without the
    /// picker (single-use: the pipe sends the next one as `PipeEvent::RestoreToken`). `app_id` names the game or app
    /// whose sound to link when `sound` is asked for, since the portal doesn't say whose window was picked (Linux finds
    /// its PipeWire output nodes by it); None records the computer's sound.
    Portal {
        restore_token: Option<String>,
        app_id: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AudioSpec {
    /// A microphone by its id from `Devices::mics`.
    Mic { device: String },
    /// The whole computer's sound.
    System,
}

/// The quality presets (`preset` gives their sizes and rates).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PresetId {
    /// The display's own pixels at 30 fps.
    Native30,
    /// The display's own pixels at 60 fps.
    Native60,
    Light,
    #[default]
    Smooth,
    Sharp,
    SlowMo,
    Camera720,
    Camera1080,
}

/// What this computer's recorder can do. Data, never `cfg`: the UI and the presets read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CaptureCaps {
    pub screen: ScreenApi,
    /// How an app's or game's own sound is captured.
    pub app_audio: AppAudioApi,
    pub hotkeys: HotkeyApi,
    /// Whether the frontmost app can be followed (auto-pause for games; false on Wayland).
    pub focus_events: bool,
    pub picker: PickerKind,
    /// Whether the recorder can show its HUD over a game.
    pub hud: bool,
    /// Whether this OS version can record at all (macOS 15 or later, for example).
    pub os_supported: bool,
    pub indicator: IndicatorPlace,
    pub cursor: CursorCaps,
}

impl CaptureCaps {
    /// A computer that can't record yet: every API None. The OS backends start here until they can record.
    pub fn none(indicator: IndicatorPlace, picker: PickerKind) -> Self {
        Self {
            screen: ScreenApi::None,
            app_audio: AppAudioApi::None,
            hotkeys: HotkeyApi::None,
            focus_events: false,
            picker,
            hud: false,
            os_supported: false,
            indicator,
            cursor: CursorCaps::none(),
        }
    }
}

/// Where the recorder's item lives: copy names it through `capture.place.*`, never "menu bar" alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum IndicatorPlace {
    MenuBar,
    NotificationArea,
    Tray,
}

impl IndicatorPlace {
    /// The nested code a message's `place` takes: "Stop it from {place}."
    pub fn coded(self) -> CodedError {
        match self {
            Self::MenuBar => msg!("capture.place.menuBar"),
            Self::NotificationArea => msg!("capture.place.notificationArea"),
            Self::Tray => msg!("capture.place.tray"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ScreenApi {
    ScreenCaptureKit,
    WindowsGraphicsCapture,
    DesktopDuplication,
    Portal,
    X11,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AppAudioApi {
    ScreenCaptureKit,
    ProcessLoopback,
    PipeWire,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HotkeyApi {
    Carbon,
    RegisterHotKey,
    Portal,
    X11,
    None,
}

/// Who chooses what to record: our list, or the system's picker (Linux's portal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PickerKind {
    OurList,
    System,
}

/// What the first-run probe measured (generated frames only, ≤ 5 s).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub encoders: Vec<EncoderResult>,
    pub took_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderResult {
    /// The encoder as the backend names it ("hevc.ave", "h264_nvenc").
    pub id: String,
    pub codec: Codec,
    pub hardware: bool,
    /// On its own engine, not the game's GPU (the T2's AVE, a discrete NVENC).
    pub dedicated: bool,
    /// The physical engine it runs on ("ave", "gva", "media", "nvenc:0"): encoders on one engine share its capacity, so
    /// a camera beside a screen on another of its ids is added up (`probe::keeps_both`).
    pub engine: String,
    /// Frames per second held at 1080p60 pacing.
    pub fps_1080p60: f32,
    /// Frames per second held at 1440p60 pacing.
    pub fps_1440p60: f32,
    /// Frames per second held at 1080p120 pacing (Slow motion).
    pub fps_1080p120: f32,
    /// 1440p as fast as it goes, unpaced: the headroom for the display's own size (`probe::decide`'s native rule).
    pub max_fps_1440p: f32,
    pub latency_p95_ms: f32,
}
