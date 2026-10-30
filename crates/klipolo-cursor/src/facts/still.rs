//! Where the cursor rests, and the fade that hides a resting cursor.
//!
//! **Still runs.** A run is extended while each sample stays within `STILL_EPS` of the frame's diagonal of the run's
//! first sample. Distances are measured in points (`area_at`), so a move along the short side counts for less of the
//! picture's 0–1 than one along the long side. One pass over the kept samples: O(samples), no allocation but the runs
//! kept (16 bytes each, a few thousand for a long take). In release on a 2013 Xeon server, shared with other builds:
//! a two-hour tutorial take (0.45 M samples) in 2.5 ms; two hours of continuous motion at 250 Hz (1.3 M samples) in
//! 4.4 ms quiet, 7.5 ms shared, 1.4 ms of it reading the samples.
//!
//! **On time, not on samples.** A run ends where the raw path leaves it: the reader's path is
//! linear between kept samples, so the end is found on the line from the last sample inside to the first outside
//! (`Moved::leaves`), not at the last sample inside. The recorder thins moves a straight line reproduces, so a slow
//! drift may keep a sample only every gather (100 ms); its run still ends when the cursor has moved `STILL_EPS`, as a
//! dense track's does.
//!
//! **The idle fade** is a binary search for the run around a time and the app's hidden spans around it: O(log n), no
//! allocation, so it can run per frame.

use klipolo_model::cursor_style::CursorStyle;

use crate::ease::smootherstep;
use crate::track::{QUANTUM, TICK_US};
use crate::{CursorTrack, Us};

/// A move under this fraction of the frame's diagonal (measured in points with `area_at`) is "still".
pub const STILL_EPS: f32 = 0.002;
pub const FADE_OUT_US: Us = 250_000;
pub const FADE_IN_LEAD_US: Us = 150_000;
pub const HIDDEN_FADE_US: Us = 80_000;
/// Runs shorter than this aren't kept: nothing reads them (the shortest hide is 0.5 s, a dwell 0.6 s).
pub const MIN_RUN_US: Us = *CursorStyle::HIDE_US.start();

/// Distances as a share of the captured area's diagonal, measured in points. An unknown area (0 × 0) counts as square.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Diagonal {
    kx: f32,
    ky: f32,
}

impl Diagonal {
    pub(crate) fn of((w, h): (f32, f32)) -> Self {
        let (w2, h2) = (w * w, h * h);
        let d2 = w2 + h2;
        if w > 0.0 && h > 0.0 && d2.is_finite() {
            Self {
                kx: w2 / d2,
                ky: h2 / d2,
            }
        } else {
            Self { kx: 0.5, ky: 0.5 }
        }
    }

    /// The squared share of the diagonal a move of (dx, dy) of the picture covers.
    pub(crate) fn share2(&self, dx: f32, dy: f32) -> f32 {
        dx * dx * self.kx + dy * dy * self.ky
    }
}

/// The runs where the cursor rests (see the module notes), from their first kept position's time to where the raw path
/// leaves them.
#[derive(Debug, Default)]
pub struct StillRuns {
    /// Sorted and disjoint, each at least `MIN_RUN_US` long. The last counts too, ending at `Us::MAX`: the raw path
    /// holds after its last position. After a burn-in with no position since (`x`, the source went silent), nothing
    /// says the cursor rested, so the last run ends at the burn-in.
    runs: Vec<(Us, Us)>,
}

impl StillRuns {
    /// O(samples), over the stored ticks and 16-bit places: a time is computed only where a run closes.
    pub fn of(track: &CursorTrack) -> Self {
        let (ticks, xy) = track.stored();
        let Some((&tick0, &place0)) = ticks.first().zip(xy.first()) else {
            return Self::default();
        };
        // A run closing within this many ticks of its start can't last `MIN_RUN_US` (its end is before the closing
        // sample), so it needs no end: most samples while moving close one.
        let min_ticks = (MIN_RUN_US / TICK_US) as u32;
        let areas = track.area_changes();
        let mut moved = Moved::at(track.area_at(track.time(tick0)));
        // The next area change, and the first sample it applies to.
        let mut next_area = areas.partition_point(|a| a.0 <= track.time(tick0));
        let mut area_from = sample_from(track, areas.get(next_area));
        let mut runs = Vec::new();
        let (mut anchor, mut first, mut prev) = (place(place0), tick0, tick0);
        for (i, (&tick, &[x, y])) in ticks.iter().zip(xy).enumerate().skip(1) {
            if i == area_from {
                let t = track.time(tick);
                moved = Moved::at(track.area_at(t));
                next_area = areas.partition_point(|a| a.0 <= t);
                area_from = sample_from(track, areas.get(next_area));
            }
            // A branch, not a select: the anchor would make every sample wait on the one before (measured slower).
            if moved.from(anchor, x, y) {
                if tick - first > min_ticks {
                    // Where the line from the last sample inside to this one leaves the run, before this one's time.
                    let (a, b) = (track.time(prev), track.time(tick));
                    let along = moved.leaves(anchor, xy[i - 1], [x, y]);
                    let end = (a + ((b - a) as f64 * along) as Us).min(b - 1).max(a);
                    let from = track.time(first);
                    if end - from >= MIN_RUN_US {
                        runs.push((from, end));
                    }
                }
                (anchor, first) = (place([x, y]), tick);
            }
            prev = tick;
        }
        let end = (track.burned_from())
            .filter(|&burned| burned > track.time(prev))
            .unwrap_or(Us::MAX);
        if end == Us::MAX || end - track.time(first) >= MIN_RUN_US {
            runs.push((track.time(first), end));
        }
        runs.shrink_to_fit();
        Self { runs }
    }

    /// The run around `t` (both ends included). O(log n).
    pub fn at(&self, t: Us) -> Option<(Us, Us)> {
        let i = self.runs.partition_point(|r| r.0 <= t).checked_sub(1)?;
        let run = self.runs[i];
        (t <= run.1).then_some(run)
    }

    pub fn runs(&self) -> &[(Us, Us)] {
        &self.runs
    }
}

/// A sample moved `STILL_EPS` of the diagonal or more from the run's first: squared 16-bit steps weighted by the area's
/// shape.
#[derive(Clone, Copy)]
struct Moved {
    kx: f32,
    ky: f32,
    at_least: f32,
}

impl Moved {
    fn at(area: (f32, f32)) -> Self {
        let Diagonal { kx, ky } = Diagonal::of(area);
        let steps = STILL_EPS / QUANTUM as f32;
        Self {
            kx,
            ky,
            at_least: steps * steps,
        }
    }

    fn from(&self, anchor: [f32; 2], x: u16, y: u16) -> bool {
        let (dx, dy) = (f32::from(x) - anchor[0], f32::from(y) - anchor[1]);
        dx * dx * self.kx + dy * dy * self.ky >= self.at_least
    }

    /// How far (0–1) along the straight line from `a` (still) to `b` (moved) the path first moves `at_least` from
    /// `anchor`: the larger root of |a − anchor + s (b − a)|² = at_least in the area's metric. 1 when it can't be found
    /// (no movement, or not finite), so the run ends no earlier than before.
    fn leaves(&self, anchor: [f32; 2], a: [u16; 2], b: [u16; 2]) -> f64 {
        let (kx, ky) = (f64::from(self.kx), f64::from(self.ky));
        let d = [0, 1].map(|k| f64::from(a[k]) - f64::from(anchor[k]));
        let v = [0, 1].map(|k| f64::from(b[k]) - f64::from(a[k]));
        let qa = kx * v[0] * v[0] + ky * v[1] * v[1];
        let qb = 2.0 * (kx * d[0] * v[0] + ky * d[1] * v[1]);
        let qc = kx * d[0] * d[0] + ky * d[1] * d[1] - f64::from(self.at_least);
        let s = (-qb + (qb * qb - 4.0 * qa * qc).max(0.0).sqrt()) / (2.0 * qa);
        if s.is_finite() {
            s.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

fn place([x, y]: [u16; 2]) -> [f32; 2] {
    [f32::from(x), f32::from(y)]
}

/// The index of the first sample at or after `change` (none: past the last).
fn sample_from(track: &CursorTrack, change: Option<&(Us, f32, f32)>) -> usize {
    let (ticks, _) = track.stored();
    change.map_or(usize::MAX, |c| {
        ticks.partition_point(|&k| track.time(k) < c.0)
    })
}

fn since(from: Us, t: Us) -> f64 {
    t.saturating_sub(from) as f64
}

/// 0–1: hidden after `hide_after` still (fading out over FADE_OUT_US), back from FADE_IN_LEAD_US before the next move;
/// an app's hidden cursor fades over HIDDEN_FADE_US. O(log n), no allocation.
pub fn idle_opacity(track: &CursorTrack, still: &StillRuns, t: Us, hide_after: Option<Us>) -> f32 {
    let resting = match (hide_after, still.at(t)) {
        (Some(after), Some((from, to))) => {
            let gone = smootherstep(since(from.saturating_add(after), t) / FADE_OUT_US as f64);
            let back = if to == Us::MAX {
                0.0
            } else {
                let lead = to.saturating_sub(FADE_IN_LEAD_US);
                smootherstep(since(lead, t) / FADE_IN_LEAD_US as f64)
            };
            (1.0 - gone).max(back)
        }
        _ => 1.0,
    };
    (resting * hidden_fade(track.hidden(), t)) as f32
}

/// 1 while shown; from where an app hides the cursor, down to 0 over `HIDDEN_FADE_US`, and back up as quickly once it
/// shows it again. The two latest hides are both read, so a quick flicker never jumps.
fn hidden_fade(hidden: &[(Us, Us)], t: Us) -> f64 {
    let i = hidden.partition_point(|h| h.0 <= t);
    let fade = |(from, to): (Us, Us)| {
        let out = |at: Us| smootherstep(since(from, at) / HIDDEN_FADE_US as f64);
        if t < to {
            1.0 - out(t)
        } else {
            1.0 - out(to) * (1.0 - smootherstep(since(to, t) / HIDDEN_FADE_US as f64))
        }
    };
    hidden[i.saturating_sub(2)..i]
        .iter()
        .map(|&h| fade(h))
        .fold(1.0, f64::min)
}
