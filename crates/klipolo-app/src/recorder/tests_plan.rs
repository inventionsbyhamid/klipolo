//! The Record dialog's data and a take's plan: the setup read from the fake backend without capture or prompts, the
//! encoder probe (once per app version, one at a time, never beside a take, never used when every encoder refused,
//! tried again once Klipolo opens again after one that gave nothing),
//! and each take's `session.json` from the person's options and the app's fixed choices, this Mac's encoders included.
//! The fake backend and fake workers only: nothing captures and nothing is asked of the OS.

use std::sync::atomic::Ordering;

use klipolo_capture::backend::{EncoderPick, Permission, PresetId, Target};
use klipolo_capture::media::Codec;
use klipolo_capture::preset::{native_preset, preset};
use klipolo_capture::probe::keeps_both;
use klipolo_capture::protocol::Command;
use klipolo_capture::session::ImportTarget;
use klipolo_capture::session::{SessionConfig, SourceSpec};
use klipolo_capture::shell::KeyBindings;
use klipolo_capture::synthetic::Script;
use klipolo_model::project::SourceRole;
use klipolo_platform::caps::OsFamily;

use super::state::RecorderState;
use super::test_support::*;
use super::types::WhatToRecord;

/// The sources a take records, by id, role and what each captures.
fn sources(cfg: &SessionConfig) -> Vec<(&str, SourceRole)> {
    (cfg.sources.iter())
        .map(|s| (s.id.as_str(), s.role))
        .collect()
}

/// What the take's screen source records.
fn screen_target(cfg: &SessionConfig) -> Target {
    (cfg.sources.iter())
        .find_map(|s| match &s.spec {
            SourceSpec::Screen(video) => Some(video.target.clone()),
            _ => None,
        })
        .expect("a screen source")
}

#[test]
fn setup_lists_devices_and_reads_permissions_without_asking() {
    let mut script = Script {
        screen_permission: Permission::NotAsked,
        ..Script::default()
    };
    script.devices.cameras.clear();
    let rec = fixture_with(script);
    let setup = rec.state.setup().unwrap();
    assert_eq!(setup.devices.displays.len(), 1);
    assert_eq!(setup.devices.mics.len(), 1);
    assert!(setup.devices.cameras.is_empty());
    assert_eq!(setup.permissions.screen, Permission::NotAsked);
    assert_eq!(
        setup.mic.as_deref(),
        Some("mic-1"),
        "the mic a take records"
    );
    assert_eq!(setup.camera, None, "no camera to offer");
    assert!(!setup.busy && !setup.own_take);
    assert!(!setup.offers.is_empty(), "the probe's offers");
    assert_eq!(
        rec.screen_requests.load(Ordering::SeqCst),
        0,
        "reading permissions never asks"
    );
    assert_eq!(rec.launch.spawned(), 0, "no worker");
    // Record isn't armed before Screen Recording is allowed: the worker's start would make macOS ask.
    let refused = rec.arm(options_screen()).unwrap_err();
    assert_eq!(refused.code, "capture.error.permission");
    assert_eq!(rec.launch.spawned(), 0);
    assert_eq!(rec.screen_requests.load(Ordering::SeqCst), 0);
}

/// While a take is the recorder's, the dialog says so and can stop it; no probe and no folder measure run beside it.
#[test]
fn setup_during_a_take_runs_no_probe_beside_it() {
    let rec = fixture();
    rec.arm(options_screen()).unwrap();
    let setup = rec.state.setup().unwrap();
    assert!(setup.busy && setup.own_take);
    assert_eq!(
        rec.probes.load(Ordering::SeqCst),
        0,
        "no probe beside a take"
    );
    assert!(setup.offers.is_empty(), "no report kept yet");
    // Once the take has ended, the next opening measures.
    rec.send(Command::Disarm).unwrap();
    rec.launch.exit(Some(3));
    rec.settle();
    assert!(!rec.state.setup().unwrap().offers.is_empty());
    assert_eq!(rec.probes.load(Ordering::SeqCst), 1);
}

/// Two openings at once (a quick close and reopen of the dialog) measure once: two probes would each see half the
/// engine.
#[test]
fn two_setups_at_once_run_one_probe() {
    let rec = fixture();
    std::thread::scope(|s| {
        let one = s.spawn(|| rec.state.setup().unwrap());
        let two = s.spawn(|| rec.state.setup().unwrap());
        let (one, two) = (one.join().unwrap(), two.join().unwrap());
        // Minutes left come from the free space at each read, which other work on the disk can move between the two.
        let report = |offers: &[klipolo_capture::preflight::PresetOffer]| {
            let mut offers = offers.to_vec();
            offers.iter_mut().for_each(|o| o.minutes_left = None);
            offers
        };
        assert_eq!(
            report(&one.offers),
            report(&two.offers),
            "both read the one report"
        );
    });
    assert_eq!(rec.probes.load(Ordering::SeqCst), 1);
}

/// A probe in which every encoder refused (as right after a restart) isn't kept, so the next opening measures again;
/// one that hung or crashed is marked, so the dialog doesn't wait for it again in this version.
#[test]
fn a_refused_probe_is_measured_again_and_a_failed_one_is_not() {
    let rec = fixture();
    let mut refused = probe_report();
    for encoder in &mut refused.encoders {
        (
            encoder.fps_1080p60,
            encoder.fps_1440p60,
            encoder.max_fps_1440p,
        ) = (0.0, 0.0, 0.0);
    }
    *rec.probe_writes.lock().unwrap() = Some(refused);
    assert!(rec.state.setup().unwrap().offers.is_empty());
    assert!(!rec.files.probe().exists(), "not kept");
    *rec.probe_writes.lock().unwrap() = Some(probe_report());
    assert!(
        !rec.state.setup().unwrap().offers.is_empty(),
        "measured again"
    );
    assert_eq!(rec.probes.load(Ordering::SeqCst), 2);

    let rec = fixture();
    *rec.probe_writes.lock().unwrap() = None;
    rec.state.setup().unwrap();
    rec.state.setup().unwrap();
    assert_eq!(rec.probes.load(Ordering::SeqCst), 1, "not waited for again");
    assert!(rec.files.probe_failed().exists(), "marked, for diagnosis");
}

/// A probe that hung or crashed is waited for once per opening of Klipolo, not once per version: one bad moment (a
/// restart, a busy engine) never pins "no qualities" until the next update. Its mark goes once one measures.
#[test]
fn a_probe_that_gave_nothing_runs_again_once_klipolo_opens_again() {
    let dir = tempfile::tempdir().unwrap();
    let (parts, before) = world(dir.path(), Script::default());
    *before.probe_writes.lock().unwrap() = None;
    let first = RecorderState::build(false, || parts);
    first.setup().unwrap();
    first.setup().unwrap();
    assert_eq!(before.probes.load(Ordering::SeqCst), 1);
    assert!(before.files.probe_failed().exists());

    // Klipolo opens again: a new recorder over the same folders.
    let (parts, again) = world(dir.path(), Script::default());
    let reopened = RecorderState::build(false, || parts);
    let setup = reopened.setup().unwrap();
    assert_eq!(again.probes.load(Ordering::SeqCst), 1, "measured again");
    assert!(!setup.offers.is_empty());
    assert!(again.files.probe().exists());
    assert!(!again.files.probe_failed().exists(), "its mark goes");
}

/// A kept report in which every encoder refused (an earlier build kept such reports) is measured again rather than
/// used; a report with no encoder at all (a computer without a hardware one) is kept, not measured on every opening.
#[test]
fn a_kept_refused_report_is_measured_again_and_one_without_encoders_is_kept() {
    let rec = fixture();
    let mut refused = probe_report();
    for encoder in &mut refused.encoders {
        (encoder.fps_1080p60, encoder.max_fps_1440p) = (0.0, 0.0);
    }
    std::fs::create_dir_all(&rec.files.dir).unwrap();
    std::fs::write(rec.files.probe(), serde_json::to_vec(&refused).unwrap()).unwrap();
    assert!(
        !rec.state.setup().unwrap().offers.is_empty(),
        "measured now"
    );
    assert_eq!(rec.probes.load(Ordering::SeqCst), 1);

    let rec = fixture();
    let mut none = probe_report();
    none.encoders.clear();
    *rec.probe_writes.lock().unwrap() = Some(none);
    rec.state.setup().unwrap();
    rec.state.setup().unwrap();
    assert_eq!(rec.probes.load(Ordering::SeqCst), 1, "kept after the first");
    assert!(rec.files.probe().exists());
}

#[test]
fn the_encoder_probe_runs_once_per_app_version() {
    let rec = fixture();
    rec.state.setup().unwrap();
    rec.state.setup().unwrap();
    assert_eq!(rec.probes.load(Ordering::SeqCst), 1, "kept after the first");
    let kept: Vec<String> = std::fs::read_dir(&rec.files.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("probe-"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(
        kept[0].ends_with(&format!("-{}.json", env!("CARGO_PKG_VERSION"))),
        "{kept:?}"
    );
    assert_eq!(rec.backends.load(Ordering::SeqCst), 1, "one backend");
}

#[test]
fn config_records_the_display_and_mic_with_a_3_s_countdown() {
    let rec = fixture();
    let setup = rec.state.setup().unwrap();
    let id = rec.arm(options_screen()).unwrap().id;
    let cfg = rec.config(&id);
    assert_eq!(cfg.id, id);
    assert_eq!(
        sources(&cfg),
        [("screen", SourceRole::Screen), ("mic", SourceRole::Mic)]
    );
    assert_eq!(
        screen_target(&cfg),
        Target::Display {
            id: "display-1".into()
        }
    );
    let mic = cfg.sources.iter().find(|s| s.id == "mic").unwrap();
    assert_eq!(
        mic.spec,
        SourceSpec::Mic {
            device: "mic-1".into()
        }
    );
    assert_eq!(cfg.countdown_s, 3);
    assert_eq!(cfg.keys, KeyBindings::defaults(OsFamily::Macos));
    let keys = [&cfg.keys.start_stop, &cfg.keys.pause, &cfg.keys.marker];
    assert_eq!(
        keys,
        ["Alt+Super+KeyR", "Alt+Super+KeyP", "Alt+Super+KeyM"],
        "Option Command R, P and M"
    );
    assert!(
        cfg.words.is_empty(),
        "the worker's menu and HUD read English"
    );
    assert!(!cfg.camera_bubble);
    // No computer sound, and the quality the setup offered first for this display.
    assert!(
        !cfg.sources
            .iter()
            .any(|s| s.role == SourceRole::SystemAudio)
    );
    let offered = (setup.displays[0].no_camera.offers.iter())
        .find(|o| o.best_native)
        .map(|o| o.id)
        .expect("an offer for a whole screen");
    let preset = cfg.sources.iter().find_map(|s| match &s.spec {
        SourceSpec::Screen(video) => Some((video.preset, video.sound)),
        _ => None,
    });
    assert_eq!(preset, Some((offered, false)));
}

#[test]
fn camera_choice_adds_a_camera_source_and_the_bubble() {
    let rec = fixture();
    let mut options = options_screen();
    options.camera = Some("camera-1".into());
    options.camera_bubble = true;
    options.mirror_camera = false;
    let id = rec.arm(options.clone()).unwrap().id;
    let cfg = rec.config(&id);
    assert_eq!(
        sources(&cfg),
        [
            ("screen", SourceRole::Screen),
            ("camera", SourceRole::Camera),
            ("mic", SourceRole::Mic)
        ]
    );
    let camera = cfg.sources.iter().find(|s| s.id == "camera").unwrap();
    assert!(
        matches!(&camera.spec, SourceSpec::Camera { device, .. } if device == "camera-1"),
        "{:?}",
        camera.spec
    );
    assert!(cfg.camera_bubble);
    assert!(!cfg.mirror_camera, "the person's mirror choice");
    rec.send(klipolo_capture::protocol::Command::Disarm)
        .unwrap();
    rec.launch.exit(Some(3));
    rec.settle();
    // Mirrored; and no bubble when the person turns it off.
    options.mirror_camera = true;
    options.camera_bubble = false;
    let id = rec.arm(options).unwrap().id;
    let cfg = rec.config(&id);
    assert!(cfg.mirror_camera && !cfg.camera_bubble);
}

#[test]
fn no_microphone_records_the_screen_alone() {
    let rec = fixture();
    let mut options = options_screen();
    options.mic = None;
    // A bubble needs a camera.
    options.camera_bubble = true;
    let id = rec.arm(options).unwrap().id;
    let cfg = rec.config(&id);
    assert_eq!(sources(&cfg), [("screen", SourceRole::Screen)]);
    assert!(!cfg.camera_bubble);
}

#[test]
fn app_choice_records_that_apps_windows() {
    let rec = fixture();
    let mut options = options_screen();
    options.what = WhatToRecord::App {
        app_id: "com.example.slides".into(),
        display: "display-1".into(),
    };
    let id = rec.arm(options).unwrap().id;
    let cfg = rec.config(&id);
    assert_eq!(
        screen_target(&cfg),
        Target::App {
            app_id: "com.example.slides".into(),
            display: "display-1".into()
        }
    );
    assert_eq!(
        sources(&cfg),
        [("screen", SourceRole::Screen), ("mic", SourceRole::Mic)],
        "no app sound"
    );
}

#[test]
fn ids_the_os_could_not_have_given_are_refused_before_a_worker() {
    let rec = fixture();
    let mut options = options_screen();
    options.mic = Some("mic\n1".into());
    assert_eq!(rec.arm(options).unwrap_err().code, "capture.error.noDevice");
    assert_eq!(rec.launch.spawned(), 0);
}

/// This Mac's probe (H.264 on the Intel GPU only) with the camera on: the screen records at the quality marked best
/// beside the camera, the camera at the preset fitted with it, both on the one engine and within what it holds, and each
/// tries the probe's encoder.
#[test]
fn a_camera_take_on_this_mac_fits_the_screen_and_camera_on_its_one_engine() {
    let rec = fixture();
    let setup = rec.state.setup().unwrap();
    let with_camera = setup.displays[0].with_camera.clone().expect("a camera fit");
    let best = (with_camera.offers.iter())
        .find(|o| o.best_native)
        .expect("a best offer with the camera");
    let mut options = options_screen();
    options.camera = Some("camera-1".into());
    let armed = rec.arm(options).unwrap();
    assert_eq!(armed.notes, None, "the camera fits");
    let cfg = rec.config(&armed.id);
    let screen = cfg.sources.iter().find_map(|s| match &s.spec {
        SourceSpec::Screen(video) => Some((video.preset, video.encoders.clone())),
        _ => None,
    });
    let camera = cfg.sources.iter().find_map(|s| match &s.spec {
        SourceSpec::Camera {
            preset, encoders, ..
        } => Some((*preset, encoders.clone())),
        _ => None,
    });
    let (screen_preset, screen_encoders) = screen.expect("a screen source");
    let (camera_preset, camera_encoders) = camera.expect("a camera source");
    assert_eq!(screen_preset, best.id);
    assert_eq!(Some(camera_preset), best.camera.as_ref().map(|c| c.preset));
    let gva = probe_report().encoders[0].clone();
    let display = (2560, 1600);
    assert!(
        keeps_both(
            &gva,
            &native_preset(screen_preset, Codec::H264, display),
            &preset(camera_preset, Codec::H264)
        ),
        "{screen_preset:?} and {camera_preset:?} on gva"
    );
    let gva_pick = EncoderPick {
        codec: Codec::H264,
        id: "h264.gva".into(),
    };
    assert_eq!(screen_encoders, std::slice::from_ref(&gva_pick));
    assert_eq!(camera_encoders, [gva_pick]);
}

/// An offer with no room for the camera records the screen alone, and the person is told before Record.
#[test]
fn a_quality_without_room_for_the_camera_records_the_screen_alone_and_says_so() {
    let rec = fixture();
    rec.state.setup().unwrap();
    let mut shown = rec.state.shown().expect("the dialog's setup");
    let note = klipolo_model::coded::CodedError::new("capture.preflight.cameraWithScreen");
    let with_camera = shown.setup.displays[0].with_camera.as_mut().unwrap();
    for offer in &mut with_camera.offers {
        (offer.camera, offer.camera_note) = (None, Some(note.clone()));
    }
    let mut options = options_screen();
    options.camera = Some("camera-1".into());
    options.camera_bubble = true;
    let (cfg, notes) = super::setup::plan(&shown, &options, "t", ImportTarget::New, 0).unwrap();
    assert_eq!(notes, [note]);
    assert_eq!(
        sources(&cfg),
        [("screen", SourceRole::Screen), ("mic", SourceRole::Mic)]
    );
    assert!(!cfg.camera_bubble, "no bubble without the camera");
    let alone = (shown.setup.displays[0].no_camera.offers.iter())
        .find(|o| o.best_native)
        .map(|o| o.id);
    let screen_preset = cfg.sources.iter().find_map(|s| match &s.spec {
        SourceSpec::Screen(video) => Some(video.preset),
        _ => None,
    });
    assert_eq!(screen_preset, alone, "the screen alone's quality");
    assert_ne!(screen_preset, Some(PresetId::Camera720));
}
