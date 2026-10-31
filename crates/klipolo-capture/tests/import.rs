#![cfg(feature = "synthetic")]
mod common;
use common::fake::{FakeCounting, Source, manifest_with, probe_ok};
use klipolo_capture::backend::{DeviceInfo, Transport};
use klipolo_capture::import::{clip_for, manifest_peaks, recording_from};
use klipolo_capture::preflight::setup;
use klipolo_model::project::SourceRole;

#[test]
fn a_finished_take_becomes_a_format_4_recording_with_one_asset_per_run() {
    let (m, dir) = manifest_with(&[
        ("screen", SourceRole::Screen, &[(0, 60.0)]),
        ("mic", SourceRole::Mic, &[(0, 60.0)]),
    ]);
    let (imported, notes) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
    assert!(notes.is_empty());
    let r = &imported.recording;
    assert_eq!(
        (r.primary.as_str(), r.sources.len(), imported.assets.len()),
        ("screen", 2, 2)
    );
    assert_eq!(r.duration_us(), 60_000_000);
    let clip = clip_for(r, true);
    assert_eq!(
        (clip.source_in_us, clip.source_out_us, clip.camera.is_some()),
        (0, 60_000_000, false)
    );
}

#[test]
fn a_camera_that_reconnected_imports_as_two_runs_of_one_source() {
    let (m, dir) = manifest_with(&[
        ("screen", SourceRole::Screen, &[(0, 60.0)]),
        (
            "camera",
            SourceRole::Camera,
            &[(33_000, 9.9), (12_000_000, 48.0)],
        ),
    ]);
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
    let cam = imported.recording.camera().unwrap();
    assert_eq!(
        cam.runs.iter().map(|r| r.offset_us).collect::<Vec<_>>(),
        [33_000, 12_000_000]
    );
    // The camera's layer is mirrored as the take's "Mirror my camera" was.
    let mirror = |on| clip_for(&imported.recording, on).camera.map(|c| c.mirror);
    assert_eq!((mirror(true), mirror(false)), (Some(true), Some(false)));
}

/// When the camera is the main footage (a Camera only take, or a screen that didn't start), the
/// clip still has its camera layout, so the footage is mirrored as the switch said.
#[test]
fn a_camera_only_take_s_footage_is_mirrored_as_the_switch_said() {
    let screen_failed: &[Source] = &[
        ("screen", SourceRole::Screen, &[]),
        ("camera", SourceRole::Camera, &[(0, 30.0)]),
    ];
    let camera_only: &[Source] = &[("camera", SourceRole::Camera, &[(0, 30.0)])];
    for sources in [screen_failed, camera_only] {
        let (m, dir) = manifest_with(sources);
        let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
        let mirror = |on| clip_for(&imported.recording, on).camera.map(|c| c.mirror);
        assert_eq!((mirror(true), mirror(false)), (Some(true), Some(false)));
    }
}

#[test]
fn a_take_whose_screen_failed_makes_the_camera_the_main_footage() {
    let (m, dir) = manifest_with(&[
        ("screen", SourceRole::Screen, &[]),
        ("camera", SourceRole::Camera, &[(0, 30.0)]),
    ]);
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap();
    assert_eq!(
        imported.recording.primary_source().unwrap().role,
        SourceRole::Camera
    );
}

#[test]
fn a_take_with_no_picture_is_refused_and_kept() {
    let (m, dir) = manifest_with(&[("mic", SourceRole::Mic, &[(0, 30.0)])]);
    let err = recording_from(&m, &dir, &manifest_peaks, &mut probe_ok()).unwrap_err();
    assert_eq!(err.code, "capture.import.noVideo");
    assert!(dir.path("mic/run-01.caf").exists());
    // A picture whose file is there but can't be read is said to be kept.
    let (m, dir) = manifest_with(&[
        ("screen", SourceRole::Screen, &[(0, 30.0)]),
        ("mic", SourceRole::Mic, &[(0, 30.0)]),
    ]);
    let mut ok = probe_ok();
    let mut probe = |path: &std::path::Path, name: &str| match name {
        "screen/run-01.mp4" => Err("moov atom not found".to_string()),
        _ => ok(path, name),
    };
    let err = recording_from(&m, &dir, &manifest_peaks, &mut probe).unwrap_err();
    assert_eq!(err.code, "capture.import.videoUnreadable");
    assert!(dir.path("screen/run-01.mp4").exists());
}

#[test]
fn preflight_blocks_below_two_gb_warns_below_thirty_minutes_and_never_captures() {
    let backend = FakeCounting::new(vec![DeviceInfo {
        id: "BlackHole".into(),
        name: "BlackHole 2ch".into(),
        transport: Transport::Virtual,
        is_virtual: true,
        in_use: false,
        suspended: false,
    }]);
    let s = setup(
        &backend,
        Some(&common::fake::t2_report()),
        Some(1_500_000_000),
        Some(40),
        None,
        false,
    );
    assert!(
        s.notes
            .iter()
            .any(|n| n.code == "capture.preflight.diskBlock")
    );
    assert!(
        s.notes
            .iter()
            .any(|n| n.code == "capture.preflight.virtualMic")
    );
    let s = setup(
        &backend,
        Some(&common::fake::t2_report()),
        Some(8_000_000_000),
        Some(900),
        None,
        false,
    );
    assert!(
        s.notes
            .iter()
            .any(|n| n.code == "capture.preflight.diskShort"),
        "Smooth HEVC with camera and sound: about 27 min above the floor"
    );
    assert!(
        s.notes
            .iter()
            .any(|n| n.code == "capture.preflight.slowDrive")
    );
    assert!(
        s.offers
            .iter()
            .any(|o| o.best_game && o.minutes_left.is_some())
    );
    assert!(
        s.offers.iter().any(|o| o.best_native),
        "a native default for tutorials too"
    );
    assert_eq!(
        backend.capture_calls(),
        0,
        "video, audio and preview are never called"
    );
}
