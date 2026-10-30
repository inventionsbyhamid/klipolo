//! How the crop's centre moves toward its target, one step at a time in O(1):
//! in O(1):
//! - **the wanted centre** is where the crop is while the target lies inside its central dead zone. Once the target
//!   leaves it, the crop heads for the least shift that brings it back inside. A target box larger than the dead zone
//!   is kept `safe` from the crop's edges instead, and one larger than that room is centred. It is always inside the
//!   picture;
//! - **the filter** follows it critically damped at ω (`smooth::filter`, exact over a step);
//! - **the limit** is `speed` crop widths a second, measured on the picture (a move down counts its height in widths):
//!   - while the target is in the crop's margin (within `safe` of an edge, or past it), the limit rises toward twice
//!     that, ramped across the margin's depth, so the crop catches up without a jump;
//!   - only a side the crop can still move toward the target counts (never a full-height crop's top or bottom), and
//!     only the cursor or the text being typed: never a click still to come. The catch-up fades out over the 0.3 s
//!     before a click comes due (`Look::boost`), so turning to the click never drops the speed in one step;
//!   - the step and the velocity are slowed, never sped, to it. Rounding to 16 bits can add a quantum a side between two
//!     stored steps, so that is taken off first: the stored path keeps the limit too;
//! - **the picture:** the centre is kept where the crop stays inside it, and a velocity into its edge is dropped.
//!
//! The safe edge is where the faster limit starts, not a bound: a mouse can outrun any calm crop. Holding (no target)
//! wants the centre where it is, so a moving crop slows to rest without coming back.

use super::aim::Aim;
use super::target::Look;
use super::{Crop, FollowPath, FollowRules};
use crate::smooth::Filter;

const DT: f64 = FollowPath::STEP_US as f64 / 1e6;
/// A stored side's quantum.
const QUANTUM: f64 = 1.0 / u16::MAX as f64;
/// The fastest the crop moves while its target is at its edge, or past it: twice its limit.
const CATCH_UP: f64 = 2.0;

/// The crop's centre and velocity (per second) as it follows.
pub(super) struct Motion {
    c: [f64; 2],
    v: [f64; 2],
    size: [f64; 2],
    rules: FollowRules,
    filter: Filter,
    /// The rounding allowance for the last picture shape: (aspect, allowance).
    rounding: (f64, f64),
}

impl Motion {
    /// At rest, centred on `aim` (else on the picture), inside the picture.
    pub(super) fn new(aim: Option<Aim>, crop: Crop, rules: FollowRules) -> Self {
        let mut m = Self {
            c: [0.5; 2],
            v: [0.0; 2],
            size: [crop.w, crop.h],
            rules,
            filter: Filter::new(rules.omega, DT),
            rounding: (f64::NAN, 0.0),
        };
        if let Some(a) = aim {
            m.c = [m.inside(0, a.mid(0)), m.inside(1, a.mid(1))];
        }
        m
    }

    pub(super) fn centre(&self) -> [f64; 2] {
        self.c
    }

    /// One step toward `look.aim` (None: hold), faster as `look.pull` (the cursor or the text being typed) lies deeper
    /// in the crop's margin, by `look.boost`, on a picture `aspect` times as wide as it is tall. Gives the new centre.
    pub(super) fn step(&mut self, look: Look, aspect: f64) -> [f64; 2] {
        let want = look
            .aim
            .map_or(self.c, |a| [self.wanted(0, &a), self.wanted(1, &a)]);
        let (mut next, mut v) = ([0.0; 2], [0.0; 2]);
        for i in 0..2 {
            (next[i], v[i]) = self.filter.step(self.c[i], self.v[i], want[i]);
        }
        let length = |d: [f64; 2]| d[0].hypot(d[1] / aspect);
        let catch_up = look.pull.map_or(0.0, |p| self.depth(&p)) * look.boost;
        let most = self.rules.speed * self.size[0] * (1.0 + (CATCH_UP - 1.0) * catch_up);
        if self.rounding.0 != aspect {
            self.rounding = (aspect, QUANTUM.hypot(QUANTUM / aspect));
        }
        let step_most = (most * DT - self.rounding.1).max(0.0);
        let moved = [next[0] - self.c[0], next[1] - self.c[1]];
        let len = length(moved);
        if len > step_most {
            let k = step_most / len;
            next = [self.c[0] + moved[0] * k, self.c[1] + moved[1] * k];
        }
        let speed = length(v);
        if speed > most {
            v = v.map(|v| v * most / speed);
        }
        for i in 0..2 {
            let kept = self.inside(i, next[i]);
            if kept != next[i] {
                (next[i], v[i]) = (kept, 0.0);
            }
        }
        (self.c, self.v) = (next, v);
        self.c
    }

    /// Along axis `i`: the least shift that brings the target inside the dead zone (none while it is), else keeps it
    /// `safe` from both edges; a target larger than that room is centred. Inside the picture. The dead zone never
    /// reaches past the safe room.
    fn wanted(&self, i: usize, a: &Aim) -> f64 {
        let room = (0.5 - self.rules.safe) * self.size[i];
        let dead = (self.rules.dead_zone * self.size[i] / 2.0).min(room);
        let within = |reach: f64| {
            let (from, to) = (a.hi[i] - reach, a.lo[i] + reach);
            (from <= to).then(|| self.c[i].clamp(from, to))
        };
        let want = within(dead)
            .or_else(|| within(room))
            .unwrap_or_else(|| a.mid(i));
        self.inside(i, want)
    }

    /// How deep `p` lies in the crop's margin: 0 inside the safe room, 1 at the crop's edge or past it. The deepest of
    /// the sides the crop can still move toward it.
    fn depth(&self, p: &Aim) -> f64 {
        let side = |i: usize| {
            let (lo, hi) = self.bounds(i);
            let (half, room) = (self.size[i] / 2.0, (0.5 - self.rules.safe) * self.size[i]);
            let (ahead, behind) = (p.hi[i] - self.c[i], self.c[i] - p.lo[i]);
            let (far, free) = if ahead >= behind {
                (ahead, self.c[i] < hi)
            } else {
                (behind, self.c[i] > lo)
            };
            if free && half > room {
                ((far - room) / (half - room)).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        side(0).max(side(1))
    }

    /// Where the centre may be along axis `i`, so the crop stays inside the picture.
    fn bounds(&self, i: usize) -> (f64, f64) {
        let half = self.size[i] / 2.0;
        (half.min(0.5), (1.0 - half).max(0.5))
    }

    /// `x` kept where the crop stays inside the picture along axis `i`.
    fn inside(&self, i: usize, x: f64) -> f64 {
        let (lo, hi) = self.bounds(i);
        x.clamp(lo, hi)
    }
}
