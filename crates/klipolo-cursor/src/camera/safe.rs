//! The safe zone: a view less `safe` of its width on each side, except a side on the frame's
//! own edge, which reaches that edge. Nothing lies past the frame to be cut off, and no view up to 3× holds a point
//! within 5 % of the frame's edge 15 % inside its own: so a click in the menu bar, the dock or a sidebar is framed
//! against the edge rather than never framed.
//!
//! On each axis, the centres at which a view holds `[lo, hi]` in its safe zone are one interval: the strict one,
//! clamped to where the view stays inside the frame (the clamp is exactly what lets a side on the edge reach it).
//! With `safe` 0 it is where the view merely shows them: a shot must also show the actions dropped around it (`seen`).

use super::View;
use crate::Rect01;

impl View {
    /// The whole frame.
    pub const FULL: View = View {
        cx: 0.5,
        cy: 0.5,
        scale: 1.0,
    };

    /// The part of the frame it shows, kept inside the frame.
    pub fn rect(&self) -> Rect01 {
        let w = 1.0 / self.scale.max(1.0);
        let at = |c: f64| (c - w / 2.0).clamp(0.0, 1.0 - w) as f32;
        let (x, y, w) = (at(self.cx), at(self.cy), w as f32);
        Rect01 { x, y, w, h: w }
    }
}

/// A box of the frame, `[x0, y0, x1, y1]` (0–1 from the top left).
pub(crate) type Box4 = [f64; 4];

/// No box at all: it holds nothing and asks for nothing.
pub(crate) const NONE: Box4 = [
    f64::INFINITY,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NEG_INFINITY,
];

/// `r` as a box inside the frame. A coordinate with no number (NaN, ±∞) reads as the frame's centre and a size as
/// none, so `plan`, which is public, never panics on one.
pub(crate) fn box_of(r: Rect01) -> Box4 {
    let num = |v: f32, or: f64| if v.is_finite() { v as f64 } else { or };
    let (x, y) = (num(r.x, 0.5), num(r.y, 0.5));
    let (w, h) = (num(r.w, 0.0).max(0.0), num(r.h, 0.0).max(0.0));
    let (x0, y0) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
    [x0, y0, (x + w).clamp(x0, 1.0), (y + h).clamp(y0, 1.0)]
}

pub(crate) fn union(a: Box4, b: Box4) -> Box4 {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// The centres, on one axis, at which a view of scale `s` (≥ 1) holds `[lo, hi]` in its safe zone: empty when the
/// first is past the second.
pub(crate) fn centres(lo: f64, hi: f64, s: f64, safe: f64) -> (f64, f64) {
    let half = 0.5 / s;
    let reach = half * (1.0 - 2.0 * safe);
    let inside = |c: f64| c.max(half).min(1.0 - half);
    (inside(hi - reach), inside(lo + reach))
}

/// `v` shows the point (x, y), anywhere in it: the same arithmetic as the glide's regions.
pub(crate) fn shows(v: View, x: f64, y: f64) -> bool {
    let ((x0, x1), (y0, y1)) = (centres(x, x, v.scale, 0.0), centres(y, y, v.scale, 0.0));
    x0 <= v.cx && v.cx <= x1 && y0 <= v.cy && v.cy <= y1
}

/// What a shot's view must hold: its `events` inside the safe zone, and the dropped actions around it (`seen`)
/// anywhere in it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Must {
    pub events: Box4,
    pub seen: Box4,
}

impl Must {
    pub(crate) fn of(events: Box4) -> Self {
        Self { events, seen: NONE }
    }

    /// The centres (x0, x1, y0, y1) at which a view of scale `s` holds both; empty on an axis when its first is past
    /// its second.
    fn range(&self, s: f64, safe: f64) -> [f64; 4] {
        let (e, n) = (self.events, self.seen);
        let mut out = [0.0; 4];
        for axis in 0..2 {
            let (a0, a1) = centres(e[axis], e[axis + 2], s, safe);
            let (b0, b1) = match n[axis] <= n[axis + 2] {
                true => centres(n[axis], n[axis + 2], s, 0.0),
                false => (f64::NEG_INFINITY, f64::INFINITY),
            };
            (out[2 * axis], out[2 * axis + 1]) = (a0.max(b0), a1.min(b1));
        }
        out
    }

    pub(crate) fn fits(&self, s: f64, safe: f64) -> bool {
        let r = self.range(s, safe);
        r[0] <= r[1] && r[2] <= r[3]
    }

    /// The view of scale `s` nearest to centring the events that holds both (when they fit at `s`).
    pub(crate) fn view(&self, s: f64, safe: f64) -> View {
        let (r, e) = (self.range(s, safe), self.events);
        let cx = ((e[0] + e[2]) / 2.0).max(r[0]).min(r[1]);
        View {
            cx,
            cy: ((e[1] + e[3]) / 2.0).max(r[2]).min(r[3]),
            scale: s,
        }
    }

    /// The largest scale in `[lo, hi]` at which both fit, or None when they don't even at `lo`. Fitting only gets
    /// easier as the scale falls (a wider view's zones are wider, and the frame's edges only relax them), so a
    /// bisection finds it.
    pub(crate) fn largest(&self, lo: f64, hi: f64, safe: f64) -> Option<f64> {
        if !self.fits(lo, safe) {
            return None;
        }
        if self.fits(hi, safe) {
            return Some(hi);
        }
        let (mut l, mut h) = (lo, hi);
        for _ in 0..40 {
            let m = 0.5 * (l + h);
            if self.fits(m, safe) {
                l = m;
            } else {
                h = m;
            }
        }
        Some(l)
    }
}
