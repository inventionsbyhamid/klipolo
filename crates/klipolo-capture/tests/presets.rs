use klipolo_capture::backend::{EncoderResult, PresetId, ProbeReport};
use klipolo_capture::games::{AppFacts, looks_like_game};
use klipolo_capture::media::Codec;
use klipolo_capture::preset::{bytes_per_s, preset};
use klipolo_capture::probe::{GpuVendor, decide, ladder};
use klipolo_capture::profile::{GameProfile, Profiles};
use klipolo_platform::caps::OsFamily;

/// `p1440` is both the paced 1440p60 result and, for these fixtures, the flat-out 1440p rate unless set apart.
fn enc(id: &str, codec: Codec, dedicated: bool, p1080: f32, p1440: f32) -> EncoderResult {
    EncoderResult {
        id: id.into(),
        codec,
        hardware: true,
        dedicated,
        engine: id.into(),
        fps_1080p60: p1080,
        fps_1440p60: p1440.min(60.0),
        fps_1080p120: 0.0,
        max_fps_1440p: p1440,
        latency_p95_ms: 20.0,
    }
}

#[test]
fn the_owners_intel_t2_mac_defaults_to_hevc_on_the_t2_at_1080p60() {
    // Measured: gva H.264 held 60 idle, but it shares the game's GPU; the T2's AVE is separate.
    let report = ProbeReport {
        encoders: vec![
            enc("h264.gva", Codec::H264, false, 60.0, 57.0),
            enc("hevc.ave", Codec::Hevc, true, 60.0, 88.0),
            enc("hevc.gva", Codec::Hevc, false, 22.0, 16.0),
        ],
        took_ms: 4_000,
    };
    let c = decide(&report, 60, Some((2560, 1600)), true);
    let s = c.screen.unwrap();
    assert_eq!(
        (s.preset, s.codec, s.encoder.as_str()),
        (PresetId::Smooth, Codec::Hevc, "hevc.ave")
    );
    // Tutorials: the 13-inch Retina's own 2560×1600 at 60 fps on the T2 (it ran 1440p at 88 fps flat out).
    let n = c.native.unwrap();
    assert_eq!((n.preset, n.codec), (PresetId::Native60, Codec::Hevc));
    assert_eq!(
        c.camera.unwrap().encoder,
        "h264.gva",
        "the camera stays off the screen's engine"
    );
    assert!(c.available.contains(&PresetId::Light) && !c.available.contains(&PresetId::SlowMo));
}

#[test]
fn an_intel_mac_without_a_t2_gets_1080p30_h264() {
    let c = decide(
        &ProbeReport {
            encoders: vec![enc("h264.gva", Codec::H264, false, 48.0, 41.0)],
            took_ms: 3_000,
        },
        60,
        Some((1440, 900)),
        true,
    );
    let s = c.screen.unwrap();
    assert_eq!((s.preset, s.codec), (PresetId::Light, Codec::H264));
}

#[test]
fn apple_silicon_defaults_to_h264_1080p60_with_sharp_and_slow_motion_on_promotion() {
    let mut h264 = enc("h264.apple", Codec::H264, true, 60.0, 120.0);
    h264.fps_1080p120 = 120.0;
    let c = decide(
        &ProbeReport {
            encoders: vec![h264, enc("hevc.apple", Codec::Hevc, true, 60.0, 120.0)],
            took_ms: 2_000,
        },
        120,
        Some((3024, 1964)),
        true,
    );
    assert_eq!(c.screen.as_ref().unwrap().preset, PresetId::Smooth);
    assert_eq!(c.screen.unwrap().codec, Codec::H264, "games: H.264");
    assert!(c.available.contains(&PresetId::Sharp) && c.available.contains(&PresetId::SlowMo));
    // Tutorials: the display's own 3024×1964 at 60 on the media engine's HEVC ("HEVC on … Apple Silicon").
    let n = c.native.unwrap();
    assert_eq!((n.preset, n.codec), (PresetId::Native60, Codec::Hevc));
}

#[test]
fn no_hardware_encoder_means_no_video_recording() {
    let none = decide(
        &ProbeReport {
            encoders: vec![],
            took_ms: 100,
        },
        60,
        Some((1920, 1080)),
        true,
    );
    assert!(none.screen.is_none() && none.native.is_none());
}

#[test]
fn ladders_try_the_vendor_s_encoder_first_and_never_x264() {
    assert_eq!(
        ladder(OsFamily::Windows, GpuVendor::Nvidia, Codec::H264)[0],
        "h264_nvenc"
    );
    assert_eq!(
        ladder(OsFamily::Windows, GpuVendor::Amd, Codec::H264)[0],
        "h264_amf"
    );
    assert_eq!(
        ladder(OsFamily::Windows, GpuVendor::Intel, Codec::H264)[0],
        "h264_qsv"
    );
    assert_eq!(
        ladder(OsFamily::Linux, GpuVendor::Intel, Codec::H264)[0],
        "h264_vaapi"
    );
    for os in [OsFamily::Windows, OsFamily::Linux] {
        for v in [
            GpuVendor::Nvidia,
            GpuVendor::Amd,
            GpuVendor::Intel,
            GpuVendor::Other,
        ] {
            assert!(
                !ladder(os, v, Codec::H264)
                    .iter()
                    .any(|e| e.contains("x264")),
                "LGPL: no x264"
            );
        }
    }
}

#[test]
fn smooth_with_a_camera_and_two_sound_sources_is_about_17_gb_an_hour() {
    let gb_h = bytes_per_s(
        Some(preset(PresetId::Smooth, Codec::H264)),
        Some(preset(PresetId::Camera720, Codec::H264)),
        1,
        1,
    ) * 3_600.0
        / 1e9;
    assert!((16.0..18.0).contains(&gb_h), "{gb_h}");
}

#[test]
fn profiles_round_trip_keep_200_and_survive_a_bad_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    std::fs::write(&path, b"{ not json").unwrap();
    let mut p = Profiles::load(&path);
    assert!(p.games.is_empty());
    for i in 0..205u64 {
        p.put(GameProfile {
            app_id: format!("game.{i}"),
            updated_ms: i,
            ..GameProfile::default()
        });
    }
    p.save(&path).unwrap();
    let back = Profiles::load(&path);
    assert_eq!(back.games.len(), 200);
    assert!(back.get("game.0").is_none() && back.get("game.204").is_some());
}

#[test]
fn games_are_recognised_but_only_suggested() {
    let mac = AppFacts {
        app_id: "com.studio.game".into(),
        category: Some("public.app-category.action-games".into()),
        ..AppFacts::default()
    };
    let steam = AppFacts {
        app_id: "x".into(),
        exe: Some(r"C:\Program Files (x86)\Steam\steamapps\common\Hades\Hades.exe".into()),
        ..AppFacts::default()
    };
    let linux = AppFacts {
        app_id: "org.supertuxkart".into(),
        desktop_categories: vec!["Game".into(), "ArcadeGame".into()],
        ..AppFacts::default()
    };
    let notes = AppFacts {
        app_id: "com.apple.Notes".into(),
        category: Some("public.app-category.productivity".into()),
        ..AppFacts::default()
    };
    assert!(
        looks_like_game(&mac)
            && looks_like_game(&steam)
            && looks_like_game(&linux)
            && !looks_like_game(&notes)
    );
}

/// "Shared encoder" means one physical engine: Apple silicon's H.264 and HEVC ids are one media
/// engine, so the H.264 camera beside the HEVC native recording is added up, and steps down to fit.
#[test]
fn an_h264_camera_beside_an_hevc_native_recording_on_one_engine_is_added_up() {
    // 1440p at 110 fps flat out: 405 Mpx/s. The 3024×1964 display at 60 (356 Mpx/s) fits alone, and beside a 720p
    // camera (384 Mpx/s with the 5 % margin: 403), but not beside a 1080p one (440).
    let on_media = |id: &str, codec| EncoderResult {
        engine: "media".into(),
        ..enc(id, codec, true, 60.0, 110.0)
    };
    let report = ProbeReport {
        encoders: vec![
            on_media("ave.avc", Codec::H264),
            on_media("ave.hevc", Codec::Hevc),
        ],
        took_ms: 3_000,
    };
    let c = decide(&report, 120, Some((3024, 1964)), true);
    let (native, camera) = (c.native.unwrap(), c.camera.unwrap());
    assert_eq!(
        (native.preset, native.codec, native.encoder.as_str()),
        (PresetId::Native60, Codec::Hevc, "ave.hevc")
    );
    assert_eq!(
        (camera.preset, camera.encoder.as_str()),
        (PresetId::Camera720, "ave.avc"),
        "the camera on the media engine's H.264 id, stepped down beside the HEVC native recording"
    );
}
