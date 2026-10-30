//! The drawn cursor's path: smoothed with no overshoot and no jumps, clicks landing where they happened
//! and drags exact, precomputed once per take and smoothing.
//!
//! **The filter.** Each step is the exact solution of a critically damped follower (ζ = 1, `filter`) toward the raw
//! path ahead, so a steady movement has no lag. Its impulse response is never negative, so every drawn point is a
//! weighted mean of raw positions since its segment began: the drawn cursor never leaves the range of the real one
//! (`build` has the proof).
//!
//! **Segments.** A segment starts where the raw path starts moving. It ends inside the next long still (`stills`),
//! once the raw path has been still for 4/ω, the drawn point is within `END_RESIDUAL` of it and no button is held.
//! Over those 4/ω the drawn point eases onto the raw path, so the hand-back never jumps. Outside segments, the drawn
//! path is the raw path.
//!
//! **Presses** (`weight`). Within `PIN_US` of a press the path eases onto the raw path, which it equals at the press.
//! While a button is held, once `DRAG_RAMP_US` has passed, `position` returns the raw position itself.
//!
//! **Storage and sampling.** Points are 16-bit (as the track's), every `STEP_US` over moving time only: 4 bytes a
//! step, 1.7 MB for an hour of continuous motion. `position` is a binary search, then cubic Hermite between two steps
//! clamped to their box. O(log n), no allocation. Sampling is a pure function of the take and the time, so preview,
//! scrubbing and export draw the same bits.

use klipolo_model::cursor_style::Smoothing;

use crate::track::unfix;
use crate::{CursorTrack, Us};

mod build;
mod filter;
mod room;
mod stills;
mod weight;

pub(crate) use filter::Filter;
pub use filter::step;
pub use stills::Rests;

/// The step the path is stored at: 120 Hz.
pub const STEP_US: Us = 8_333;
/// Eases to the raw path within this of a press.
pub const PIN_US: Us = 120_000;
/// After a button goes down, the path becomes the raw path over this.
pub const DRAG_RAMP_US: Us = 80_000;
/// A segment ends once the follower is this close to the raw path and the raw path is still (0.25 px at 1080p).
pub const END_RESIDUAL: f64 = 0.25 / 1080.0;
/// The most steps a path stores: 16 MB, about 9.7 hours of continuous motion. Only a pathological file reaches it (or a
/// button stuck down, which keeps one segment open); the path then eases onto the raw path, and later times draw the
/// raw path. The engine's 64 MB across its takes bounds what several such paths hold.
pub const MAX_STEPS: usize = 4 << 20;

/// A take's drawn path for one smoothing (see the module notes).
#[derive(Debug, Default)]
pub struct SmoothedPath {
    /// By start.
    segments: Vec<Seg>,
    /// Every segment's points, in order.
    xy: Vec<[u16; 2]>,
}

/// A segment: its points are `xy[first..first + len]`, one per `STEP_US` from `start`.
#[derive(Clone, Copy, Debug)]
struct Seg {
    start: Us,
    first: u32,
    len: u32,
}

impl Seg {
    /// Its last step's time.
    fn end(&self) -> Us {
        let steps = Us::from(self.len).saturating_sub(1);
        self.start.saturating_add(steps.saturating_mul(STEP_US))
    }
}

impl SmoothedPath {
    /// Off gives an empty path (sampling then gives the raw path). O(samples + steps); 4 bytes a step.
    pub fn build(track: &CursorTrack, smoothing: Smoothing) -> Self {
        match smoothing.omega() {
            Some(_) => Self::build_with(track, smoothing, &Rests::of(track)),
            None => Self::default(),
        }
    }

    /// As `build`, with the take's rests found once for all its smoothings (`Rests::of`). O(rests + steps).
    pub fn build_with(track: &CursorTrack, smoothing: Smoothing, rests: &Rests) -> Self {
        Self::capped(track, smoothing, rests, MAX_STEPS)
    }

    /// As `build_with`, stopping at `max_steps`: tests reach the cap's hand-back without 9.7 hours of motion.
    #[cfg(feature = "test-util")]
    pub fn build_capped(
        track: &CursorTrack,
        smoothing: Smoothing,
        rests: &Rests,
        max_steps: usize,
    ) -> Self {
        Self::capped(track, smoothing, rests, max_steps)
    }

    fn capped(track: &CursorTrack, smoothing: Smoothing, rests: &Rests, max_steps: usize) -> Self {
        smoothing.omega().map_or_else(Self::default, |omega| {
            build::build(track, omega, rests, max_steps)
        })
    }

    /// Steps stored (4 bytes each).
    pub fn len(&self) -> usize {
        self.xy.len()
    }

    pub fn is_empty(&self) -> bool {
        self.xy.is_empty()
    }

    /// The drawn position at `t`: the raw position while a held button's ramp is over or outside every segment;
    /// inside a segment, Hermite between steps, eased onto the raw path near presses. O(log n).
    pub fn position(&self, track: &CursorTrack, t: Us) -> Option<(f32, f32)> {
        let raw = track.position(t)?;
        let Some(seg) = self.segment(t) else {
            return Some(raw);
        };
        let w = weight::onto_raw(track, t);
        if w >= 1.0 {
            return Some(raw);
        }
        let (x, y) = self.hermite(seg, t);
        let (rx, ry) = (f64::from(raw.0), f64::from(raw.1));
        Some(((x + (rx - x) * w) as f32, (y + (ry - y) * w) as f32))
    }

    /// Per second, the central difference over one step (0 at rest). O(log n).
    pub fn velocity(&self, track: &CursorTrack, t: Us) -> (f32, f32) {
        let before = t.saturating_sub(STEP_US / 2);
        let points = (
            self.position(track, before),
            self.position(track, before.saturating_add(STEP_US)),
        );
        let (Some(a), Some(b)) = points else {
            return (0.0, 0.0);
        };
        let per_second = 1e6 / STEP_US as f32;
        ((b.0 - a.0) * per_second, (b.1 - a.1) * per_second)
    }

    /// The segment's start and end (its last step's time) around `t` (the tests' windows).
    pub fn segment_at(&self, t: Us) -> Option<(Us, Us)> {
        self.segment(t).map(|s| (s.start, s.end()))
    }

    pub fn bytes(&self) -> usize {
        self.segments.capacity() * size_of::<Seg>() + self.xy.capacity() * size_of::<[u16; 2]>()
    }

    fn segment(&self, t: Us) -> Option<&Seg> {
        let i = self.segments.partition_point(|s| s.start <= t);
        let seg = &self.segments[i.checked_sub(1)?];
        (t <= seg.end()).then_some(seg)
    }

    /// Cubic Hermite between the steps around `t`, with tangents from their neighbours (Catmull-Rom), clamped to the
    /// two steps' box.
    fn hermite(&self, seg: &Seg, t: Us) -> (f64, f64) {
        let points = &self.xy[seg.first as usize..][..seg.len as usize];
        let last = points.len() - 1;
        let since = t - seg.start;
        let k = (since / STEP_US) as usize;
        let at = |i: usize| {
            let [x, y] = points[i.min(last)];
            (unfix(f64::from(x)), unfix(f64::from(y)))
        };
        if k >= last {
            return at(last);
        }
        let s = (since % STEP_US) as f64 / STEP_US as f64;
        let (a, b, c, d) = (at(k.saturating_sub(1)), at(k), at(k + 1), at(k + 2));
        (
            hermite(s, [a.0, b.0, c.0, d.0]),
            hermite(s, [a.1, b.1, c.1, d.1]),
        )
    }
}

/// From `b` (s = 0) to `c` (s = 1), its tangents from `a` and `d`, never outside [b, c].
fn hermite(s: f64, [a, b, c, d]: [f64; 4]) -> f64 {
    let (m1, m2) = ((c - a) / 2.0, (d - b) / 2.0);
    let (s2, s3) = (s * s, s * s * s);
    let h = (2.0 * s3 - 3.0 * s2 + 1.0) * b
        + (s3 - 2.0 * s2 + s) * m1
        + (3.0 * s2 - 2.0 * s3) * c
        + (s3 - s2) * m2;
    h.clamp(b.min(c), b.max(c))
}
