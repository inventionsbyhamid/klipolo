//! The Record dialog's data and a take's plan: the setup read from the fake backend without capture or prompts, the
//! encoder probe once per app version, and each take's `session.json` from the person's options and the app's fixed
//! choices. The fake backend and fake workers only: nothing captures and nothing is asked of the OS.

use std::sync::atomic::Ordering;

use klipolo_capture::backend::{Permission, Target};
use klipolo_capture::session::{SessionConfig, SourceSpec};
use klipolo_capture::shell::KeyBindings;
use klipolo_capture::synthetic::Script;
use klipolo_model::project::SourceRole;
use klipolo_platform::caps::OsFamily;

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
    // While a take runs, the dialog says so and can stop it.
    rec.arm(options_screen()).unwrap();
    let setup = rec.state.setup().unwrap();
    assert!(setup.busy && setup.own_take);
    assert_eq!(rec.screen_requests.load(Ordering::SeqCst), 0);
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
