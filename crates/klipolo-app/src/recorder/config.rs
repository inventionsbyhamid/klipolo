//! A take's fixed config (`session.json`) from the person's options and what the app chose for them. Pure: the
//! choices (`TakeChoices`) come from the setup (`setup::plan`).
//!
//! One source per file: the screen (a whole display, or one app's windows), the camera and the mic. No computer or
//! app sound. Each label is its `capture.source.*` code, never worded text, and each id is stable and role-like
//! (`screen`, `camera`, `mic`), so the page words a gate, meter or health event's source by its id. "Record yourself"
//! (`WhatToRecord::Camera`) is a Camera take: the camera and the mic, no screen, and the camera is its main picture.
//!
//! **Fixed by the app:** the quality the setup marked best for the display (and the probe's encoders for it), no
//! sound, macOS's shortcuts (Option Command R, P and M), a 3 s countdown, no words for the worker (it reads English),
//! and where the take lands.

use klipolo_capture::backend::{CursorCaps, EncoderPick, PresetId, Target, VideoSpec};
use klipolo_capture::preflight::cursor::cursor_plan;
use klipolo_capture::profile::Corner;
use klipolo_capture::session::{ImportTarget, SessionConfig, SourceConfig, SourceSpec};
use klipolo_capture::shell::KeyBindings;
use klipolo_model::project::{RecordingKind, SourceRole};
use klipolo_platform::caps::OsFamily;

use super::types::{ArmOptions, WhatToRecord};
use super::words::worker_words;

/// The countdown after Record ("Starting in 3, 2, 1").
pub const COUNTDOWN_S: u32 = 3;
/// The disk outlook's expectation of a take's length.
const EXPECTED_MINUTES: u32 = 60;

/// What the app chose for a take beyond the person's options.
#[derive(Clone, Debug, PartialEq)]
pub struct TakeChoices {
    /// The screen's quality (unused by a take of the camera alone).
    pub preset: PresetId,
    /// The camera's, fitted beside the screen's, or the camera's own best alone (`probe::camera_alone`).
    pub camera_preset: PresetId,
    /// What this computer can do with a screen's cursor: with the gate, whether it is kept apart from the frames.
    pub cursor_caps: CursorCaps,
    pub created_ms: u64,
    /// This OS, for the take's diagnostics.
    pub os: String,
    /// The open project, or a new one made when the take lands.
    pub target: ImportTarget,
    /// The encoders the screen tries, in order (`probe::encoders_for`); empty: the backend's own pick, its first
    /// hardware H.264 encoder.
    pub encoders: Vec<EncoderPick>,
    /// The encoders the camera tries, in order (`probe::camera_encoders_for`); empty: the worker's own order.
    pub camera_encoders: Vec<EncoderPick>,
}

/// The config of take `id`.
pub fn session_config(o: &ArmOptions, id: &str, c: &TakeChoices) -> SessionConfig {
    let source = |id: &str, role, spec| SourceConfig {
        id: id.into(),
        role,
        label: format!("capture.source.{id}"),
        spec,
    };
    let screen = take_target(&o.what).map(|target| {
        SourceSpec::Screen(VideoSpec {
            target,
            preset: c.preset,
            cursor: cursor_plan(RecordingKind::Screen, &c.cursor_caps, true),
            sound: false,
            encoders: c.encoders.clone(),
            bubble: false,
            bubble_mirrored: false,
        })
    });
    let kind = if screen.is_some() {
        RecordingKind::Screen
    } else {
        RecordingKind::Camera
    };
    let mut sources: Vec<SourceConfig> = (screen.into_iter())
        .map(|screen| source("screen", SourceRole::Screen, screen))
        .collect();
    if let Some(device) = &o.camera {
        let camera = SourceSpec::Camera {
            device: device.clone(),
            preset: c.camera_preset,
            encoders: c.camera_encoders.clone(),
        };
        sources.push(source("camera", SourceRole::Camera, camera));
    }
    if let Some(device) = &o.mic {
        let mic = SourceSpec::Mic {
            device: device.clone(),
        };
        sources.push(source("mic", SourceRole::Mic, mic));
    }
    SessionConfig {
        id: id.into(),
        created_ms: c.created_ms,
        kind,
        sources,
        countdown_s: COUNTDOWN_S,
        auto_pause: None,
        target: c.target.clone(),
        game: None,
        app_version: env!("CARGO_PKG_VERSION").into(),
        os: c.os.clone(),
        expected_minutes: EXPECTED_MINUTES,
        words: worker_words(),
        keys: KeyBindings::defaults(OsFamily::Macos),
        hud_corner: Corner::default(),
        // The bubble shows the camera over a recorded screen: a take of the camera alone has none.
        camera_bubble: o.camera_bubble && o.camera.is_some() && kind == RecordingKind::Screen,
        mirror_camera: o.mirror_camera,
    }
}

/// What a take of `what` captures for its screen source: a display without Klipolo's own windows, or one app's
/// windows on its display. None for the camera alone, which records no screen.
pub fn take_target(what: &WhatToRecord) -> Option<Target> {
    match what {
        WhatToRecord::Screen { display } => Some(Target::Display {
            id: display.clone(),
        }),
        WhatToRecord::App { app_id, display } => Some(Target::App {
            app_id: app_id.clone(),
            display: display.clone(),
        }),
        WhatToRecord::Camera => None,
    }
}
