//! The scene log's cut finder: clip A's 7 hard cuts found to the frame in the committed copy of it, and the finder's
//! rules on made-up frames.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use klipolo_analysis::cuts::{self, Finder, Histogram};
use klipolo_model::time::Us;

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
