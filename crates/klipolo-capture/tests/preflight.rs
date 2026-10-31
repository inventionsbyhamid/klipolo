//! The preflight's rules: each display decided on its own, with the camera on and off, the camera fitted beside
//! each quality, a profile's quality this computer lost, and the order of the notes.
#![cfg(feature = "synthetic")]
mod common;

use common::fake::{FakeCounting, display, encoder, t2_report};
use klipolo_capture::backend::{AppInfo, DeviceInfo, Permission, PresetId, ProbeReport, Transport};
use klipolo_capture::english::english;
use klipolo_capture::media::Codec;
use klipolo_capture::preflight::{Fitted, check_folder, profile_preset, setup, setup_with};
use klipolo_capture::probe::decide;
use klipolo_capture::profile::GameProfile;
use klipolo_platform::power::Battery;

/// One dedicated H.264 engine (about 276 Mpx/s flat out) for the screen and the camera, a 1920×1200 60 Hz display and
/// a 4K 120 Hz one.
fn shared_encoder() -> (FakeCounting, ProbeReport) {
    let mut backend = FakeCounting::new(vec![]);
    backend.devices.displays = vec![
        display("built-in", 1920, 1200, 60),
        display("4k", 3840, 2160, 120),
    ];
    let mut e = encoder("h264.media", Codec::H264, true, 60.0, 75.0);
    e.fps_1080p120 = 120.0;
    (
        backend,
        ProbeReport {
            encoders: vec![e],
            took_ms: 3_000,
        },
    )
}

#[test]
fn preflight_decides_per_display_and_fits_the_camera_beside_each_quality() {
    let (backend, report) = shared_encoder();
    let s = setup(
        &backend,
        Some(&report),
        Some(500_000_000_000),
        Some(40),
        None,
        false,
    );
    let [main, big] = &s.displays[..] else {
        panic!("one setup per display")
    };
    let (main_id, main, big) = (
        main.display.as_str(),
        main.with_camera.as_ref().unwrap(),
        big.with_camera.as_ref().unwrap(),
    );
    assert_eq!(
        (main_id, &s.choices, &s.offers),
        ("built-in", &main.choices, &main.offers)
    );
    let offer = |d: &Fitted, id| d.offers.iter().find(|o| o.id == id).cloned();
    let camera = |d, id| offer(d, id).unwrap().camera.map(|c| c.preset);
    // The built-in display keeps its own pixels at 60 beside a 1080p camera; Sharp would overload the engine, so the
    // camera drops to 720p there.
    let native = offer(main, PresetId::Native60).unwrap();
    assert_eq!(
        (native.width, native.height, native.best_native),
        (1920, 1200, true)
    );
    assert_eq!(camera(main, PresetId::Smooth), Some(PresetId::Camera1080));
    assert_eq!(camera(main, PresetId::Sharp), Some(PresetId::Camera720));
    assert!(offer(main, PresetId::SlowMo).is_none(), "a 60 Hz display");
    // The 4K display: its own pixels only at 30, and without the camera; tutorials there default to Smooth.
    assert_eq!(big.choices.native, None);
    let native = offer(big, PresetId::Native30).unwrap();
    assert_eq!(
        (native.width, native.height, native.camera.is_none()),
        (3840, 2160, true)
    );
    assert_eq!(
        native.camera_note.unwrap().code,
        "capture.preflight.cameraWithScreen"
    );
    assert!(offer(big, PresetId::Smooth).unwrap().best_native);
    assert!(offer(big, PresetId::SlowMo).is_some(), "a 120 Hz display");
    // An encoder that can't take the camera beside even Light says so.
    let igpu = ProbeReport {
        encoders: vec![encoder("h264.igpu", Codec::H264, false, 35.0, 35.0)],
        took_ms: 3_000,
    };
    let s = setup(&backend, Some(&igpu), None, None, None, false);
    let codes: Vec<&str> = s.notes.iter().map(|n| n.code.as_str()).collect();
    assert_eq!(codes, ["capture.preflight.cameraWithScreen"]);
    assert_eq!(backend.capture_calls(), 0);
}

#[test]
fn switching_the_camera_off_gives_back_the_screens_own_defaults() {
    // One dedicated H.264 engine that holds 1080p60 with little to spare (about 147 Mpx/s flat out).
    let mut backend = FakeCounting::new(vec![]);
    let report = ProbeReport {
        encoders: vec![encoder("h264.engine", Codec::H264, true, 60.0, 40.0)],
        took_ms: 3_000,
    };
    let s = setup(&backend, Some(&report), None, None, None, false);
    let main = &s.displays[0];
    let preset = |c: &Option<klipolo_capture::probe::ScreenChoice>| c.as_ref().map(|c| c.preset);
    let off = &main.no_camera.choices;
    assert_eq!(
        (
            preset(&off.screen),
            preset(&off.native),
            off.camera.is_none()
        ),
        (Some(PresetId::Smooth), Some(PresetId::Native30), true)
    );
    let on = &main.with_camera.as_ref().unwrap().choices;
    assert_eq!(
        (preset(&on.screen), preset(&on.native)),
        (Some(PresetId::Light), None),
        "the camera on the same engine steps the screen down"
    );
    assert_eq!(
        on.camera.as_ref().map(|c| c.preset),
        Some(PresetId::Camera720)
    );
    assert_eq!(&s.choices, on, "the camera is recorded by default");
    // A camera Klipolo may not use doesn't step the defaults down.
    backend.permissions.camera = Permission::Denied;
    let denied = setup(&backend, Some(&report), None, None, None, false);
    assert_eq!(
        denied.choices, *off,
        "camera permission denied: the camera-off defaults"
    );
    assert!(
        denied.displays[0].with_camera.is_some(),
        "the switch keeps its fit"
    );
    backend.permissions.camera = s.permissions.camera;
    let best_game = |f: &Fitted| f.offers.iter().find(|o| o.best_game).map(|o| o.id);
    assert_eq!(best_game(&main.no_camera), Some(PresetId::Smooth));
    assert_eq!(
        best_game(main.with_camera.as_ref().unwrap()),
        Some(PresetId::Light)
    );
    // A computer without a camera has only the camera-less defaults.
    backend.devices.cameras.clear();
    let s = setup(&backend, Some(&report), None, None, None, false);
    assert!(s.displays[0].with_camera.is_none());
    assert_eq!(s.choices, s.displays[0].no_camera.choices);
}

#[test]
fn a_profile_quality_this_computer_lost_falls_back_to_the_best() {
    let choices = decide(&t2_report(), 60, Some((2560, 1600)), true);
    let profile = |preset| GameProfile {
        name: "Hades".into(),
        preset,
        ..GameProfile::default()
    };
    let (preset, note) = profile_preset(&profile(Some(PresetId::Sharp)), &choices);
    assert_eq!(preset, Some(PresetId::Smooth));
    assert_eq!(
        english(&note.unwrap()),
        "The quality you chose for Hades isn't available on this computer now, so we picked the best one for it."
    );
    assert_eq!(
        profile_preset(&profile(Some(PresetId::Light)), &choices),
        (Some(PresetId::Light), None)
    );
    assert_eq!(
        profile_preset(&profile(None), &choices),
        (Some(PresetId::Smooth), None)
    );
}

#[test]
fn preflight_notes_come_in_order_and_a_network_folder_is_refused() {
    let virtual_mic = DeviceInfo {
        id: "BlackHole".into(),
        name: "BlackHole 2ch".into(),
        transport: Transport::Virtual,
        is_virtual: true,
        in_use: false,
        suspended: false,
    };
    let mut backend = FakeCounting::new(vec![virtual_mic]);
    backend.caps.os_supported = false;
    backend.permissions.screen = Permission::Denied;
    backend.devices.cameras[0].in_use = true;
    let game = |name: &str, frontmost| AppInfo {
        app_id: format!("com.studio.{name}"),
        name: name.into(),
        pid: 7,
        is_game: true,
        frontmost,
        exe: None,
    };
    backend.devices.apps = vec![game("old", false), game("hades", true)];
    let draining = Battery {
        percent: 15,
        charging: false,
    };
    let mut s = setup(
        &backend,
        Some(&t2_report()),
        Some(8_000_000_000),
        Some(900),
        Some(draining),
        false,
    );
    assert_eq!(
        s.suggested_game.as_ref().map(|g| g.name.as_str()),
        Some("hades")
    );
    let folder = check_folder(&tempfile::tempdir().unwrap().path().join("Recordings"));
    assert!(folder.locks && folder.write_ms.is_some(), "{folder:?}");
    s.refuse_folder();
    let codes: Vec<&str> = s.notes.iter().map(|n| n.code.as_str()).collect();
    assert_eq!(
        codes,
        [
            "capture.preflight.osTooOld",
            "capture.permission.screen",
            "capture.preflight.diskShort",
            "capture.preflight.slowDrive",
            "capture.preflight.networkFolder",
            "capture.preflight.virtualMic",
            "capture.preflight.cameraBusy",
            "capture.preflight.lowBattery",
        ]
    );
    let charging = Battery {
        percent: 15,
        charging: true,
    };
    let s = setup(
        &backend,
        Some(&t2_report()),
        None,
        None,
        Some(charging),
        false,
    );
    assert!(
        !s.notes
            .iter()
            .any(|n| n.code == "capture.preflight.lowBattery")
    );
}

/// The person's own mic is picked while it is connected; one that isn't is named, never replaced silently.
#[test]
fn the_person_s_saved_mic_is_picked_and_a_missing_one_is_named() {
    let mic = |id: &str, name: &str| DeviceInfo {
        id: id.into(),
        name: name.into(),
        transport: Transport::Usb,
        is_virtual: false,
        in_use: false,
        suspended: false,
    };
    let backend = FakeCounting::new(vec![
        mic("built-in", "MacBook Pro Microphone"),
        mic("mv7", "MV7"),
    ]);
    let s = setup_with(&backend, None, None, None, None, false, Some("mv7"));
    assert_eq!(s.mic.as_deref(), Some("mv7"));
    assert!(
        s.notes
            .iter()
            .all(|n| n.code != "capture.preflight.micMissing")
    );
    let s = setup_with(&backend, None, None, None, None, false, Some("yeti"));
    assert_eq!(s.mic.as_deref(), Some("built-in"));
    let missing = s
        .notes
        .iter()
        .find(|n| n.code == "capture.preflight.micMissing");
    assert_eq!(
        missing.map(english).as_deref(),
        Some("The microphone you chose isn't connected, so we picked MacBook Pro Microphone.")
    );
    assert_eq!(backend.capture_calls(), 0);
}
