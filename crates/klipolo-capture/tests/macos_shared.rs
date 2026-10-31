//! The macOS preflight's pure parts: Core Audio transports, the facts that mark a game, the folders whose
//! bundles are never read, and the no-capture rule checked on the source. Nothing here lists a device, so it runs on
//! every OS, the Linux CI runs included. Also VideoToolbox's encoder ids and the probe's
//! schedule, and ScreenCaptureKit's picture size and still-screen rules.
use klipolo_capture::backend::Transport;
use klipolo_capture::platform::common::mac_preflight::{
    app_facts, reads_without_asking, transport_of,
};
use klipolo_capture::platform::common::probe_plan::{Measure, infer_1440p60, plan, total};
use klipolo_capture::platform::common::sck::{
    CURSOR_BEAT_NS, KEYFRAME_EVERY_NS, cursor_beat_due, fit_size, frame_interval_parts,
    keyframe_due,
};
use klipolo_capture::platform::common::vt_ids::{dedicated_on_mac, engine_on_mac};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

#[test]
fn core_audio_transports_and_virtual_names_are_classified() {
    let code = |s: &[u8; 4]| u32::from_be_bytes(*s);
    assert_eq!(
        transport_of(code(b"bltn"), "MacBook Pro Microphone"),
        Transport::BuiltIn
    );
    assert_eq!(transport_of(code(b"blue"), "AirPods"), Transport::Bluetooth);
    assert_eq!(transport_of(code(b"usb "), "MV7"), Transport::Usb);
    assert_eq!(
        transport_of(code(b"virt"), "BlackHole 2ch"),
        Transport::Virtual
    );
    assert_eq!(
        transport_of(code(b"usb "), "Microsoft Teams Audio"),
        Transport::Virtual,
        "known virtual devices by name"
    );
}

#[test]
fn a_game_category_or_game_mode_marks_a_game() {
    let mut plist = BTreeMap::new();
    plist.insert(
        "LSApplicationCategoryType".to_string(),
        "public.app-category.action-games".to_string(),
    );
    assert!(klipolo_capture::games::looks_like_game(&app_facts(
        "com.studio.game",
        &plist,
        None
    )));
    let steam = app_facts(
        "com.valvesoftware.steam.app",
        &BTreeMap::new(),
        Some(
            "/Users/sam/Library/Application Support/Steam/steamapps/common/Hades/Hades.app/Contents/MacOS/Hades",
        ),
    );
    assert!(klipolo_capture::games::looks_like_game(&steam));
    // The launchers themselves are never suggested, though their category says games (the launcher exclusions).
    for launcher in [
        "com.valvesoftware.steam",
        "com.epicgames.EpicGamesLauncher",
        "com.gog.galaxy",
    ] {
        let facts = app_facts(launcher, &plist, None);
        assert!(!klipolo_capture::games::looks_like_game(&facts), "{launcher}");
    }
}

/// Reading a bundle's `Info.plist` in a folder macOS guards would ask the person for access: preflight never asks.
#[test]
fn a_bundle_in_a_guarded_folder_is_never_read() {
    let home = Path::new("/Users/sam");
    for guarded in [
        "/Users/sam/Downloads/Game.app",
        "/Users/sam/Desktop/Game.app",
        "/Users/sam/Documents/Games/Game.app",
        "/Users/sam/Library/Mobile Documents/com~apple~CloudDocs/Game.app",
        "/Volumes/Games/Game.app",
        "/private/var/folders/ab/c1d2/T/AppTranslocation/0A1B2C3D/d/Game.app",
    ] {
        assert!(!reads_without_asking(Path::new(guarded), home), "{guarded}");
    }
    for open in [
        "/Applications/Game.app",
        "/Users/sam/Applications/Game.app",
        "/Users/sam/Library/Application Support/Steam/steamapps/common/Hades/Hades.app",
    ] {
        assert!(reads_without_asking(Path::new(open), home), "{open}");
    }
}

/// The no-capture rule, checked on the source: preflight's files never name ScreenCaptureKit's content or stream APIs
/// (macOS 15 shows its monthly prompt when `SCShareableContent` is called) nor open a capture session.
#[test]
fn preflight_code_never_touches_capture_apis() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/platform/macos");
    for file in [
        "devices.rs",
        "displays.rs",
        "permissions.rs",
        "apps.rs",
        "mod.rs",
    ] {
        let text = std::fs::read_to_string(dir.join(file)).unwrap();
        for banned in [
            "ShareableContent",
            "sc::Stream",
            "CaptureSession",
            "startRunning",
            "build_input_stream",
        ] {
            assert!(!text.contains(banned), "{file} names {banned}");
        }
    }
}

/// VideoToolbox's ids: on Intel only the T2's AVE runs apart from the iGPU, and Quick Sync's two ids are one
/// engine; on Apple silicon every hardware encoder is the media engine.
#[test]
fn only_the_t2_engine_counts_as_dedicated_on_intel() {
    let id = |s: &str| format!("com.apple.videotoolbox.videoencoder.{s}");
    assert!(dedicated_on_mac(&id("ave.hevc"), "x86_64"));
    assert!(!dedicated_on_mac(&id("h264.gva"), "x86_64"));
    assert!(dedicated_on_mac(&id("h264"), "aarch64"));
    assert_eq!(engine_on_mac(&id("ave.hevc"), "x86_64"), "ave");
    assert_eq!(engine_on_mac(&id("h264.gva"), "x86_64"), "gva");
    assert_eq!(engine_on_mac(&id("hevc.gva"), "x86_64"), "gva");
    assert_eq!(engine_on_mac(&id("ave.avc"), "aarch64"), "media");
    assert_eq!(engine_on_mac(&id("ave.hevc"), "aarch64"), "media");
    let other = id("h264.other");
    assert_eq!(
        engine_on_mac(&other, "x86_64"),
        other,
        "an unknown id is its own engine"
    );
}

/// The probe's 5 s hold every number `probe::decide` reads, on every Mac and in any list order:
/// each encoder's 1080p60 and flat-out 1440p come first, then Slow motion and Sharp on the dedicated ones.
#[test]
fn the_probe_plan_measures_what_decide_needs_within_five_seconds_on_every_mac() {
    let budget = Duration::from_secs(5);
    let phase = |m| {
        [
            Measure::Hd60,
            Measure::QhdFlatOut,
            Measure::Hd120,
            Measure::Qhd60,
        ]
        .iter()
        .position(|&p| p == m)
    };
    for (mac, arch, dedicated) in [
        ("Apple silicon, H.264 and HEVC", "aarch64", vec![true, true]),
        (
            "a T2: its AVE and Quick Sync's two",
            "x86_64",
            vec![true, false, false],
        ),
        ("a T2 listed last", "x86_64", vec![false, false, true]),
        ("a plain Intel Mac", "x86_64", vec![false, false]),
    ] {
        let runs = plan(&dedicated, arch, budget);
        assert!(total(&runs) <= budget, "{mac}: {:?}", total(&runs));
        for (i, &own) in dedicated.iter().enumerate() {
            let has = |m| runs.iter().any(|r| r.encoder == i && r.measure == m);
            assert!(
                has(Measure::Hd60) && has(Measure::QhdFlatOut),
                "{mac}: encoder {i}"
            );
            assert_eq!(
                has(Measure::Hd120),
                own && arch == "aarch64",
                "{mac}: encoder {i}"
            );
            assert_eq!(has(Measure::Qhd60), own, "{mac}: encoder {i}");
        }
        let phases: Vec<_> = runs.iter().map(|r| phase(r.measure)).collect();
        assert!(
            phases.is_sorted(),
            "{mac}: by what decide needs, across encoders"
        );
        assert!(
            (runs.iter()).all(|r| r.window >= Duration::from_millis(250)),
            "{mac}: every window still measures"
        );
    }
    let roomy = plan(&[false, false], "x86_64", budget);
    assert_eq!(
        roomy[0].window,
        Duration::from_secs(1),
        "full windows when they fit"
    );
    let squeezed = plan(&[true, true], "aarch64", budget);
    assert!(
        squeezed[0].window < Duration::from_secs(1),
        "two dedicated engines shrink them"
    );
    // Four dedicated encoders: the floors don't all fit, so the least needed runs go, never a 1080p60 or a flat-out.
    let four = plan(&[true; 4], "aarch64", budget);
    assert!(total(&four) <= budget);
    for i in 0..4 {
        for m in [Measure::Hd60, Measure::QhdFlatOut] {
            assert!(
                four.iter().any(|r| r.encoder == i && r.measure == m),
                "{i} {m:?}"
            );
        }
    }
    assert!(
        infer_1440p60(66.0) && !infer_1440p60(63.0),
        "flat out well over 60 holds Sharp"
    );
}

/// The picture keeps the captured area's shape inside the preset's box, never above its own pixels: a
/// 16:10 Retina display at 1080p, a portrait display, a small window, the display's own size.
#[test]
fn the_picture_fits_the_captured_area_s_shape_inside_the_preset() {
    assert_eq!(fit_size((1920, 1080), (2880, 1800)), (1728, 1080));
    assert_eq!(fit_size((1920, 1080), (3840, 2160)), (1920, 1080));
    assert_eq!(
        fit_size((1920, 1080), (1080, 1920)),
        (1080, 1920),
        "turned for a portrait display"
    );
    assert_eq!(
        fit_size((1920, 1080), (801, 601)),
        (800, 600),
        "never larger, even sides"
    );
    assert_eq!(fit_size((2880, 1800), (2880, 1800)), (2880, 1800), "native");
    assert_eq!(frame_interval_parts(60), (10, 660), "1/66 s");
}

/// A still screen still gets a keyframe a second, and a still cursor is sampled four times a second (at least
/// carry: at least 2 Hz). Its re-feeds are `still`'s schedule (`tests/still_screen.rs`).
#[test]
fn a_still_screen_gets_keyframes_and_a_still_cursor_beats() {
    const { assert!(CURSOR_BEAT_NS <= 500_000_000, "at least 2 Hz") };
    assert!(!cursor_beat_due(1_000, 1_000 + CURSOR_BEAT_NS - 1));
    assert!(cursor_beat_due(1_000, 1_000 + CURSOR_BEAT_NS));
    assert!(!keyframe_due(1_000, 1_000 + KEYFRAME_EVERY_NS - 1));
    assert!(
        keyframe_due(1_000, 1_000 + KEYFRAME_EVERY_NS),
        "a keyframe a second of picture time"
    );
}

/// A budget under the plan's reserve measures nothing, and the plan ends (it once looped for ever).
#[test]
fn a_budget_under_the_reserve_plans_nothing_and_ends() {
    for budget in [0, 100, 299] {
        let budget = Duration::from_millis(budget);
        assert!(plan(&[true, false], "x86_64", budget).is_empty());
        assert!(plan(&[true, true], "aarch64", budget).is_empty());
    }
}

/// A whole-display take leaves out Klipolo's own processes and the notification apps: their windows (the
/// HUD, the camera bubble, the editor; every banner) never reach the file. Any other app stays in.
#[test]
fn a_whole_display_leaves_out_klipolo_and_the_notifications() {
    use klipolo_capture::platform::common::sck::{ListedApp, NOTIFICATION_APPS, left_out};
    let app = |pid, bundle_id| ListedApp { pid, bundle_id };
    let apps = [
        app(10, "com.apple.finder"),
        app(20, "com.klipolo.app"),
        app(30, "com.apple.notificationcenterui"),
        app(40, ""),
        app(50, "com.apple.UserNotificationCenter"),
        app(60, "com.klipolo.app"),
        app(70, "com.apple.Safari"),
    ];
    // The worker (40, no bundle id in a dev build), its parent (20) and another Klipolo (60); the system's alert panels
    // (50) stay in, so a tutorial can show a prompt being answered.
    assert_eq!(left_out(&apps, 40, 20, Some("com.klipolo.app")), [1, 2, 3, 5]);
    // A worker without a bundle id never matches every app that has none.
    assert_eq!(left_out(&apps, 99, 98, Some("")), [2]);
    assert_eq!(NOTIFICATION_APPS, ["com.apple.notificationcenterui"]);
}
