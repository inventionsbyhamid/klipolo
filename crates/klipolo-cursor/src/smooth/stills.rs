//! Where the raw path rests: runs of kept positions within `STILL_EPS` of the run's first. A long one ends a moving
//! segment (`build`):
//! - the follower's look-ahead stops at its start, so it never sees the next movement before that begins;
//! - the drawn point eases onto the raw path over 4/ω, so the segment hands back with no jump, however far the
//!   follower still had to settle (a critically damped tail is slow: 4/ω after a step it is still 9 % short).
//!
//! A run ends at its last kept position inside the dead zone: the raw path is linear from there to the next kept
//! position, so that is where it starts to move, on a thinned track as on a dense one (the recorder keeps the held place
//! before a move and the rest point after one). Lengths are times, never sample counts.
//!
//! A take finds its rests once (`Rests::of`), long enough for its fastest smoothing, and each smoothing keeps those
//! long enough for it (`Rests::at_least`): one scan of the samples for all of a take's variants.

use klipolo_model::cursor_style::Smoothing;

use super::STEP_US;
use crate::{CursorTrack, Us};

/// Movement smaller than this from a run's first position isn't movement (0.54 px at 1080p): a segment starts once the
/// raw path leaves it.
pub(super) const STILL_EPS: f64 = 0.0005;

/// How long a still eases the drawn point onto the raw path at `omega` (4/ω).
pub(super) fn rest_us(omega: f64) -> Us {
    (4.0 / omega * 1e6) as Us
}

/// The shortest still any smoothing ends a segment in: the fastest one's ease plus a step.
fn shortest() -> Us {
    [Smoothing::Light, Smoothing::Medium, Smoothing::Strong]
        .iter()
        .filter_map(|s| s.omega())
        .map(|omega| rest_us(omega) + STEP_US)
        .min()
        .unwrap_or(0)
}

/// A take's rests, found once for every smoothing it is drawn with (see the module notes).
#[derive(Debug, Default)]
pub struct Rests {
    runs: Vec<(Us, Us)>,
    min_us: Us,
}

impl Rests {
    /// O(samples); 16 bytes a rest at least as long as the fastest smoothing's (a few thousand for a long take).
    pub fn of(track: &CursorTrack) -> Self {
        let min_us = shortest();
        let mut runs = long(track, min_us);
        runs.shrink_to_fit();
        Self { runs, min_us }
    }

    pub fn bytes(&self) -> usize {
        self.runs.capacity() * size_of::<(Us, Us)>()
    }

    /// The rests lasting at least `min_us`, exactly as a scan for that length finds them (the last always counts).
    /// O(rests).
    pub(super) fn at_least(&self, min_us: Us) -> Vec<(Us, Us)> {
        debug_assert!(
            min_us >= self.min_us,
            "{min_us} µs is shorter than the scan's"
        );
        self.runs
            .iter()
            .copied()
            .filter(|&(from, to)| to == Us::MAX || to - from >= min_us)
            .collect()
    }
}

/// The runs lasting at least `min_us`, as their first and last kept positions' times, in order. The last run always
/// counts, ending at `Us::MAX`: the raw path holds after its last position. Empty for an empty track.
///
/// O(samples); one entry per long run (a few thousand for a long take).
fn long(track: &CursorTrack, min_us: Us) -> Vec<(Us, Us)> {
    let mut out = Vec::new();
    let mut samples = track.samples(Us::MIN, Us::MAX);
    let Some((t0, x0, y0)) = samples.next() else {
        return out;
    };
    let (mut anchor, mut from, mut last) = ((x0, y0), t0, t0);
    for (t, x, y) in samples {
        let (dx, dy) = (f64::from(x - anchor.0), f64::from(y - anchor.1));
        if dx * dx + dy * dy > STILL_EPS * STILL_EPS {
            if last - from >= min_us {
                out.push((from, last));
            }
            (anchor, from) = ((x, y), t);
        }
        last = t;
    }
    out.push((from, Us::MAX));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Limits;
    use crate::format::{Line, write_line};

    #[test]
    fn one_scan_gives_each_smoothing_the_rests_its_own_scan_would() {
        // Stills of 50 ms to 600 ms between moves, so each smoothing keeps a different set.
        let mut bytes = Vec::new();
        let (mut t, mut x) = (0, 0.1_f32);
        for still_ms in [50, 120, 200, 260, 380, 600, 90, 340] {
            for _ in 0..20 {
                (t, x) = (t + 4_000, x + 0.002);
                write_line(&Line::Move { t, x, y: 0.5 }, &mut bytes);
            }
            // At rest where the move ended.
            t += still_ms * 1_000;
            write_line(&Line::Move { t, x, y: 0.5 }, &mut bytes);
        }
        let track = CursorTrack::read(&bytes[..], Limits::for_file_len(bytes.len() as u64));
        let rests = Rests::of(&track);
        for s in [Smoothing::Light, Smoothing::Medium, Smoothing::Strong] {
            let min = rest_us(s.omega().unwrap()) + STEP_US;
            assert_eq!(rests.at_least(min), long(&track, min), "{s:?}");
        }
        // Light keeps the rests from 120 ms, Strong from 342 ms (and the last, open one).
        assert_eq!(rests.at_least(shortest()).len(), 6);
        assert_eq!(rests.at_least(400_000).len(), 2);
    }
}
