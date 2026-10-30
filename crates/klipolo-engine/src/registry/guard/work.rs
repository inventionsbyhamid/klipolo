//! Work per pixel: operations and texture samples, as f64 clamped at `HUGE` so the arithmetic never reaches
//! infinity, and a block's cost by bounds, outside loops, and in the largest own body of a loop.

use std::ops::{Add, Mul};

/// Past this a cost is simply far over the cap: the arithmetic never reaches infinity.
pub(super) const HUGE: f64 = 1e30;

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Work {
    pub ops: f64,
    pub samples: f64,
}

pub(super) const ONE: Work = Work {
    ops: 1.0,
    samples: 0.0,
};

impl Add for Work {
    type Output = Work;
    fn add(self, o: Work) -> Work {
        Work {
            ops: (self.ops + o.ops).min(HUGE),
            samples: (self.samples + o.samples).min(HUGE),
        }
    }
}

impl Mul<f64> for Work {
    type Output = Work;
    fn mul(self, k: f64) -> Work {
        Work {
            ops: (self.ops * k).min(HUGE),
            samples: (self.samples * k).min(HUGE),
        }
    }
}

impl Work {
    pub(super) fn larger(self, o: Work) -> Work {
        Work {
            ops: self.ops.max(o.ops),
            samples: self.samples.max(o.samples),
        }
    }
}

/// A block's cost: everything by bounds; outside loops; the largest own body (nested loops excluded) of a loop in it.
#[derive(Clone, Copy, Default)]
pub(super) struct Cost {
    pub bound: Work,
    pub straight: Work,
    pub max_own: Work,
    /// Holds a loop or a sample.
    pub work: bool,
}

impl Cost {
    pub(super) fn add_straight(&mut self, w: Work) {
        self.bound = self.bound + w;
        self.straight = self.straight + w;
    }

    pub(super) fn merge(&mut self, k: Cost) {
        self.bound = self.bound + k.bound;
        self.straight = self.straight + k.straight;
        self.max_own = self.max_own.larger(k.max_own);
        self.work |= k.work;
    }
}
