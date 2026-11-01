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

/// The mean colour of `picture` within `(x, y, w, h)`.
fn mean_rgb(picture: &image::RgbImage, (x, y, w, h): (u32, u32, u32, u32)) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for py in y..y + h {
        for px in x..x + w {
            for (c, value) in picture.get_pixel(px, py).0.iter().enumerate() {
                sum[c] += f64::from(*value);
            }
        }
    }
    sum.map(|s| s / f64::from(w * h))
}

/// A scan writes the file's first picture again as its poster, fitted in 480 × 270 instead of the tile's 72 px tall,
/// for the start screen's cards: the picture tile 0 shows, in the same colours (an MJPEG file's full range too).
#[test]
fn a_scan_writes_a_poster_of_the_first_picture() {
    for name in ["bars-30fps-0.5gop.mp4", "intra-6s-mjpeg.mov"] {
        let root = tempfile::tempdir().unwrap();
        let video = fixture(name);
        let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
        assert_eq!(
            poster(root.path(), &video),
            None,
            "{name}: none before a scan"
        );
        let scanned = scan_thumbs(&store.shared(), &video, &[0], &|| true);
        assert_eq!(scanned.end, ScanEnd::Done, "{name}");

        let found = poster(root.path(), &video).expect("a poster");
        assert_eq!((found.w, found.h), (480, 270), "{name}");
        let picture = image::open(&found.path).unwrap().to_rgb8();
        assert_eq!(picture.dimensions(), (480, 270), "{name}");
        let strip = strip(root.path(), &video).unwrap();
        assert_eq!(
            (strip.tile_w, strip.tile_h),
            (128, 72),
            "{name}: the tiles stay as they were"
        );
        let sheet = image::open(&strip.chunks[0].path).unwrap().to_rgb8();
        // Region by region (an 8 × 4 grid over each), so a darker or paler picture shows where it is.
        for row in 0..4 {
            for col in 0..8 {
                let cell = |w: u32, h: u32| (col * w / 8, row * h / 4, w / 8, h / 4);
                let tile = mean_rgb(&sheet, cell(strip.tile_w, strip.tile_h));
                let big = mean_rgb(&picture, cell(480, 270));
                for c in 0..3 {
                    assert!(
                        (tile[c] - big[c]).abs() < 12.0,
                        "{name}, cell {col},{row}: tile 0 {tile:?}, poster {big:?}"
                    );
                }
            }
        }
    }
}

/// A phone video stored turned has an upright poster: shown 720 × 1280 (red on top, blue below), fitted to 152 × 270.
#[test]
fn a_turned_phone_video_has_an_upright_poster() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("phone-portrait.mov");
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    let scanned = scan_thumbs(&store.shared(), &video, &[0], &|| true);
    assert_eq!(scanned.end, ScanEnd::Done);
    let found = poster(root.path(), &video).expect("a poster");
    assert_eq!((found.w, found.h), (152, 270));
    let picture = image::open(&found.path).unwrap().to_rgb8();
    let [r, _, b] = mean_rgb(&picture, (0, 0, 152, 100));
    assert!(r > 150.0 && b < 80.0, "top {r} red, {b} blue");
    let [r, _, b] = mean_rgb(&picture, (0, 170, 152, 100));
    assert!(b > 150.0 && r < 80.0, "bottom {r} red, {b} blue");
}

/// A cache from before posters (chunk 0 and the keyframe index on disk, no poster): a scan asked for chunk 0 cuts it
/// again for the poster, and keeps the chunk on disk as it was.
#[test]
fn a_cache_without_a_poster_gets_one_and_keeps_its_chunk() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("bars-30fps-0.5gop.mp4");
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    assert_eq!(
        scan_thumbs(&store.shared(), &video, &[0], &|| true).end,
        ScanEnd::Done
    );
    std::fs::remove_file(poster(root.path(), &video).unwrap().path).unwrap();
    let chunk = strip(root.path(), &video).unwrap().chunks[0].path.clone();
    let kept = std::fs::metadata(&chunk).unwrap().modified().unwrap();

    let scanned = scan_thumbs(&store.shared(), &video, &[0], &|| true);
    assert_eq!(scanned.end, ScanEnd::Done);
    assert!(scanned.decoded > 0, "chunk 0 was cut again");
    assert!(poster(root.path(), &video).is_some());
    assert_eq!(std::fs::metadata(&chunk).unwrap().modified().unwrap(), kept);
}
