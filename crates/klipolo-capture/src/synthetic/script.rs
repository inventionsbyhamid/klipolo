//! The fake computer a `FakeBackend` runs (`Script`): what its pipes send, when, and what fails.

use std::collections::BTreeMap;
use std::path::PathBuf;

use klipolo_cursor::ClickSource;

use super::audio::Signal;
use super::cursor::CursorScript;
use super::packets::SyntheticVideoSpec;
use crate::backend::{
    AppAudioApi, CaptureCaps, CursorCaps, DeviceInfo, Devices, DisplayInfo, HotkeyApi,
    IndicatorPlace, Permission, PickerKind, ScreenApi, Target, Transport,
};
use crate::media::Codec;
use crate::pipe::{PipeError, PipeEvent};

/// What the fake video pipes send.
#[derive(Clone, Debug, PartialEq)]
pub enum VideoFeed {
    /// Synthetic packets of this spec (its size is the screen's; a camera's is 1280×720).
    Synthetic(SyntheticVideoSpec),
    /// The committed fixture's real H.264 or HEVC at `fps` (`FixtureVideo`): what the threaded test and `synthetic-take`
    /// decode. A keyframe (in GOP order, or asked for) is the fixture's first frame, and the frames after it follow it,
    /// so the file always decodes.
    Fixture(PathBuf, u32),
}

/// What the fake computer has and does. Times are ms after the pipe's `start`.
#[derive(Clone, Debug)]
pub struct Script {
    pub video: VideoFeed,
    /// What every fake sound pipe plays (default: 200 ms bursts a second, exact zeros between). A real mic never sends
    /// exact zeros (its noise floor), which recovery's trim of a power cut's zero pages relies on (`synthetic-take`
    /// plays a tone).
    pub sound: Signal,
    /// How long after its pipe starts a source sends its first packet or block (0 when absent).
    pub live_after_ms: BTreeMap<String, u32>,
    /// (ms after that source's pipe started, source key, event). Stalls, losses, stops and a gone target stop the
    /// source's media until `DeviceBack` or `TargetBack`; a video source then starts again on a keyframe.
    pub events: Vec<(u32, String, PipeEvent)>,
    /// The source's `start` fails with this.
    pub start_errors: BTreeMap<String, PipeError>,
    pub devices: Devices,
    /// Mics unplugged for a while: (device id, from ms, to ms) after the backend was made. Meanwhile the listing leaves
    /// the mic out and a pipe on it fails to start (`NoDevice`); a pipe recording on it as it goes sends `DeviceLost`
    /// then, and `DeviceBack` once it is listed again, as `CpalMic`'s once-a-second look does.
    pub unplugged: Vec<(String, u32, u32)>,
    /// What a mic pipe's start fails with while its device is unplugged (default `NoDevice`; an `Os` error is a device
    /// listed a moment before it can stream, as cpal reports it).
    pub unplugged_error: PipeError,
    /// Encoder ids whose start a video pipe refuses (VideoToolbox's −12900), as HEVC on a Mac whose probe cached it.
    pub refused_encoders: Vec<String>,
    pub caps: CaptureCaps,
    /// Screen Recording as `permissions()` reads it (default Granted; camera and mic are always Granted).
    pub screen_permission: Permission,
    /// A screen's cursor source (`Backend::cursor_source`); None: the screen's video pipe sends the cursor itself.
    pub cursor: Option<CursorScript>,
    /// The screen's pipe can show the cursor live (`VideoPipe::set_cursor_in_frames`); false: it refuses, and shows it
    /// from its next start.
    pub live_switch: bool,
    /// The live view's running stream doesn't take this many new view sizes (as one that didn't answer in time).
    pub live_view_refuses: u32,
    /// A target's first stills that fail (`Backend::still`), as one whose screenshot didn't come.
    pub still_fails: Vec<(Target, u32)>,
    /// The sound pipes add the sync blips to `sound` (default true; off for a pure tone, as the mic meter's test reads).
    pub sync_blips: bool,
}

impl Default for Script {
    /// A computer with one display, a camera and a mic, every source live at once, and everything available.
    fn default() -> Self {
        let device = |id: &str, name: &str| DeviceInfo {
            id: id.into(),
            name: name.into(),
            transport: Transport::BuiltIn,
            is_virtual: false,
            in_use: false,
            suspended: false,
        };
        Self {
            video: VideoFeed::Synthetic(video_spec(1920, 1080)),
            sound: Signal::Bursts { dbfs: -20.0 },
            live_after_ms: BTreeMap::new(),
            events: vec![],
            start_errors: BTreeMap::new(),
            devices: Devices {
                displays: vec![DisplayInfo {
                    id: "display-1".into(),
                    name: "Built-in Display".into(),
                    width: 2560,
                    height: 1600,
                    refresh_hz: 60,
                }],
                apps: vec![],
                cameras: vec![device("camera-1", "Built-in Camera")],
                mics: vec![device("mic-1", "Built-in Microphone")],
                output: Some(device("speakers-1", "Built-in Speakers")),
            },
            unplugged: vec![],
            unplugged_error: PipeError::NoDevice,
            refused_encoders: vec![],
            cursor: None,
            live_switch: true,
            live_view_refuses: 0,
            still_fails: vec![],
            sync_blips: true,
            caps: CaptureCaps {
                screen: ScreenApi::ScreenCaptureKit,
                app_audio: AppAudioApi::ScreenCaptureKit,
                hotkeys: HotkeyApi::Carbon,
                focus_events: true,
                picker: PickerKind::OurList,
                hud: true,
                os_supported: true,
                indicator: IndicatorPlace::MenuBar,
                cursor: CursorCaps {
                    hide: true,
                    positions: true,
                    shapes: true,
                    clicks: ClickSource::Events,
                },
            },
            screen_permission: Permission::Granted,
        }
    }
}

/// 30 fps with a keyframe and a flash every second.
pub(super) fn video_spec(width: u32, height: u32) -> SyntheticVideoSpec {
    SyntheticVideoSpec {
        fps: 30,
        gop_frames: 30,
        width,
        height,
        codec: Codec::H264,
        flash_every: Some(30),
        packet_bytes: 2_000,
    }
}
