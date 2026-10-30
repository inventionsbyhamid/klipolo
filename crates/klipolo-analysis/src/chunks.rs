//! A long recording cut into Voice Transcribe requests (at most `max_s` each; the API takes 600 s) at its pauses.
//!
//! Each cut goes in the quietest 300 ms window (by RMS, on a 10 ms grid) of the last minute before the limit, at the
//! window's middle. The floor is the file's own room tone: its 10th-percentile window plus 6 dB (and never below
//! -60 dBFS, so a file that is partly digital silence doesn't make every room tone count as sound). A window louder
//! than the floor is sound; when the last minute has no window under it, the cut still goes in the quietest one and
//! `Plan::loud_cuts` says so, for the log. The chunks cover the file end to end with no gap and no overlap: the
//! transcript adds each chunk's start to its times. Like the WAV they come from, chunk times are the clip's sound time
//! (`audio`'s module notes): adding a chunk's start gives sound time, not file time.

use klipolo_model::time::{RangeUs, Us};

use crate::audio;

/// The window a cut is centred in.
pub const WINDOW_MS: u32 = 300;
/// How far before the limit a cut is looked for.
pub const SEARCH_S: u32 = 60;
/// The grid windows start on.
const STEP_MS: u32 = 10;
const FLOOR_PERCENTILE: f64 = 0.10;
const FLOOR_ABOVE_DB: f64 = 6.0;
const FLOOR_MIN_DBFS: f64 = -60.0;

/// A recording's chunks, and which cuts had to go in sound.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// In order, from 0 to the end of the file, each starting where the one before it ends.
    pub chunks: Vec<RangeUs>,
    /// The cuts (chunk ends) with no window under the floor in reach: they may split a word. Empty for speech with
    /// pauses.
    pub loud_cuts: Vec<Us>,
}

/// `pcm` (mono, `rate` Hz) in chunks of at most `max_s` seconds, cut at pauses. No sound, no chunks.
pub fn plan(pcm: &[i16], rate: u32, max_s: u32) -> Vec<RangeUs> {
    plan_with_loud_cuts(pcm, rate, max_s).chunks
}

/// `plan`, with the cuts that had to go in sound.
pub fn plan_with_loud_cuts(pcm: &[i16], rate: u32, max_s: u32) -> Plan {
    let total = pcm.len();
    if total == 0 || rate == 0 {
        return Plan::default();
    }
    let rate_n = rate as usize;
    let max = (max_s.max(1) as usize).saturating_mul(rate_n);
    if total <= max {
        return Plan {
            chunks: vec![RangeUs {
                start: 0,
                end: us(total, rate),
            }],
            loud_cuts: Vec::new(),
        };
    }
    let step = (rate_n * STEP_MS as usize / 1000).max(1);
    let window = step * (WINDOW_MS / STEP_MS) as usize;
    let levels = Levels::of(pcm, step, window);
    let floor = levels.floor();
    let search = SEARCH_S as usize * rate_n;
    let mut plan = Plan::default();
    let mut start = 0;
    while total - start > max {
        let limit = start + max;
        // Windows wholly inside the last minute before the limit (and after the chunk's start).
        let from = limit.saturating_sub(search).max(start).div_ceil(step);
        let to = limit.checked_sub(window).map(|s| s / step);
        let cut = match to.and_then(|to| levels.quietest(from, to)) {
            Some(i) => {
                if levels.sum(i) as f64 > floor {
                    plan.loud_cuts.push(us(i * step + window / 2, rate));
                }
                i * step + window / 2
            }
            // No whole window fits (a limit shorter than a window): cut at the limit.
            None => {
                plan.loud_cuts.push(us(limit, rate));
                limit
            }
        };
        plan.chunks.push(RangeUs {
            start: us(start, rate),
            end: us(cut, rate),
        });
        start = cut;
    }
    plan.chunks.push(RangeUs {
        start: us(start, rate),
        end: us(total, rate),
    });
    plan
}

/// Sample index to µs (rounded down; `sample_at` turns it back into the same index).
fn us(sample: usize, rate: u32) -> Us {
    (sample as u128 * 1_000_000 / u128::from(rate)) as Us
}

/// µs to the nearest sample index.
fn sample_at(t: Us, rate: u32) -> usize {
    ((t.max(0) as u128 * u128::from(rate) + 500_000) / 1_000_000).min(usize::MAX as u128) as usize
}

/// The energy (sum of squares) of every window, one starting each step.
struct Levels {
    sums: Vec<u64>,
    window: usize,
}

impl Levels {
    fn of(pcm: &[i16], step: usize, window: usize) -> Self {
        let steps: Vec<u64> = pcm
            .chunks_exact(step)
            .map(|s| {
                s.iter()
                    .map(|&x| (i64::from(x) * i64::from(x)) as u64)
                    .sum()
            })
            .collect();
        let per = window / step;
        let mut sums = Vec::with_capacity(steps.len().saturating_sub(per) + 1);
        if steps.len() >= per {
            let mut sum: u64 = steps[..per].iter().sum();
            sums.push(sum);
            for i in per..steps.len() {
                sum = sum + steps[i] - steps[i - per];
                sums.push(sum);
            }
        }
        Self { sums, window }
    }

    fn sum(&self, i: usize) -> u64 {
        self.sums[i]
    }

    /// The floor as a window energy: the 10th-percentile window plus 6 dB, at least -60 dBFS.
    fn floor(&self) -> f64 {
        let min = (32767.0 * 10f64.powf(FLOOR_MIN_DBFS / 20.0)).powi(2) * self.window as f64;
        if self.sums.is_empty() {
            return min;
        }
        let mut sorted = self.sums.clone();
        let k = ((sorted.len() - 1) as f64 * FLOOR_PERCENTILE) as usize;
        let (_, &mut p10, _) = sorted.select_nth_unstable(k);
        (p10 as f64 * 10f64.powf(FLOOR_ABOVE_DB / 10.0)).max(min)
    }

    /// The quietest window from `from` to `to` (inclusive), the later one of equals (a longer chunk).
    fn quietest(&self, from: usize, to: usize) -> Option<usize> {
        let to = to.min(self.sums.len().checked_sub(1)?);
        (from..=to).reduce(|best, i| {
            if self.sums[i] <= self.sums[best] {
                i
            } else {
                best
            }
        })
    }
}

/// The part of a mono or multichannel 16-bit WAV (`audio::extract_wav_16k`'s) within `range`, as a WAV of its own:
/// a Voice Transcribe request. A range past the end stops at the end. Empty when `wav` isn't a 16-bit PCM WAV.
pub fn wav_slice(wav: &[u8], range: RangeUs) -> Vec<u8> {
    let Some(layout) = audio::layout(wav) else {
        return Vec::new();
    };
    let frame = 2 * usize::from(layout.channels);
    let frames = layout.data.len() / frame;
    let start = sample_at(range.start, layout.rate).min(frames);
    let end = sample_at(range.end, layout.rate).clamp(start, frames);
    let bytes = &wav[layout.data.start + start * frame..layout.data.start + end * frame];
    let mut out = Vec::with_capacity(44 + bytes.len());
    out.extend_from_slice(&audio::wav_header(
        layout.rate,
        layout.channels,
        bytes.len() as u32,
    ));
    out.extend_from_slice(bytes);
    out
}
