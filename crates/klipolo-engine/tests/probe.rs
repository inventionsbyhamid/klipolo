//! Imports probe in-process through the bundled libavformat: no ffprobe process, so the shipped app needs no
//! FFmpeg tool. The only test in this binary, so emptying `PATH` can't disturb another test.
mod support;

use klipolo_engine::render::probe::{MediaInfo, MediaKind, probe_media};

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn imports_probe_without_an_ffprobe_tool() {
    let empty = tempfile::tempdir().unwrap();
    // SAFETY: single-threaded here; no other test runs in this binary.
    unsafe { std::env::set_var("PATH", empty.path()) };

    let screen = probe_media(&support::fixture("bars-30fps-0.5gop.mp4")).unwrap();
    assert_eq!(screen.kind, MediaKind::Video);
    assert!((screen.duration - 4.0).abs() < 0.1, "{screen:?}");
    assert_eq!(
        (screen.width, screen.height, screen.fps),
        (1920, 1080, 30.0)
    );
    assert!(screen.has_audio);

    // Stored 1280×720 with a quarter-turn display matrix: shown portrait.
    let phone = probe_media(&support::fixture("phone-portrait.mov")).unwrap();
    assert_eq!(
        (phone.width, phone.height, phone.has_audio),
        (720, 1280, false)
    );
    // The turn is kept for the asset, so opening its decoder doesn't probe the file again.
    assert_eq!(phone.rotation % 180, 90);

    let tone = probe_media(&support::fixture("tone-440.m4a")).unwrap();
    assert_eq!(
        (tone.kind, tone.width, tone.fps, tone.has_audio),
        (MediaKind::Audio, 0, 0.0, true)
    );
    assert!((tone.duration - 4.0).abs() < 0.1, "{tone:?}");

    let png = empty.path().join("still.png");
    image::RgbaImage::new(40, 30).save(&png).unwrap();
    assert_eq!(
        probe_media(&png).unwrap(),
        MediaInfo {
            kind: MediaKind::Image,
            duration: 5.0,
            width: 40,
            height: 30,
            fps: 0.0,
            rotation: 0,
            has_audio: false,
            audio_lead_us: 0,
            created_ms: None,
            audio: None,
        }
    );
    std::fs::write(empty.path().join("notes.mp4"), b"not media").unwrap();
    assert!(probe_media(&empty.path().join("notes.mp4")).is_err());
}
