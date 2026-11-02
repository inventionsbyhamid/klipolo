//! "Record yourself": a Camera take of the camera and the mic alone, with no screen. It records both, each its own file,
//! with the camera as the main picture; its sound lands on the camera's frames on the take's clock; it imports as camera
//! footage (the camera the main picture, mirrored as the switch said); and a crashed worker's take resumes and finishes
//! as any take does.
#![cfg(feature = "synthetic")]

mod common;

use std::path::Path;

use common::harness::{Harness, Src, cfg};
use klipolo_capture::import::{clip_for, manifest_peaks, recording_from};
use klipolo_capture::manifest::{Manifest, read_manifest};
use klipolo_capture::protocol::Command;
use klipolo_capture::run::threads::run_with;
use klipolo_capture::session::{self, SessionConfig, SessionDir};
use klipolo_model::ids::new_id;
use klipolo_model::project::{Asset, AssetKind, FormatKind, RecordingKind, SourceRole};

/// A Camera take's config: the camera and the mic, with this countdown.
fn camera_take(countdown_s: u32) -> SessionConfig {
    let mut cfg = cfg(&[Src::Camera, Src::Mic], countdown_s);
    cfg.kind = RecordingKind::Camera;
    cfg
}

/// The app's probe as a real take's files answer it: each run's file as long as the manifest measured it, a 1080p30
/// video for `.mp4` and sound for `.caf`.
fn probe(m: &Manifest) -> impl FnMut(&Path, &str) -> Result<Asset, String> + '_ {
    move |path: &Path, name: &str| {
        let run = (m.sources.iter().flat_map(|s| &s.runs))
            .find(|r| r.file == name)
            .ok_or("not a run")?;
        let video = name.ends_with(".mp4");
        Ok(Asset {
            id: new_id(),
            kind: if video {
                AssetKind::Video
            } else {
                AssetKind::Audio
            },
            path: path.to_string_lossy().into_owned(),
            name: name.into(),
            duration_us: run.duration_us,
            width: if video { 1920 } else { 0 },
            height: if video { 1080 } else { 0 },
            fps: if video { 30.0 } else { 0.0 },
            rotation: None,
            has_audio: !video,
            audio_lead_us: None,
            made_with: None,
        })
    }
}

fn sources(m: &Manifest) -> Vec<(&str, SourceRole, usize)> {
    (m.sources.iter())
        .map(|s| (s.id.as_str(), s.role, s.runs.len()))
        .collect()
}

#[test]
fn a_camera_take_keeps_its_camera_and_mic_in_sync() {
    // The camera's first frame comes 0.6 s after the mic's first sound and the mic's clock runs 80 ppm fast: the take
    // starts once the camera sends, and over a minute the mic's blips still land on the camera's flash frames.
    let mut h = Harness::recording_cfg_with_drift(camera_take(3), 80.0, &[("camera", 0.6)]);
    h.feed_for(60.0);
    h.command(Command::Stop);
    let m = h.manifest();
    assert_eq!(m.kind, RecordingKind::Camera);
    assert_eq!(
        sources(&m),
        [("camera", SourceRole::Camera, 1), ("mic", SourceRole::Mic, 1)]
    );
    assert!(m.cursor.is_none(), "no screen, no cursor");
    let worst_ms = h.sync_error_ms("camera", "mic");
    assert!(worst_ms <= 10.0, "{worst_ms} ms");
}

#[test]
fn a_finished_camera_take_imports_as_camera_footage() {
    let root = tempfile::tempdir().unwrap();
    let dir = SessionDir::create(root.path(), "take").unwrap();
    let mut cfg = camera_take(0);
    cfg.mirror_camera = false;
    session::write_config(&dir, &cfg).unwrap();
    let code = run_with(
        common::harness::fake_backend(),
        dir.clone(),
        false,
        vec![(0, Command::Start), (900, Command::Marker), (2_500, Command::Stop)],
    );
    assert_eq!(code, 0);
    let m = read_manifest(&dir).unwrap();
    assert_eq!(m.kind, RecordingKind::Camera);
    assert_eq!(
        sources(&m),
        [("camera", SourceRole::Camera, 1), ("mic", SourceRole::Mic, 1)]
    );
    assert_eq!(m.markers.len(), 1, "Mark this moment");
    let camera = &m.sources[0].runs[0];
    let frames = common::decode::decode_all(&dir.path(&camera.file)).0;
    assert!(frames >= 30, "{frames} frames of the camera decode");

    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe(&m)).unwrap();
    let r = &imported.recording;
    assert_eq!(r.kind, RecordingKind::Camera);
    assert!(r.camera_only(), "the camera is the main footage");
    assert_eq!(r.primary, "camera");
    assert!(r.camera().is_none(), "no bubble");
    assert_eq!(r.markers.len(), 1);
    let clip = clip_for(r, cfg.mirror_camera, FormatKind::Landscape);
    assert_eq!(clip.camera.map(|c| c.mirror), Some(false));
    assert_eq!((clip.source_in_us, clip.source_out_us), (0, r.duration_us()));
}

#[test]
fn a_crashed_camera_take_resumes_and_finishes() {
    let root = tempfile::tempdir().unwrap();
    let crashed = common::take::Take {
        finalize: false,
        cut_last_fragment: true,
        camera: true,
        ..common::take::Take::default()
    };
    let t = common::take::write_take(root.path(), "take", crashed);
    let code = run_with(
        common::harness::fake_backend(),
        t.dir.clone(),
        true,
        vec![(1_500, Command::Stop)],
    );
    assert_eq!(code, 0);
    let m = read_manifest(&t.dir).unwrap();
    assert_eq!(m.kind, RecordingKind::Camera);
    assert_eq!(
        sources(&m),
        [("camera", SourceRole::Camera, 2), ("mic", SourceRole::Mic, 2)],
        "the crashed runs, then the resumed ones"
    );
    for run in &m.sources[0].runs {
        let frames = common::decode::decode_all(&t.dir.path(&run.file)).0 as i64;
        assert!(
            (frames - run.duration_us * 30 / 1_000_000).abs() <= 1,
            "{}: {frames} frames",
            run.file
        );
    }
    let (imported, _) = recording_from(&m, &t.dir, &manifest_peaks, &mut probe(&m)).unwrap();
    assert!(imported.recording.camera_only());
    assert_eq!(imported.recording.sources[0].runs.len(), 2);
}
