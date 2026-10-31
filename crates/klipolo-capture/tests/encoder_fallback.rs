//! A take never records without video while another encoder works: the probe's choice comes
//! first, then the encoders that hold its preset, and a pipe whose first choice won't start records on the next one,
//! with a mild note in the take's manifest. The camera's list follows the same rule, an encoder on the screen's engine
//! holding the camera only beside the screen (one physical engine).
#![cfg(feature = "synthetic")]

mod common;

use common::harness::Src;
use klipolo_capture::backend::{EncoderPick, EncoderResult, PresetId, ProbeReport};
use klipolo_capture::manifest::Manifest;
use klipolo_capture::media::Codec;
use klipolo_capture::probe::{CameraChoice, ScreenChoice, camera_encoders_for, encoders_for};
use klipolo_capture::run::threads::run_with;
use klipolo_capture::session::{self, SourceSpec};
use klipolo_capture::synthetic::{FakeBackend, Script, VideoFeed};

fn encoder(id: &str, codec: Codec, dedicated: bool, hd60: f32, qhd: f32) -> EncoderResult {
    EncoderResult {
        id: id.into(),
        codec,
        hardware: true,
        dedicated,
        engine: id.rsplit('.').next().unwrap().into(),
        fps_1080p60: hd60,
        fps_1440p60: 0.0,
        fps_1080p120: 0.0,
        max_fps_1440p: qhd,
        latency_p95_ms: 10.0,
    }
}

fn pick(id: &str, codec: Codec) -> EncoderPick {
    EncoderPick {
        codec,
        id: id.into(),
    }
}

/// A T2 Mac: the AVE's HEVC chosen for Smooth; Quick Sync's H.264 holds it, its HEVC only 1080p30, and an encoder the
/// probe saw refuse goes last but is kept.
#[test]
fn the_choice_comes_first_then_what_holds_its_preset_h264_first() {
    let report = ProbeReport {
        encoders: vec![
            encoder("hevc.ave", Codec::Hevc, true, 60.0, 70.0),
            encoder("old.hevc.gone", Codec::Hevc, false, 0.0, 0.0),
            encoder("hevc.gva", Codec::Hevc, false, 31.0, 14.0),
            encoder("h264.gva", Codec::H264, false, 60.0, 40.6),
        ],
        took_ms: 4_000,
    };
    let choice = ScreenChoice {
        preset: PresetId::Smooth,
        codec: Codec::Hevc,
        encoder: "hevc.ave".into(),
    };
    assert_eq!(
        encoders_for(&report, &choice, Some((2880, 1800))),
        [
            pick("hevc.ave", Codec::Hevc),
            pick("h264.gva", Codec::H264),
            pick("hevc.gva", Codec::Hevc),
            pick("old.hevc.gone", Codec::Hevc),
        ]
    );
}

/// The camera beside a T2's HEVC screen on the AVE: another engine first (Quick Sync's HEVC), then the screen's own
/// engine where it holds both (the AVE's HEVC), then one there that holds the camera alone but not beside the screen;
/// a camera-only take has no screen to share with, so its list is the screen's rule for the camera preset.
#[test]
fn the_camera_s_fallbacks_go_to_another_engine_first_and_share_one_only_if_it_holds_both() {
    let report = ProbeReport {
        encoders: vec![
            encoder("hevc.ave", Codec::Hevc, true, 60.0, 70.0),
            encoder("h264.ave", Codec::H264, true, 60.0, 20.0),
            encoder("old.hevc.gone", Codec::Hevc, false, 0.0, 0.0),
            encoder("hevc.gva", Codec::Hevc, false, 31.0, 14.0),
            encoder("h264.gva", Codec::H264, false, 60.0, 40.6),
        ],
        took_ms: 4_000,
    };
    let screen = ScreenChoice {
        preset: PresetId::Smooth,
        codec: Codec::Hevc,
        encoder: "hevc.ave".into(),
    };
    let camera = CameraChoice {
        preset: PresetId::Camera720,
        codec: Codec::H264,
        encoder: "h264.gva".into(),
    };
    let ids = |list: Vec<EncoderPick>| list.into_iter().map(|p| p.id).collect::<Vec<_>>();
    assert_eq!(
        ids(camera_encoders_for(&report, &camera, Some(&screen), None)),
        [
            "h264.gva",
            "hevc.gva",
            "hevc.ave",
            "h264.ave",
            "old.hevc.gone"
        ]
    );
    assert_eq!(
        ids(camera_encoders_for(&report, &camera, None, None)),
        [
            "h264.gva",
            "h264.ave",
            "hevc.ave",
            "hevc.gva",
            "old.hevc.gone"
        ],
        "a camera-only take"
    );
}

fn manifest(dir: &session::SessionDir) -> Manifest {
    serde_json::from_slice(&std::fs::read(dir.manifest()).unwrap()).unwrap()
}

#[test]
fn a_choice_that_wont_start_records_on_the_next_encoder_with_a_mild_note() {
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Screen, Src::Mic]);
    let mut cfg = session::read_config(&dir).unwrap();
    for s in &mut cfg.sources {
        if let SourceSpec::Screen(v) = &mut s.spec {
            v.encoders = vec![pick("hevc.ave", Codec::Hevc), pick("h264.gva", Codec::H264)];
        }
    }
    session::write_config(&dir, &cfg).unwrap();
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let backend = FakeBackend::new(Script {
        video: VideoFeed::Fixture(fixture, 30),
        refused_encoders: vec!["hevc.ave".into()],
        ..Script::default()
    });
    let script = common::harness::script_start_then_stop_after_ms(2_000);
    assert_eq!(run_with(Box::new(backend), dir.clone(), false, script), 0);
    let m = manifest(&dir);
    let screen = m.sources.iter().find(|s| s.id == "screen").unwrap();
    assert!(
        screen.runs.len() == 1 && screen.runs[0].duration_us > 500_000,
        "the screen was recorded: {screen:?}"
    );
    let fallback = (m.notes.iter()).filter(|n| n.code == "capture.note.encoderFallback");
    assert_eq!(fallback.count(), 1, "{:?}", m.notes);
}

#[test]
fn a_camera_whose_choice_wont_start_records_on_the_next_encoder() {
    let root = tempfile::tempdir().unwrap();
    let dir = common::harness::session(root.path(), &[Src::Camera, Src::Mic]);
    let mut cfg = session::read_config(&dir).unwrap();
    for s in &mut cfg.sources {
        if let SourceSpec::Camera { encoders, .. } = &mut s.spec {
            *encoders = vec![pick("h264.ave", Codec::H264), pick("h264.gva", Codec::H264)];
        }
    }
    session::write_config(&dir, &cfg).unwrap();
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4");
    let backend = FakeBackend::new(Script {
        video: VideoFeed::Fixture(fixture, 30),
        refused_encoders: vec!["h264.ave".into()],
        ..Script::default()
    });
    let script = common::harness::script_start_then_stop_after_ms(2_000);
    assert_eq!(run_with(Box::new(backend), dir.clone(), false, script), 0);
    let m = manifest(&dir);
    let camera = m.sources.iter().find(|s| s.id == "camera").unwrap();
    assert!(
        camera.runs.len() == 1 && camera.runs[0].duration_us > 500_000,
        "the camera was recorded: {camera:?}"
    );
    let fallback = (m.notes.iter()).filter(|n| n.code == "capture.note.encoderFallback");
    assert_eq!(fallback.count(), 1, "{:?}", m.notes);
}
