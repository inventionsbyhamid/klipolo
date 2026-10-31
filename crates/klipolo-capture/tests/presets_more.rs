//! The camera on the screen's encoder, launchers and a profile's
//! preset.

use klipolo_capture::backend::{EncoderResult, PresetId, ProbeReport};
use klipolo_capture::games::{AppFacts, looks_like_game};
use klipolo_capture::media::Codec;
use klipolo_capture::preset::{native_preset, preset};
use klipolo_capture::probe::{Choices, decide};
use klipolo_capture::profile::GameProfile;

fn enc(id: &str, codec: Codec, dedicated: bool, p1080: f32, flat_1440: f32) -> EncoderResult {
    EncoderResult {
        id: id.into(),
        codec,
        hardware: true,
        dedicated,
        engine: id.into(),
        fps_1080p60: p1080,
        fps_1440p60: flat_1440.min(60.0),
        fps_1080p120: 0.0,
        max_fps_1440p: flat_1440,
        latency_p95_ms: 20.0,
    }
}

fn report(encoders: Vec<EncoderResult>) -> ProbeReport {
    ProbeReport {
        encoders,
        took_ms: 3_000,
    }
}

/// What the probe measured `e` encoding per second, in pixels: a paced 1080p60 run that fell short of 60 found its
/// limit; one that held 60 only shows the pacing, so its flat-out 1440p run says how far past it goes.
fn capacity(e: &EncoderResult) -> f64 {
    let paced = f64::from(e.fps_1080p60) * 1920.0 * 1080.0;
    if e.fps_1080p60 < 59.0 {
        paced
    } else {
        paced.max(f64::from(e.max_fps_1440p) * 2560.0 * 1440.0)
    }
}

/// Every choice that shares the camera's encoder fits beside it, with the native rule's 5 % margin.
fn never_overloaded(c: &Choices, e: &EncoderResult, display: (u32, u32)) {
    let Some(camera) = &c.camera else { return };
    let cam = preset(camera.preset, Codec::H264);
    let rate = |w: u32, h: u32, fps: u32| f64::from(w) * f64::from(h) * f64::from(fps);
    for choice in [&c.screen, &c.native].into_iter().flatten() {
        if choice.encoder == camera.encoder && choice.encoder == e.id {
            let s = native_preset(choice.preset, choice.codec, display);
            let load = rate(s.width, s.height, s.fps) + rate(cam.width, cam.height, cam.fps);
            assert!(
                capacity(e) >= 1.05 * load,
                "{:?} and {:?} overload {}",
                choice.preset,
                camera.preset,
                e.id
            );
        }
    }
}

#[test]
fn a_camera_on_the_screens_encoder_never_overloads_it() {
    // An iGPU that held 35 fps at 1080p: Light (30) plus Camera720 (13.3 in 1080p frames) is 43 a second. Neither can go
    // lower, so the camera goes, and preflight says why.
    let igpu = enc("h264.igpu", Codec::H264, false, 35.0, 18.0);
    let c = decide(&report(vec![igpu.clone()]), 60, None, true);
    assert_eq!(c.screen.as_ref().unwrap().preset, PresetId::Light);
    assert!(c.camera.is_none(), "{:?}", c.camera);
    assert_eq!(
        c.camera_note.as_ref().map(|n| n.code.as_str()),
        Some("capture.preflight.cameraWithScreen")
    );
    never_overloaded(&c, &igpu, (1920, 1080));

    // A dedicated engine that runs 1440p at 40 flat out: Smooth with a 1080p camera, then with a 720p one, is too much;
    // the camera steps down first, then the screen. The 1280×800 display's own 60 fps still fits beside the camera.
    let engine = enc("h264.engine", Codec::H264, true, 60.0, 40.0);
    let c = decide(&report(vec![engine.clone()]), 60, Some((1280, 800)), true);
    let (s, n, cam) = (
        c.screen.clone().unwrap(),
        c.native.clone().unwrap(),
        c.camera.clone().unwrap(),
    );
    assert_eq!(
        (s.preset, n.preset, cam.preset, cam.encoder.as_str()),
        (
            PresetId::Light,
            PresetId::Native60,
            PresetId::Camera720,
            "h264.engine"
        )
    );
    assert!(
        c.available.contains(&PresetId::Smooth),
        "without the camera, Smooth still fits"
    );
    never_overloaded(&c, &engine, (1280, 800));
}

#[test]
fn a_take_without_the_camera_keeps_what_the_screen_alone_holds() {
    // The same engine, the camera off: the gamer without a facecam records Smooth, not Light, and
    // nothing is said about a camera.
    let engine = enc("h264.engine", Codec::H264, true, 60.0, 40.0);
    let c = decide(&report(vec![engine.clone()]), 60, Some((1280, 800)), false);
    assert_eq!(c.screen.as_ref().unwrap().preset, PresetId::Smooth);
    assert_eq!(c.native.as_ref().unwrap().preset, PresetId::Native60);
    assert!(c.camera.is_none() && c.camera_note.is_none());
    // The 35 fps iGPU keeps Light, and with no camera there is no note about one.
    let igpu = enc("h264.igpu", Codec::H264, false, 35.0, 18.0);
    let c = decide(&report(vec![igpu]), 60, None, false);
    assert_eq!(c.screen.as_ref().unwrap().preset, PresetId::Light);
    assert!(c.camera.is_none() && c.camera_note.is_none());
}

#[test]
fn a_roomy_shared_encoder_keeps_both_and_an_hevc_only_computer_still_gets_a_camera() {
    // Apple silicon's one H.264 engine: Smooth and a 1080p camera fit side by side, unchanged.
    let apple = enc("h264.apple", Codec::H264, true, 60.0, 120.0);
    let c = decide(&report(vec![apple.clone()]), 60, None, true);
    assert_eq!(c.screen.as_ref().unwrap().preset, PresetId::Smooth);
    let cam = c.camera.clone().unwrap();
    assert_eq!(
        (cam.preset, cam.encoder.as_str()),
        (PresetId::Camera1080, "h264.apple")
    );
    never_overloaded(&c, &apple, (1920, 1080));

    // An HEVC-only computer: the camera shares the screen's encoder, in HEVC.
    let hevc = enc("hevc.only", Codec::Hevc, true, 60.0, 90.0);
    let c = decide(&report(vec![hevc]), 60, None, true);
    let cam = c.camera.clone().expect("a camera on the screen's encoder");
    assert_eq!(
        (cam.preset, cam.codec, cam.encoder.as_str()),
        (PresetId::Camera1080, Codec::Hevc, "hevc.only")
    );
    assert!(c.camera_note.is_none());
}

#[test]
fn launchers_and_the_tools_steam_ships_are_not_games() {
    let exe = |path: &str| AppFacts {
        app_id: "x".into(),
        exe: Some(path.into()),
        ..AppFacts::default()
    };
    let desktop = |id: &str, categories: &[&str]| AppFacts {
        app_id: id.into(),
        desktop_categories: categories.iter().map(|c| c.to_string()).collect(),
        ..AppFacts::default()
    };
    let mac = |id: &str, category: &str| AppFacts {
        app_id: id.into(),
        category: Some(category.into()),
        ..AppFacts::default()
    };
    let table = [
        (
            exe(r"C:\Program Files (x86)\Steam\steamapps\common\Hades\Hades.exe"),
            true,
        ),
        (
            exe("/home/sam/.local/share/Steam/steamapps/common/Celeste/Celeste"),
            true,
        ),
        (
            exe(
                r"C:\Program Files (x86)\GOG Galaxy\Games\Cyberpunk 2077\bin\x64\Cyberpunk2077.exe",
            ),
            true,
        ),
        (
            exe(
                r"C:\Program Files\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe",
            ),
            true,
        ),
        (desktop("org.supertuxkart", &["Game", "ArcadeGame"]), true),
        (
            exe(r"C:\Program Files (x86)\GOG Galaxy\GalaxyClient.exe"),
            false,
        ),
        (
            exe(r"C:\Program Files\Epic Games\UE_5.4\Engine\Binaries\Win64\UnrealEditor.exe"),
            false,
        ),
        (
            exe(r"C:\Program Files\Epic Games\DirectXRedist\DXSETUP.exe"),
            false,
        ),
        (
            exe(
                r"C:\Program Files (x86)\Epic Games\Launcher\Portal\Binaries\Win64\EpicGamesLauncher.exe",
            ),
            false,
        ),
        (exe(r"C:\Program Files (x86)\Steam\steam.exe"), false),
        (
            exe(r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\wallpaper64.exe"),
            false,
        ),
        (
            exe(r"C:\Program Files (x86)\Steam\steamapps\common\SteamVR\bin\win64\vrmonitor.exe"),
            false,
        ),
        (
            exe(r"C:\Program Files (x86)\Steam\steamapps\common\OBS Studio\bin\64bit\obs64.exe"),
            false,
        ),
        (
            exe(r"C:\Program Files (x86)\Steam\steamapps\common\Blender\blender.exe"),
            false,
        ),
        (
            exe("/home/sam/.local/share/Steam/steamapps/common/Proton 9.0 (Beta)/files/bin/wine64"),
            false,
        ),
        (
            exe("/home/sam/.local/share/Steam/steamapps/common/SteamLinuxRuntime_sniper/pv-adverb"),
            false,
        ),
        (
            desktop("steam", &["Network", "FileTransfer", "Game"]),
            false,
        ),
        (desktop("net.lutris.Lutris", &["Game"]), false),
        (desktop("com.heroicgameslauncher.hgl", &["Game"]), false),
        (
            mac("com.valvesoftware.steam", "public.app-category.games"),
            false,
        ),
    ];
    for (facts, game) in &table {
        assert_eq!(looks_like_game(facts), *game, "{facts:?}");
    }
}

#[test]
fn a_new_game_profile_records_at_what_suits_this_computer() {
    // None is "Best for this computer": the probe's choice, never a fixed Smooth an older Mac can't hold.
    let json = serde_json::to_value(GameProfile::default()).unwrap();
    assert_eq!(json["preset"], serde_json::Value::Null);
}
