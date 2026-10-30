//! The raw position at times that only move forward, without a search each time: a smoothing build reads the raw path
//! at three such times a step.

use super::CursorTrack;
use crate::Us;

/// A cursor over a track's kept positions. `at` gives exactly what `CursorTrack::position` gives.
pub(crate) struct Walk<'a> {
    track: &'a CursorTrack,
    /// The first kept position later than the last time asked.
    after: usize,
}

impl<'a> Walk<'a> {
    pub(crate) fn new(track: &'a CursorTrack, from: Us) -> Self {
        Self {
            track,
            after: track.after(from),
        }
    }

    /// The raw position at `t`, no earlier than the last time asked; `passed` sees each kept position stepped over, in
    /// order. Amortised O(1).
    pub(crate) fn to(&mut self, t: Us, mut passed: impl FnMut((f32, f32))) -> Option<(f32, f32)> {
        let track = self.track;
        debug_assert!(
            self.after == 0 || track.time(track.t[self.after - 1]) <= t,
            "walked back to {t}"
        );
        while self.after < track.t.len() && track.time(track.t[self.after]) <= t {
            passed(track.point(self.after));
            self.after += 1;
        }
        track.between(self.after, t)
    }

    pub(crate) fn at(&mut self, t: Us) -> Option<(f32, f32)> {
        self.to(t, |_| {})
    }
}

impl CursorTrack {
    /// The first kept position later than `t`. O(log n).
    pub(super) fn after(&self, t: Us) -> usize {
        self.t.partition_point(|&k| self.time(k) <= t)
    }

    /// The raw position at `t`, given `after`, the first kept position later than it: linear between kept positions,
    /// held after the last, None before the first.
    pub(super) fn between(&self, after: usize, t: Us) -> Option<(f32, f32)> {
        let i = after.checked_sub(1)?;
        let (x0, y0) = self.place(i);
        if after == self.t.len() {
            return Some((x0 as f32, y0 as f32));
        }
        // `t` is before the next position and at or after this one, and kept times strictly increase.
        let (a, b) = (self.time(self.t[i]), self.time(self.t[after]));
        let f = (t - a) as f64 / (b - a) as f64;
        let (x1, y1) = self.place(after);
        Some(((x0 + (x1 - x0) * f) as f32, (y0 + (y1 - y0) * f) as f32))
    }

    /// A kept position, as `position` gives it at its own time.
    fn point(&self, i: usize) -> (f32, f32) {
        let (x, y) = self.place(i);
        (x as f32, y as f32)
    }
}
