//! Preflight: what the recorder screen shows before Record, from the backend's
//! no-capture answers only (devices from the OS's registries, permissions without prompts) and the cached probe. It
//! never asks for a pipe, a preview or a probe run.
//!
//! - **Per display**: the native size and Slow motion follow the display recorded, so each listed
//!   display gets its own `decide` and offers; `choices` and `offers` are the main display's (the first listed).
//! - **With and without the camera**: `decide` runs once with the camera off and, on a computer with
//!   a camera, once with it on, so switching the camera in the recorder changes the defaults at once without asking
//!   again. With the camera off a take keeps what the screen alone can hold (Smooth, Native60); with it on, the
//!   camera may step them down where they share an encoder. `choices` and `offers` are the camera-on ones when there
//!   is a camera (it is recorded by default), and so are the notes.
//! - **Offers**: every quality this computer can sustain there, with what a take at it writes (GB an hour) and the
//!   minutes of space it leaves above the 2 GB floor. A take counts the screen, the camera the defaults record (when
//!   there is one), the mic and one sound track: game sound is on by default for games, and counting it for every
//!   offer errs towards less time left. Each offer carries its own camera: where the camera shares that quality's
//!   encoder, it is checked with `probe::keeps_both`, stepped down, or left out with `capture.preflight.cameraWithScreen`
//!   (only ever down from the defaults' camera, so nothing overloads an encoder).
//! - **Notes** in this order: the OS, the encoder, the cursor, the disk and the drive, the mic, the camera, the battery.
//!
//! Cost: at most four `decide`s per display (each camera state and the screen alone), O(displays × presets).

pub mod cursor;
mod folder;
mod offers;

pub use folder::{FolderCheck, check_folder};

use klipolo_model::coded::CodedError;
use klipolo_platform::caps::OsFamily;
use klipolo_platform::power::Battery;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::backend::{
    AppInfo, Backend, CaptureCaps, Devices, DisplayInfo, Permission, Permissions, PresetId,
    ProbeReport,
};
use crate::disk::{STOP_BYTES, WARN_MINUTES};
use crate::english::msg;
use crate::mic::pick_mic;
use crate::probe::{CameraChoice, Choices};
use crate::profile::GameProfile;
use offers::{Load, fit};

/// Writing and flushing 1 MB slower than this gives `capture.preflight.slowDrive`.
pub const SLOW_WRITE_MS: u32 = 500;
/// Below this charge, while draining, `capture.preflight.lowBattery`.
pub const LOW_BATTERY_PERCENT: u8 = 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecorderSetup {
    pub caps: CaptureCaps,
    pub devices: Devices,
    pub permissions: Permissions,
    /// The main display's choices (the first listed; without a display, those without a native size), with the camera
    /// on when this computer has one. Default (all None) until the probe has run.
    pub choices: Choices,
    /// The main display's offers, as `choices`.
    pub offers: Vec<PresetOffer>,
    /// Each listed display's own choices and offers, with the camera off and on.
    pub displays: Vec<DisplaySetup>,
    /// The mic a take records by default (`mic::pick_mic`), by id.
    pub mic: Option<String>,
    /// The camera the recorder opens with, by id: the first listed until the person chose (the app puts their last
    /// choice here). None: "No camera".
    pub camera: Option<String>,
    /// "Mirror my camera": on until the person turns it off (the app puts their last choice here).
    pub mirror_camera: bool,
    pub free_bytes: Option<u64>,
    pub notes: Vec<CodedError>,
    /// A take is running.
    pub busy: bool,
    /// The running take is this editor's own, so "Stop the running recording" reaches it. False when nothing runs, and
    /// for a take an earlier run left recording, which stops from its own item. The app sets
    /// it; `setup` leaves it false.
    pub own_take: bool,
    /// This OS has camera and microphone privacy pages the recorder can open ("Open privacy settings").
    /// The app sets it, as `own_take`.
    pub privacy_settings: bool,
    /// macOS still refuses Screen Recording after the step's Reopen Klipolo: the step explains a stale entry
    /// in its list instead of asking again. The app sets it, as `own_take`.
    pub screen_refused_after_reopen: bool,
    /// The frontmost app that looks like a game: "{game} is running. Record it?" (never started by itself).
    pub suggested_game: Option<AppInfo>,
}

/// One display's choices and offers, with the camera off and on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DisplaySetup {
    /// `DisplayInfo::id`.
    pub display: String,
    /// "No camera": the screen alone keeps its own defaults.
    pub no_camera: Fitted,
    /// The camera recorded beside the screen. None on a computer without a camera.
    pub with_camera: Option<Fitted>,
}

/// The probe's choices for one display and camera state, and the offers they give.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CaptureFitted")]
pub struct Fitted {
    pub choices: Choices,
    pub offers: Vec<PresetOffer>,
}

/// `best_native`: the default for whole-screen and window recordings (the native choice, else the screen choice they
/// fall back to); `best_game`: for games. The recorder marks "Best for this computer" by what is being
/// recorded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PresetOffer {
    pub id: PresetId,
    /// What it records: the display's own pixels for the native presets ("Native ({width} × {height}, {fps} fps)").
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub gb_per_hour: f32,
    /// Whole minutes until the 2 GB floor at this quality; None when the free space isn't known.
    pub minutes_left: Option<u32>,
    pub best_native: bool,
    pub best_game: bool,
    /// The camera recorded beside this quality; None: no camera at it.
    pub camera: Option<CameraChoice>,
    /// Why there is no camera at this quality although there is one (`capture.preflight.cameraWithScreen`).
    pub camera_note: Option<CodedError>,
}

/// `write_ms`: how long writing and fully flushing 1 MB under the recordings root took (`check_folder`, which the app
/// runs); over 500 ms gives the `slowDrive` note. `probe`: the cached probe report; None until it has run
/// (no choices or offers then, and no encoder note).
pub fn setup(
    backend: &dyn Backend,
    probe: Option<&ProbeReport>,
    free_bytes: Option<u64>,
    write_ms: Option<u32>,
    battery: Option<Battery>,
    busy: bool,
) -> RecorderSetup {
    setup_with(backend, probe, free_bytes, write_ms, battery, busy, None)
}

/// `setup` with the person's own mic, `wanted` (the one their last take recorded, by id), through `pick_mic`: picked
/// while it is connected, and never replaced silently when it isn't (`capture.preflight.micMissing` names the mic
/// picked instead). The app's recorder screen calls this.
pub fn setup_with(
    backend: &dyn Backend,
    probe: Option<&ProbeReport>,
    free_bytes: Option<u64>,
    write_ms: Option<u32>,
    battery: Option<Battery>,
    busy: bool,
    wanted: Option<&str>,
) -> RecorderSetup {
    let caps = backend.caps();
    let devices = backend.devices();
    let permissions = backend.permissions();
    let (mic, mic_note) = pick_mic(&devices.mics, devices.output.as_ref(), wanted);
    let has_camera = !devices.cameras.is_empty();
    let camera = devices.cameras.first().map(|c| c.id.clone());
    let load = Load {
        mono: u32::from(mic.is_some()),
        free_bytes,
    };
    let displays: Vec<DisplaySetup> = probe.map_or_else(Vec::new, |report| {
        let one = |d: &DisplayInfo| DisplaySetup {
            display: d.id.clone(),
            no_camera: fit(report, Some(d), &load, false),
            with_camera: has_camera.then(|| fit(report, Some(d), &load, true)),
        };
        devices.displays.iter().map(one).collect()
    });
    // The camera is recorded by default when there is one Klipolo may use: with its permission
    // denied, the defaults are the camera-off ones, though the camera-on fit stays in `displays` for the switch.
    let camera_on = has_camera && permissions.camera != Permission::Denied;
    let main = match (displays.first(), probe) {
        (Some(main), _) => match &main.with_camera {
            Some(with) if camera_on => with.clone(),
            _ => main.no_camera.clone(),
        },
        (None, Some(report)) => fit(report, None, &load, camera_on),
        (None, None) => Fitted::default(),
    };
    let Fitted { choices, offers } = main;

    let mut notes = Vec::new();
    if !caps.os_supported {
        let (os, version) = os_floor(klipolo_platform::caps::current().os.family);
        notes.push(msg!(
            "capture.preflight.osTooOld",
            os = os,
            version = version
        ));
    }
    if permissions.screen == Permission::Denied {
        notes.push(msg!("capture.permission.screen"));
    }
    // A computer whose encoders can't hold the games' rules may still keep a small display's own size.
    let no_video = choices.screen.is_none()
        && choices.native.is_none()
        && displays
            .iter()
            .all(|d| d.no_camera.choices.native.is_none());
    if probe.is_some() && no_video {
        notes.push(msg!("capture.preflight.noEncoder"));
    }
    notes.extend(cursor::cursor_note(&caps.cursor));
    if let Some(free) = free_bytes {
        let short = offers.iter().any(|o| {
            (o.best_native || o.best_game) && o.minutes_left.is_some_and(|m| m < WARN_MINUTES)
        });
        if free <= STOP_BYTES {
            notes.push(msg!("capture.preflight.diskBlock"));
        } else if short {
            notes.push(msg!("capture.preflight.diskShort"));
        }
    }
    if write_ms.is_some_and(|ms| ms > SLOW_WRITE_MS) {
        notes.push(msg!("capture.preflight.slowDrive"));
    }
    notes.extend(mic_note);
    let picked = mic
        .as_deref()
        .and_then(|id| devices.mics.iter().find(|m| m.id == id));
    if let Some(m) = picked.filter(|m| m.is_virtual) {
        notes.push(msg!(
            "capture.preflight.virtualMic",
            device = m.name.as_str()
        ));
    }
    if picked.is_some() && permissions.mic == Permission::Denied {
        notes.push(msg!("capture.permission.mic"));
    }
    if let Some(camera) = devices.cameras.first() {
        if camera.in_use {
            notes.push(msg!(
                "capture.preflight.cameraBusy",
                device = camera.name.as_str()
            ));
        }
        if camera.suspended {
            notes.push(msg!(
                "capture.preflight.cameraOff",
                device = camera.name.as_str()
            ));
        }
        if permissions.camera == Permission::Denied {
            notes.push(msg!("capture.permission.camera"));
        }
    }
    notes.extend(choices.camera_note.clone());
    if battery.is_some_and(|b| b.percent < LOW_BATTERY_PERCENT && !b.charging) {
        notes.push(msg!("capture.preflight.lowBattery"));
    }

    RecorderSetup {
        suggested_game: devices
            .apps
            .iter()
            .find(|a| a.frontmost && a.is_game)
            .cloned(),
        caps,
        devices,
        permissions,
        choices,
        offers,
        displays,
        mic,
        camera,
        mirror_camera: true,
        free_bytes,
        notes,
        busy,
        own_take: false,
        privacy_settings: false,
        screen_refused_after_reopen: false,
    }
}

/// The quality a game's profile records at: its stored preset while this computer can still record it
/// (`Choices::available`; an unplugged eGPU or a hand edit can leave one behind), else "Best for this computer", the
/// probe's choice for games, with `capture.preflight.profilePreset` naming the game. A profile without a preset takes
/// the best quietly. The profile keeps the person's choice, for when the computer can record it again. `choices`: the
/// game's display's, for the take's camera state (`DisplaySetup`).
pub fn profile_preset(
    profile: &GameProfile,
    choices: &Choices,
) -> (Option<PresetId>, Option<CodedError>) {
    let best = choices.screen.as_ref().map(|s| s.preset);
    match profile.preset {
        Some(p) if choices.available.contains(&p) => (Some(p), None),
        Some(_) => {
            let app = if profile.name.is_empty() {
                &profile.app_id
            } else {
                &profile.name
            };
            let note = msg!("capture.preflight.profilePreset", app = app.as_str());
            (best, Some(note))
        }
        None => (best, None),
    }
}

/// The oldest OS each backend records on, for `capture.preflight.osTooOld`: macOS 15 (ScreenCaptureKit through cidre's
/// `macos_15_0`); Windows 10 version 1903 (Windows Graphics Capture); on
/// Linux it is the ScreenCast portal with restore tokens.
fn os_floor(family: OsFamily) -> (&'static str, &'static str) {
    match family {
        OsFamily::Macos => ("macOS", "15"),
        OsFamily::Windows => ("Windows 10", "version 1903"),
        OsFamily::Linux => ("xdg-desktop-portal", "1.12"),
    }
}
