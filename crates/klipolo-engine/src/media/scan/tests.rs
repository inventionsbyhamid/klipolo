//! What a scan costs where the store can't see it: decodes per slot and bytes read.

use std::sync::Arc;

use klipolo_platform::durable::Os;

use super::{ScanEnd, scan_thumbs};
use crate::media::key::{CacheKind, stem};
use crate::media::store::ThumbStore;
use crate::media::thumbs::{poster, strip};

fn fixture(name: &str) -> String {
    let path = crate::fixture_set::fixture_dir().join(name);
    assert!(
        path.exists(),
        "{name} is missing: run scripts/gen-engine-fixtures.sh"
    );
    path.to_string_lossy().into_owned()
}

#[test]
fn an_all_intra_file_is_cut_with_one_seek_and_one_decode_per_slot() {
    let root = tempfile::tempdir().unwrap();
    // 6 s of MJPEG at 30 fps: 180 frames, every one a keyframe.
    let video = fixture("intra-6s-mjpeg.mov");
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    let scanned = scan_thumbs(&store.shared(), &video, &[0], &|| true);
    assert_eq!(scanned.end, ScanEnd::Done);
    assert!(
        scanned.decoded <= 6,
        "{} decodes for 6 slots",
        scanned.decoded
    );
    let size = std::fs::metadata(&video).unwrap().len();
    assert!(
        scanned.bytes_read * 4 < size,
        "read {} of {size} bytes",
        scanned.bytes_read
    );
    assert_eq!(strip(root.path(), &video).unwrap().chunks.len(), 1);
    // The index comes from the demuxer's sample table, and it is the one a demux of every packet builds.
    let key = stem(&video, &CacheKind::Thumbs).unwrap();
    let stored = crate::media::keyframes::read(root.path(), &key).expect("a stored index");
    assert_eq!(stored.pts.len(), 180);
    #[cfg(target_os = "macos")]
    {
        use crate::vendor::cap::video_decode::KeyframeIndex;
        let built = KeyframeIndex::build(std::path::Path::new(&video)).unwrap();
        assert_eq!(
            KeyframeIndex::from_stored(stored).keyframes(),
            built.keyframes()
        );
    }
}

#[test]
fn a_short_gop_decodes_one_keyframe_per_slot() {
    let root = tempfile::tempdir().unwrap();
    // 4 s with a keyframe every 0.5 s: 8 keyframes, 4 slots.
    let video = fixture("bars-30fps-0.5gop.mp4");
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    let scanned = scan_thumbs(&store.shared(), &video, &[0], &|| true);
    assert_eq!(scanned.end, ScanEnd::Done);
    assert_eq!(scanned.decoded, 4);
    assert_eq!(strip(root.path(), &video).unwrap().chunks.len(), 1);
}

/// A Matroska file's sample table holds only what was read when it opened (Cues are read at the first seek): stored as
/// the keyframe index, it would leave decoders most keyframes short. Only a table that covers every frame of the stream is stored; otherwise
/// decoders build their own index, and no scan tries again.
#[test]
fn a_partial_sample_table_is_not_stored_as_the_keyframe_index() {
    let root = tempfile::tempdir().unwrap();
    // `intra-6s-mjpeg.mov` remuxed into Matroska (180 frames; its table at open holds the packets read so far).
    let video = fixture("intra-6s-mjpeg.mkv");
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    let scanned = scan_thumbs(&store.shared(), &video, &[0], &|| true);
    assert_eq!(scanned.end, ScanEnd::Done);
    let key = stem(&video, &CacheKind::Thumbs).unwrap();
    let stored = crate::media::keyframes::read(root.path(), &key).map(|k| k.pts.len());
    assert!(
        stored.is_none_or(|n| n == 180),
        "{stored:?} keyframes stored"
    );
    assert!(crate::media::keyframes::has(root.path(), &key));
}

/// The index a scan stores says where the picture starts (a phone file whose picture starts 70 ms after
/// its sound), so a decoder that reads it numbers frames from there, as FFmpeg's decoders do.
#[test]
fn a_scan_stores_where_the_picture_starts() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("video-starts-late.mp4");
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    let scanned = scan_thumbs(&store.shared(), &video, &[0], &|| true);
    assert_eq!(scanned.end, ScanEnd::Done);
    let key = stem(&video, &CacheKind::Thumbs).unwrap();
    let stored = crate::media::keyframes::read(root.path(), &key).expect("a stored index");
    let (num, den) = stored.time_base;
    let start_us = stored.start * i64::from(num) * 1_000_000 / i64::from(den);
    assert!(
        (start_us - 70_000).abs() <= 1_000,
        "starts at {start_us} µs"
    );
}

/// The warning for a file FFmpeg can't open says why (FFmpeg's reason, which names no path).
#[test]
fn a_file_that_cant_be_opened_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes.mov");
    std::fs::write(&notes, b"not a video").unwrap();
    let path = notes.to_string_lossy().into_owned();
    let why = ffmpeg::format::input(&path).err().unwrap().to_string();
    let store = ThumbStore::new(dir.path().join("thumbs"), Arc::new(Os));
    let scanned = scan_thumbs(&store.shared(), &path, &[0], &|| true);
    assert_eq!(
        scanned.end,
        ScanEnd::Failed(format!("The file can't be opened: {why}"))
    );
}

/// A scan cuts tiles only: the poster is the poster maker's (`poster::make_poster`), so a file's tiles never wait on
/// it and a cache without one is never cut again for it.
#[test]
fn a_scan_leaves_the_poster_to_the_poster_maker() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("bars-30fps-0.5gop.mp4");
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    assert_eq!(
        scan_thumbs(&store.shared(), &video, &[0], &|| true).end,
        ScanEnd::Done
    );
    assert_eq!(strip(root.path(), &video).unwrap().chunks.len(), 1);
    assert_eq!(poster(root.path(), &video), None);
}
