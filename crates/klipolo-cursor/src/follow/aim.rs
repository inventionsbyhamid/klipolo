//! A part of the picture the follow-crop looks at, and the box arithmetic its targets, typed parts and motion share.

use crate::Rect01;

/// A part of the picture the crop looks at, 0–1 from its top left, both ways: a point has no size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Aim {
    pub(super) lo: [f64; 2],
    pub(super) hi: [f64; 2],
}

impl Aim {
    /// A point inside the picture; None outside it or not a number.
    pub(super) fn point(x: f32, y: f32) -> Option<Self> {
        let (x, y) = (f64::from(x), f64::from(y));
        let inside = |v: f64| (0.0..=1.0).contains(&v);
        (inside(x) && inside(y)).then_some(Self {
            lo: [x, y],
            hi: [x, y],
        })
    }

    /// The part of `r` inside the picture; None when none of it is, or it isn't a box of numbers.
    pub(super) fn of_rect(r: Rect01) -> Option<Self> {
        let [x, y, w, h] = [r.x, r.y, r.w, r.h].map(f64::from);
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || w < 0.0 || h < 0.0 {
            return None;
        }
        let (lo, hi) = (
            [x.max(0.0), y.max(0.0)],
            [(x + w).min(1.0), (y + h).min(1.0)],
        );
        (lo[0] <= hi[0] && lo[1] <= hi[1]).then_some(Self { lo, hi })
    }

    pub(super) fn mid(&self, i: usize) -> f64 {
        (self.lo[i] + self.hi[i]) / 2.0
    }

    /// No larger than `limit` either way.
    pub(super) fn fits(&self, limit: [f64; 2]) -> bool {
        (0..2).all(|i| self.hi[i] - self.lo[i] <= limit[i])
    }

    /// The smallest box holding both.
    pub(super) fn union(&self, o: &Self) -> Self {
        Self {
            lo: [self.lo[0].min(o.lo[0]), self.lo[1].min(o.lo[1])],
            hi: [self.hi[0].max(o.hi[0]), self.hi[1].max(o.hi[1])],
        }
    }

    /// The boxes overlap or touch.
    pub(super) fn meets(&self, o: &Self) -> bool {
        (0..2).all(|i| self.lo[i] <= o.hi[i] && o.lo[i] <= self.hi[i])
    }

    /// The boxes share some area, as cell boxes overlap (touching isn't enough).
    pub(super) fn overlaps(&self, o: &Self) -> bool {
        (0..2).all(|i| self.lo[i] < o.hi[i] && o.lo[i] < self.hi[i])
    }

    /// `o` lies within one of its own sizes of this box, both ways (as `activity::typing` joins regions).
    pub(super) fn beside(&self, o: &Self) -> bool {
        let apart = |i: usize| (o.lo[i] - self.hi[i]).max(self.lo[i] - o.hi[i]);
        (0..2).all(|i| apart(i) <= o.hi[i] - o.lo[i])
    }

    /// How far apart the boxes are, the farther way; 0 where they meet.
    pub(super) fn gap(&self, o: &Self) -> f64 {
        let apart = |i: usize| (o.lo[i] - self.hi[i]).max(self.lo[i] - o.hi[i]).max(0.0);
        apart(0).max(apart(1))
    }

    pub(super) fn holds(&self, x: f64, y: f64) -> bool {
        self.meets(&Self {
            lo: [x, y],
            hi: [x, y],
        })
    }
}
