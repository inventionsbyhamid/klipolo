//! A clip's hard cuts, found from how its brightness spread changes.
//!
//! Each frame becomes a 320 × 180 grey picture (`klipolo_engine::media::luma_frames`) and a 64-bin histogram of it. Two
//! histograms differ by half their L1 distance: the share of the picture whose brightness moved to another bin (0 to
//! 1). A hard cut changes that share a lot at once; motion and pans change it a little each frame.
//!
//! Samples are taken 4 times a second (the frame on screen every 250 ms). A cut is where two samples in a row differ
//! by at least `THRESHOLD`, and the largest change between two frames in a row inside that quarter second does too: it
//! is dated to that frame, the first of the new shot. The second test keeps a fast pan or a fade (big over a quarter
//! second, small from frame to frame) from counting.
//!
//! **Tuning (clip A, 155 s, 30 fps, its 7 known cuts):** a quarter second scores the lower of its two changes. In the
//! original file, the committed fixture and the 360p copies made of both, every cut scores 0.247 to 0.350 and no other
//! quarter second more than 0.130; 0.18 sits about 1.4 times from each. A cut between two shots of the same look and
//! light (the same room from another camera) changes the histogram too little to be found.
//!
//! **Where the frames come from:** `detect` decodes a file at its own rate; `proxy::make` feeds a `Finder` every frame
//! of the source, at its own rate, while it makes the copy it uploads (which keeps fewer), so the cuts are the
//! source's own frames either way.
//!
//! **Times** are on the video's file clock (its first frame is 0).

use std::path::Path;
use std::sync::atomic::AtomicBool;

use klipolo_engine::media::luma_frames::{self, LumaError};
use klipolo_model::time::{RangeUs, Us};

use crate::error::VideoError;

/// Samples a second: 4.
pub const SAMPLE_US: Us = 250_000;
/// The change (0 to 1) a cut makes, both between samples and between two frames in a row.
pub const THRESHOLD: f64 = 0.18;
/// The grey picture each frame is measured on.
pub const PICTURE: (u32, u32) = (320, 180);
/// Brightness bins.
pub const BINS: usize = 64;

/// A grey picture's brightness spread: pixels per bin (4 levels a bin).
pub type Histogram = [u32; BINS];

/// `pixels`' histogram.
pub fn histogram(pixels: &[u8]) -> Histogram {
    let mut h = [0; BINS];
    for &p in pixels {
        h[usize::from(p >> 2)] += 1;
    }
    h
}

/// Half the L1 distance between two histograms of the same number of pixels: 0 (same spread) to 1.
pub fn distance(a: &Histogram, b: &Histogram) -> f64 {
    let pixels = a.iter().sum::<u32>().max(1);
    let moved: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| u64::from(x.abs_diff(y)))
        .sum();
    moved as f64 / (2.0 * f64::from(pixels))
}

/// Finds cuts in frames handed over one by one, in order (see the module notes).
///
/// Quarter seconds run (k − 1) × 250 ms to k × 250 ms; a frame at a boundary belongs to the quarter it ends. The sample
/// at a boundary is the frame on screen then: the latest at or before it.
#[derive(Debug)]
pub struct Finder {
    /// The previous frame: its time and histogram.
    last: Option<(Us, Histogram)>,
    /// The sample that starts the current quarter second.
    sample: Histogram,
    /// Where the current quarter second ends.
    quarter_end: Us,
    /// The largest change between two frames in a row in this quarter second, and the time of the second frame.
    biggest: Option<(f64, Us)>,
    cuts: Vec<Us>,
}

impl Default for Finder {
    fn default() -> Self {
        Self {
            last: None,
            sample: [0; BINS],
            quarter_end: 0,
            biggest: None,
            cuts: Vec::new(),
        }
    }
}

impl Finder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next frame: its time and its grey picture.
    pub fn push(&mut self, at: Us, pixels: &[u8]) {
        self.push_histogram(at, histogram(pixels));
    }

    /// The next frame, as its histogram.
    pub fn push_histogram(&mut self, at: Us, histogram: Histogram) {
        let Some((_, last)) = self.last else {
            // The first frame is the first sample.
            self.sample = histogram;
            self.quarter_end = (at.div_euclid(SAMPLE_US) + 1) * SAMPLE_US;
            self.last = Some((at, histogram));
            return;
        };
        while at > self.quarter_end {
            self.end_quarter(&last);
        }
        let change = distance(&last, &histogram);
        if self.biggest.is_none_or(|(biggest, _)| change > biggest) {
            self.biggest = Some((change, at));
        }
        self.last = Some((at, histogram));
    }

    /// The cuts found: each the time of the first frame of a new shot, in order.
    pub fn finish(mut self) -> Vec<Us> {
        if let Some((_, last)) = self.last {
            self.end_quarter(&last);
        }
        self.cuts
    }

    /// Closes the current quarter second, `on_screen` the frame on screen at its end.
    fn end_quarter(&mut self, on_screen: &Histogram) {
        if let Some((biggest, at)) = self.biggest.take()
            && biggest >= THRESHOLD
            && distance(&self.sample, on_screen) >= THRESHOLD
        {
            self.cuts.push(at);
        }
        self.sample = *on_screen;
        self.quarter_end += SAMPLE_US;
    }
}

/// The hard cuts in `video` (every frame decoded). Blocking; `cancel` stops it between packets.
pub fn detect(video: &Path, cancel: &AtomicBool) -> Result<Vec<Us>, VideoError> {
    let mut finder = Finder::new();
    luma_frames::each_frame(video, PICTURE, cancel, &mut |at, pixels| {
        finder.push(at, pixels);
        true
    })
    .map_err(|e| match e {
        LumaError::NoVideo => VideoError::NoVideo,
        LumaError::Stopped => VideoError::Stopped,
        LumaError::Failed(e) => VideoError::Failed(e),
    })?;
    Ok(finder.finish())
}

/// Each chunk's cuts (the clip's time) kept inside its chunk, in order, once each: a cut in an overlap is found by
/// both chunks, to the frame.
pub fn merge(chunks: Vec<(RangeUs, Vec<Us>)>) -> Vec<Us> {
    let mut all: Vec<Us> = chunks
        .into_iter()
        .flat_map(|(range, cuts)| {
            cuts.into_iter()
                .filter(move |at| (range.start..range.end).contains(at))
        })
        .collect();
    all.sort_unstable();
    all.dedup_by(|later, earlier| *later - *earlier < SAME_CUT_US);
    all
}

/// Two cuts closer than this (under two frames at 30 fps) are one cut found twice.
const SAME_CUT_US: Us = 50_000;
