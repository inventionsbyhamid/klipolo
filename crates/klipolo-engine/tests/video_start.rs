//! Every decoder numbers a file's frames from its picture's start, as FFmpeg's decoders and the thumbnails
//! always have. A phone file whose picture starts after its sound (Android's MPEG4Writer writes an empty edit for the
//! later track) shows the same frame at the same time on a Mac's AVAssetReader as on FFmpeg, and the stored keyframe
//! index records where the picture starts. Files whose picture starts at 0 (most) are numbered as before.
mod support;

use std::path::PathBuf;

use klipolo_engine::media::keyframes::from_sample_table;
use klipolo_engine::vendor::cap::rendering::decoder::{DecoderTuning, DecoderType, spawn_decoder};

/// Sound from 0 and the picture from 70 ms: 2 s at 30 fps whose luma names each frame (16 + 8 × (frame mod 25)).
const LATE: &str = "video-starts-late.mp4";
/// Frames of the picture asked for, in a seeking order: on, back, far, back to the start.
const FRAMES: [u32; 8] = [2, 3, 1, 29, 0, 52, 15, 59];

fn luma_of(frame: u32) -> i32 {
    16 + 8 * (frame % 25) as i32
}

/// The decoder that read `path`, and the luma at the centre of each frame it shows half a frame into `FRAMES` (picture
/// time, 30 fps).
async fn shown(path: PathBuf, force_ffmpeg: bool) -> (DecoderType, Vec<i32>) {
    let decoder = spawn_decoder(
        "video-start",
        path,
        30.0,
        0.0,
        force_ffmpeg,
        DecoderTuning::default(),
    )
    .await
    .unwrap();
    let mut lumas = Vec::new();
    for n in FRAMES {
        let at = ((f64::from(n) + 0.5) / 30.0) as f32;
        let frame = decoder.get_frame(at).await.expect("a frame");
        let y = frame.y_plane().expect("a luma plane");
        lumas.push(i32::from(y[90 * frame.y_stride() as usize + 160]));
    }
    (decoder.decoder_type(), lumas)
}

/// Each shown luma is its frame's (within a few code values of the encoder's).
fn assert_frames(decoder: &str, lumas: &[i32]) {
    for (&n, &luma) in FRAMES.iter().zip(lumas) {
        assert!(
            (luma - luma_of(n)).abs() <= 3,
            "{decoder}: frame {n} of the picture shows luma {luma}, want {} (all: {lumas:?})",
            luma_of(n)
        );
    }
}

/// The stored keyframe index (an editing copy's, read from the sample table) and the editing copies' probe say the
/// picture starts 70 ms in, where FFmpeg's frame 0 is: FFmpeg shows the picture's frame n at n frames into the picture.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn ffmpeg_and_the_stored_index_start_the_picture_at_the_same_place() {
    let path = support::fixture(LATE);
    let keys = from_sample_table(&path).expect("no B-frames: the sample table is the index");
    let (num, den) = keys.time_base;
    let start_us = keys.start * i64::from(num) * 1_000_000 / i64::from(den);
    assert!(
        (start_us - 70_000).abs() <= 1_000,
        "starts at {start_us} µs"
    );
    let probed = klipolo_engine::media::proxy::facts::probe_facts(&path).unwrap();
    assert!(
        (probed.start_us - 70_000).abs() <= 1_000,
        "the copies' probe: {} µs",
        probed.start_us
    );
    assert_frames("FFmpeg", &shown(path, true).await.1);
}

/// On a Mac, AVAssetReader (the preview and the normal export) shows the frame FFmpeg shows at each time, for the file
/// as it is and for a copy that reads its index from beside it.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn avassetreader_shows_the_frame_ffmpeg_shows() {
    use klipolo_engine::media::keyframes::write_beside;
    use klipolo_engine::vendor::cap::video_decode::KeyframeIndex;

    let path = support::fixture(LATE);
    // The reader's index, built or stored, is on the picture's timeline: its first keyframe is frame 0 at 0 s.
    let built = KeyframeIndex::build(&path).unwrap();
    assert!(
        (built.start_secs() - 0.07).abs() < 0.001,
        "{}",
        built.start_secs()
    );
    assert_eq!(built.keyframes()[0], (0, 0.0));
    let stored = KeyframeIndex::from_stored(from_sample_table(&path).unwrap());
    assert_eq!(stored.keyframes(), built.keyframes());
    assert!((stored.start_secs() - built.start_secs()).abs() < 1e-9);

    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("late.mp4");
    std::fs::copy(&path, &copy).unwrap();
    write_beside(&copy, &from_sample_table(&copy).unwrap()).unwrap();
    assert_frames("FFmpeg", &shown(path.clone(), true).await.1);
    for (what, path) in [
        ("AVAssetReader", path),
        ("AVAssetReader, stored index", copy),
    ] {
        let (kind, lumas) = shown(path, false).await;
        assert_eq!(kind, DecoderType::AVAssetReader, "{what}");
        assert_frames(what, &lumas);
    }
}
