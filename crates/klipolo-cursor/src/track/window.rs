//! Window helpers over the raw path: the mean over a window ending at a time, and the place held at a time.
//!
//! **On time, not on samples.** The recorder thins moves a straight line reproduces, so a
//! track can hold two places for a whole glide. The mean is therefore taken over the raw path itself (linear between
//! kept places, held after the last), weighted by time: a thinned track gives what the dense one gave, within the
//! thinning's half a 16-bit step.

use super::CursorTrack;
use crate::Us;

impl CursorTrack {
    /// The raw path's mean over [t − window, t] (from the first kept position when the window starts before it), by
    /// time. None before the first position. O(log n + the window's kept positions), no allocation.
    pub fn smoothed(&self, t: Us, window_us: Us) -> Option<(f32, f32)> {
        let (first, _) = self.span()?;
        if t < first {
            return None;
        }
        let from = t.saturating_sub(window_us.max(0)).max(first);
        if from >= t {
            return self.position(t);
        }
        // The last kept position at or before `from` (there is one: `from` is at or after the first).
        let mut i = self.after(from) - 1;
        let (mut at, mut sum) = (from, (0.0, 0.0));
        // Each pass covers the path up to the next kept position or `t`: at most the window's positions plus one.
        while at < t {
            let next = self.t.get(i + 1).map_or(t, |&k| self.time(k).min(t));
            // The path is linear over [at, next], so its mean there is its place at the middle.
            let (x, y) = self.linear(i, (at as f64 + next as f64) / 2.0);
            let w = (next - at) as f64;
            sum = (sum.0 + x * w, sum.1 + y * w);
            (at, i) = (next, i + 1);
        }
        let span = (t - from) as f64;
        Some(((sum.0 / span) as f32, (sum.1 / span) as f32))
    }

    /// The last kept position at or before `t`.
    pub fn held(&self, t: Us) -> Option<(f32, f32)> {
        let i = self.after(t).checked_sub(1)?;
        let (x, y) = self.place(i);
        Some((x as f32, y as f32))
    }

    /// The raw path at `t` (in µs, fractional) between kept position `i` and the next; held after the last.
    fn linear(&self, i: usize, t: f64) -> (f64, f64) {
        let (x0, y0) = self.place(i);
        let Some(&next) = self.t.get(i + 1) else {
            return (x0, y0);
        };
        let (a, b) = (self.time(self.t[i]) as f64, self.time(next) as f64);
        let f = ((t - a) / (b - a)).clamp(0.0, 1.0);
        let (x1, y1) = self.place(i + 1);
        (x0 + (x1 - x0) * f, y0 + (y1 - y0) * f)
    }
}
