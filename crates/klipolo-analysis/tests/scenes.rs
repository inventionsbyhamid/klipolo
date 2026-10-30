//! The scene log's small copy and its cuts: clip A's 7 hard cuts found to the frame, the finder's rules on made-up
//! frames, the copy's chunks, a ready copy as one chunk, and (on a Mac only: VideoToolbox) the copy itself, in which
//! the same cuts are found across chunk joins. Elsewhere the committed fixture stands in for the copy.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use klipolo_analysis::cuts::{self, Finder, Histogram};
use klipolo_analysis::proxy;
use klipolo_model::time::{RangeUs, Us};

/// Clip A's 7 hard cuts, as frame numbers at 30 fps (13.47, 24.07, 52.07, 62.70, 86.07, 96.10, 150.67 s).
const CLIP_A_CUT_FRAMES: [i64; 7] = [404, 722, 1562, 1881, 2582, 2883, 4520];

/// One frame at 30 fps, rounded up.
const FRAME_US: Us = 33_334;

fn frame_us(frame: i64) -> Us {
    (frame as f64 * 1e6 / 30.0).round() as Us
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn clip_a() -> PathBuf {
    fixture("clip_a_360p.mp4")
}

fn assert_clip_a_cuts(cuts: &[Us]) {
    assert_eq!(cuts.len(), 7, "{cuts:?}");
    for (&found, &frame) in cuts.iter().zip(&CLIP_A_CUT_FRAMES) {
        let want = frame_us(frame);
        assert!(
            (found - want).abs() <= FRAME_US,
            "cut at {found} µs, want frame {frame} ({want} µs): {cuts:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Cuts

#[test]
fn cuts_detect_known_cuts_of_clip_a_within_one_frame() {
    let cuts = cuts::detect(&clip_a(), &AtomicBool::new(false)).unwrap();
    assert_clip_a_cuts(&cuts);
}

/// A histogram of 57,600 pixels (320 × 180) with `bright` of them in bin 50 and the rest in bin 10.
fn split(bright: u32) -> Histogram {
    let mut h = [0; cuts::BINS];
    h[50] = bright;
    h[10] = 57_600 - bright;
    h
}

#[test]
fn the_cut_finder_dates_a_hard_cut_to_its_frame_and_lets_a_fade_or_a_flicker_pass() {
    let mut finder = Finder::new();
    let at = |n: i64| frame_us(n);
    let mut n = 0;
    // 2 s dark, then a hard cut to bright at frame 60 (2.0 s).
    while n < 60 {
        finder.push_histogram(at(n), split(0));
        n += 1;
    }
    while n < 120 {
        finder.push_histogram(at(n), split(57_600));
        n += 1;
    }
    // A fade back to dark over half a second (15 frames): each frame moves 1/15 of the picture (0.067), a quarter
    // second about half of it.
    for step in 1..=15 {
        finder.push_histogram(at(n), split(57_600 - 3_840 * step));
        n += 1;
    }
    while n < 200 {
        finder.push_histogram(at(n), split(0));
        n += 1;
    }
    // Then a sudden change of 15% of the picture between two frames: under the threshold.
    while n < 260 {
        finder.push_histogram(at(n), split(8_640));
        n += 1;
    }
    // And a hard cut on the last frame.
    finder.push_histogram(at(n), split(57_600));
    assert_eq!(finder.finish(), vec![at(60), at(260)]);

    // The same distance the module documents: half the L1 distance, 0 to 1.
    assert_eq!(cuts::distance(&split(0), &split(57_600)), 1.0);
    assert!((cuts::distance(&split(0), &split(8_640)) - 0.15).abs() < 1e-9);
    let grey = vec![200u8; 57_600];
    let h = cuts::histogram(&grey);
    assert_eq!((h[50], h.iter().sum::<u32>()), (57_600, 57_600));
}

// ---------------------------------------------------------------------------------------------------------------------
// The copy's chunks

const S: Us = 1_000_000;

fn range(start_s: f64, end_s: f64) -> RangeUs {
    RangeUs {
        start: (start_s * 1e6).round() as Us,
        end: (end_s * 1e6).round() as Us,
    }
}

#[test]
fn chunks_cover_a_long_clip_evenly_with_twenty_seconds_of_overlap() {
    let (max, overlap) = (proxy::CHUNK_US, proxy::OVERLAP_US);
    assert_eq!(proxy::plan(155 * S, max, overlap), vec![range(0.0, 155.0)]);
    assert_eq!(proxy::plan(600 * S, max, overlap), vec![range(0.0, 600.0)]);
    assert_eq!(
        proxy::plan(610 * S, max, overlap),
        vec![range(0.0, 315.0), range(295.0, 610.0)]
    );
    // 25 minutes: three chunks of 513.33 s.
    let long = proxy::plan(1500 * S, max, overlap);
    assert_eq!(long.len(), 3, "{long:?}");
    assert_eq!((long[0].start, long[2].end), (0, 1500 * S));
    for pair in long.windows(2) {
        assert_eq!(pair[0].end - pair[1].start, overlap, "{long:?}");
    }
    for chunk in &long {
        assert!(chunk.end - chunk.start <= max, "{long:?}");
        // Equal to the microsecond, but for the rounding the last one takes up.
        assert!(
            (chunk.end - chunk.start - 513_333_334).abs() <= 3,
            "{long:?}"
        );
    }
}

#[test]
fn a_ready_copy_is_one_chunk_with_the_clips_facts() {
    let copy = proxy::existing(&clip_a()).unwrap();
    // The file's length: its sound ends 16 ms after its picture.
    assert!(
        (copy.duration_us - 155 * S).abs() < 50_000,
        "{}",
        copy.duration_us
    );
    assert_eq!(copy.chunks.len(), 1);
    assert_eq!(
        copy.chunks[0].range,
        RangeUs {
            start: 0,
            end: copy.duration_us
        }
    );
    assert_eq!(copy.chunks[0].path, clip_a());
    assert_eq!(
        copy.chunks[0].bytes,
        std::fs::metadata(clip_a()).unwrap().len()
    );
    assert_eq!(
        (copy.width, copy.height, copy.has_audio, copy.audio_lead_us),
        (640, 360, true, 0)
    );
    assert!((copy.fps - 30.0).abs() < 1e-6, "{}", copy.fps);
}

// ---------------------------------------------------------------------------------------------------------------------
// The small copy itself: VideoToolbox, so on a Mac only

/// Where `kind` (a top-level MP4 box) starts in `bytes`.
#[cfg(target_os = "macos")]
fn box_at(bytes: &[u8], kind: &[u8; 4]) -> Option<usize> {
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if &bytes[at + 4..at + 8] == kind {
            return Some(at);
        }
        if size < 8 {
            return None;
        }
        at += size;
    }
    None
}

#[cfg(target_os = "macos")]
#[test]
fn makes_a_small_copy_on_this_mac_and_finds_the_same_cuts_in_it() {
    use klipolo_engine::render::probe::probe_media;
    let cancel = AtomicBool::new(false);
    let dir = tempfile::tempdir().unwrap();
    let copy = proxy::make(&clip_a(), dir.path(), 360, &cancel).unwrap();
    assert_eq!(copy.chunks.len(), 1);
    assert_eq!((copy.width, copy.height, copy.has_audio), (640, 360, true));
    let chunk = &copy.chunks[0];
    assert_eq!(
        chunk.range,
        RangeUs {
            start: 0,
            end: copy.duration_us
        }
    );
    assert!(
        (copy.duration_us - 155 * S).abs() < 50_000,
        "{}",
        copy.duration_us
    );
    let bytes = std::fs::read(&chunk.path).unwrap();
    assert_eq!(chunk.bytes, bytes.len() as u64);
    eprintln!("clip A's copy at 640 x 360: {} bytes", bytes.len());
    // H.264 and AAC, its index before its media (faststart).
    assert!(bytes.windows(4).any(|w| w == b"avc1") && bytes.windows(4).any(|w| w == b"mp4a"));
    assert!(box_at(&bytes, b"moov").unwrap() < box_at(&bytes, b"mdat").unwrap());
    // About 300 kb/s of picture and 48 kb/s of sound.
    assert!(
        bytes.len() < 155 * (300_000 + 48_000) / 8 * 13 / 10,
        "{}",
        bytes.len()
    );
    let info = probe_media(&chunk.path).unwrap();
    assert!((info.duration - 155.0).abs() < 0.2, "{}", info.duration);
    assert_eq!(info.audio.map(|a| a.channels), Some(1));
    assert_clip_a_cuts(&cuts::of_proxy(&copy, &cancel).unwrap());

    // In four chunks of 53.75 s at 320 x 180, the cuts still land on their frames across the joins.
    let dir = tempfile::tempdir().unwrap();
    let chunked =
        proxy::make_in_chunks(&clip_a(), dir.path(), 180, (60 * S, 20 * S), &cancel).unwrap();
    assert_eq!(
        chunked.chunks.iter().map(|c| c.range).collect::<Vec<_>>(),
        proxy::plan(chunked.duration_us, 60 * S, 20 * S)
    );
    assert_eq!((chunked.width, chunked.height), (320, 180));
    for chunk in &chunked.chunks {
        eprintln!("chunk {:?}: {} bytes", chunk.range, chunk.bytes);
    }
    assert_clip_a_cuts(&cuts::of_proxy(&chunked, &cancel).unwrap());
}
