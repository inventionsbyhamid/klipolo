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
//! A cut also changes the look for good: the picture on screen a quarter second before the jump and the one a quarter
//! second or more after it differ by `THRESHOLD` too. That keeps a flash or a flicker that returns to the old look from
//! counting, at its start and at its end alike (the frame just before a flash's end is the flash itself, so the look
//! is taken from before it began).
//!
//! **A lower bound:** the cuts found are cuts, but not every cut is found. A cut between two shots of the same look and
//! light (a jump cut, the same room from another camera) changes the histogram too little, and a shot shorter than a
//! quarter second reads as a flash.
//!
//! **Tuning (clip A, 155 s, 30 fps, its 7 known cuts):** a quarter second scores the lower of its two changes. In the
//! original file, the committed fixture and the 360p copies made of both, every cut scores 0.247 to 0.350 and no other
//! quarter second more than 0.130; 0.18 sits about 1.4 times from each.
//!
//! **Where the frames come from:** `detect` decodes a file at its own rate; `proxy::make` feeds a `Finder` every frame
//! of the source, at its own rate, while it makes the copy it uploads (which keeps fewer), so the cuts are the
//! source's own frames either way.
//!
//! **Times** are on the video's file clock (its first frame is 0).

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use klipolo_engine::media::luma_frames::{self, LumaError};
use klipolo_engine::media::small_copy::EveryFrame;
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
/// at a boundary is the frame on screen then: the latest at or before it. A jump that passes both tests waits for a
/// frame a quarter second after it (or the last frame) to be confirmed.
#[derive(Debug)]
pub struct Finder {
    /// The previous frame: its time and histogram.
    last: Option<(Us, Histogram)>,
    /// The sample that starts the current quarter second.
    sample: Histogram,
    /// Where the current quarter second ends.
    quarter_end: Us,
    /// The largest change between two frames in a row in this quarter second: its size, the time of the second frame,
    /// and the picture on screen a quarter second before it.
    biggest: Option<(f64, Us, Histogram)>,
    /// The last quarter second's frames and the one on screen when it began: what was on screen before a jump.
    recent: VecDeque<(Us, Histogram)>,
    /// Jumps waiting for a frame a quarter second after them: their time and the picture a quarter second before.
    pending: VecDeque<(Us, Histogram)>,
    cuts: Vec<Us>,
}

impl Default for Finder {
    fn default() -> Self {
        Self {
            last: None,
            sample: [0; BINS],
            quarter_end: 0,
            biggest: None,
            recent: VecDeque::new(),
            pending: VecDeque::new(),
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
        if let Some((_, last)) = self.last {
            while at > self.quarter_end {
                self.end_quarter(&last);
            }
            self.confirm(Some(at), &histogram);
            let change = distance(&last, &histogram);
            if self
                .biggest
                .as_ref()
                .is_none_or(|(biggest, ..)| change > *biggest)
            {
                self.biggest = Some((change, at, self.on_screen(at - SAMPLE_US)));
            }
        } else {
            // The first frame is the first sample.
            self.sample = histogram;
            self.quarter_end = (at.div_euclid(SAMPLE_US) + 1) * SAMPLE_US;
        }
        self.recent.push_back((at, histogram));
        // Keep the frame on screen a quarter second before this one, and every frame since.
        while self.recent.len() > 1 && self.recent[1].0 <= at - SAMPLE_US {
            self.recent.pop_front();
        }
        self.last = Some((at, histogram));
    }

    /// The cuts found: each the time of the first frame of a new shot, in order.
    pub fn finish(mut self) -> Vec<Us> {
        if let Some((_, last)) = self.last {
            self.end_quarter(&last);
            // Jumps in the last quarter second are judged by the last frame.
            self.confirm(None, &last);
        }
        self.cuts
    }

    /// Closes the current quarter second, `on_screen` the frame on screen at its end.
    fn end_quarter(&mut self, on_screen: &Histogram) {
        if let Some((biggest, at, before)) = self.biggest.take()
            && biggest >= THRESHOLD
            && distance(&self.sample, on_screen) >= THRESHOLD
        {
            self.pending.push_back((at, before));
        }
        self.sample = *on_screen;
        self.quarter_end += SAMPLE_US;
    }

    /// Judges the waiting jumps a quarter second or more before `at` (all of them for `None`) by `after`, the picture
    /// then: a jump whose before and after differ by `THRESHOLD` is a cut.
    fn confirm(&mut self, at: Option<Us>, after: &Histogram) {
        while let Some(&(jump, before)) = self.pending.front() {
            if at.is_some_and(|at| at < jump + SAMPLE_US) {
                break;
            }
            self.pending.pop_front();
            if distance(&before, after) >= THRESHOLD {
                self.cuts.push(jump);
            }
        }
    }

    /// The picture on screen at `at` among the recent frames (the oldest kept, for a time before it).
    fn on_screen(&self, at: Us) -> Histogram {
        self.recent
            .iter()
            .rev()
            .find(|(t, _)| *t <= at)
            .or(self.recent.front())
            .map_or([0; BINS], |(_, h)| *h)
    }
}

/// The small copy hands its source's frames straight to a `Finder` (`proxy::make`), and says when it starts again.
impl EveryFrame for Finder {
    fn size(&self) -> (u32, u32) {
        PICTURE
    }

    fn see(&mut self, at: Us, pixels: &[u8]) {
        self.push(at, pixels);
    }

    fn again(&mut self) {
        *self = Finder::new();
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
