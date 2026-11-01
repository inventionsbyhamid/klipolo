//! The macOS cursor sampler's schedule and its shapes' sizes. Pure, built on every OS.
//! - **Passes:** 60 Hz while the cursor moved in the last second, a button is held or a down waits to be judged
//!   (the self-check's `pressedMouseButtons` edges); 10 Hz while still. The budgets: ≤ 0.1 % of a core still and
//!   ≤ 0.5 % moving (measured in the cursor's benches).
//! - **Shape looks**: `currentSystem` costs about 0.3 ms a call, so with Core
//!   Graphics' cursor seed (16 ns) a look runs whenever the seed moves, plus a safety look at 1 Hz still and 3 Hz
//!   moving. Without the seed (option 2), the public look runs at 3 Hz still and 10 Hz moving.
//! - **The area** every 250 ms (a window's bounds at 4 Hz); a **beat** once 500 ms passed with nothing sent.
//! - **The sampled fallback** polls at 240 Hz (clicks within ±4.2 ms).
//! - **Shapes:** a vector-backed one is drawn at 8× its points, a bitmap one from its largest representation, both
//!   within 256 pixels a side (`klipolo_cursor::shapes::MAX_SHAPE_PX`).

use klipolo_cursor::shapes::MAX_SHAPE_PX;

pub const MOVING_NS: u64 = 1_000_000_000;
pub const FAST_PASS_NS: u64 = 16_666_667;
pub const SLOW_PASS_NS: u64 = 100_000_000;
pub const AREA_EVERY_NS: u64 = 250_000_000;
pub const BEAT_AFTER_NS: u64 = 500_000_000;
pub const POLL_NS: u64 = 4_166_667;
/// A vector-backed shape is drawn at this many pixels a point.
pub const VECTOR_SCALE: f64 = 8.0;

/// The time between two shape looks: with the seed, its safety looks; without it, every look.
pub fn look_every_ns(seed: bool, moving: bool) -> u64 {
    match (seed, moving) {
        (true, false) => 1_000_000_000,
        (true, true) | (false, false) => 333_333_333,
        (false, true) => 100_000_000,
    }
}

/// See the module notes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pace {
    moved_at: Option<u64>,
    look_at: Option<u64>,
    area_at: Option<u64>,
}

impl Pace {
    /// The cursor moved (the pointer's place changed, or the monitor sent a move) at `now_ns`.
    pub fn moved(&mut self, now_ns: u64) {
        self.moved_at = Some(now_ns);
    }

    pub fn moving(&self, now_ns: u64) -> bool {
        self.moved_at
            .is_some_and(|at| now_ns.saturating_sub(at) < MOVING_NS)
    }

    /// Whether a scheduled look is due at `now_ns` (marked done when it is). The first pass always looks.
    pub fn look_due(&mut self, now_ns: u64, seed: bool) -> bool {
        let every = look_every_ns(seed, self.moving(now_ns));
        let due = self
            .look_at
            .is_none_or(|at| now_ns.saturating_sub(at) >= every);
        if due {
            self.look_at = Some(now_ns);
        }
        due
    }

    /// A look ran for another reason (the seed moved): the next scheduled one counts from here.
    pub fn looked(&mut self, now_ns: u64) {
        self.look_at = Some(now_ns);
    }

    /// Whether the area is due a refresh at `now_ns` (marked done when it is).
    pub fn area_due(&mut self, now_ns: u64) -> bool {
        let due = self
            .area_at
            .is_none_or(|at| now_ns.saturating_sub(at) >= AREA_EVERY_NS);
        if due {
            self.area_at = Some(now_ns);
        }
        due
    }

    /// The time to the next pass: fast while moving or `busy` (a button held, a down to judge), else slow.
    pub fn pass_ns(&self, now_ns: u64, busy: bool) -> u64 {
        if busy || self.moving(now_ns) {
            FAST_PASS_NS
        } else {
            SLOW_PASS_NS
        }
    }
}

/// A beat is due: nothing was sent since `last_sent_ns`, 500 ms or more ago.
pub fn beat_due(last_sent_ns: u64, now_ns: u64) -> bool {
    now_ns.saturating_sub(last_sent_ns) >= BEAT_AFTER_NS
}

/// A shape of `pts` points drawn at `scale` pixels a point: its pixel size, the longer side at most 256 with the
/// proportions kept, each side at least 1. A bitmap's own pixels fit with `scale` 1.
pub fn shape_px(pts: (f64, f64), scale: f64) -> (u16, u16) {
    let (w, h) = (pts.0 * scale, pts.1 * scale);
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return (1, 1);
    }
    let max = f64::from(MAX_SHAPE_PX);
    let k = (max / w.max(h)).min(1.0);
    let side = |v: f64| (v * k).round().clamp(1.0, max) as u16;
    (side(w), side(h))
}

/// Premultiplied RGBA8 (what a Core Graphics bitmap context draws) as straight alpha (what `ShapeImage` holds).
pub fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        if a == 0 || a == 255 {
            continue;
        }
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
        }
    }
}

/// AppKit's global point (bottom-left origin, y up, at the main display's corner) in Core Graphics' space (top-left,
/// y down), the main display being `main_h` points high.
pub fn top_left(bottom_left: (f64, f64), main_h: f64) -> (f64, f64) {
    (bottom_left.0, main_h - bottom_left.1)
}
