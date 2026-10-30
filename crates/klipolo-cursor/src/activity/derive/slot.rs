//! A region followed from tick to tick: its box, which of the last ticks it changed in (a bit ring), and the stretch
//! where it changed often enough. Every step is O(1) and allocates nothing.

use crate::activity::Region;

/// A stretch a slot found: ticks `[from, to)`, from its first change to its last, with the slot's box.
#[derive(Clone, Copy, Debug)]
pub(super) struct Found {
    pub(super) from: u32,
    pub(super) to: u32,
    pub(super) rect: Region,
}

/// The stretch a slot is in: its first and last changing ticks, and the last tick its window was dense.
#[derive(Clone, Copy)]
struct Stretch {
    first: u32,
    last: u32,
    dense_at: u32,
}

#[derive(Clone, Copy)]
pub(super) struct Slot {
    /// None: free.
    pub(super) rect: Option<Region>,
    /// Bit k: the region changed k ticks ago (inside the window).
    ring: u128,
    open: Option<Stretch>,
}

impl Slot {
    pub(super) const FREE: Slot = Slot {
        rect: None,
        ring: 0,
        open: None,
    };

    /// A new tick: the window (at most 127 ticks) slides on.
    pub(super) fn shift(&mut self, window: u32) {
        self.ring = (self.ring << 1) & ((1u128 << window) - 1);
    }

    /// The region changed this tick; `grow` widens the box to hold `r`.
    pub(super) fn hit(&mut self, r: Region, grow: bool) {
        self.ring |= 1;
        if grow {
            self.rect = Some(self.rect.map_or(r, |b| b.union(&r)));
        }
    }

    /// `r` starts being followed in a free slot, else in place of the one that changed least lately (never one in a
    /// stretch; when every slot is, `r` isn't followed).
    pub(super) fn seed(slots: &mut [Slot], r: Region) {
        let idle = slots.iter_mut().filter(|s| s.open.is_none());
        if let Some(s) = idle.min_by_key(|s| (s.rect.is_some(), s.ring.count_ones())) {
            *s = Slot {
                rect: Some(r),
                ring: 1,
                open: None,
            };
        }
    }

    /// After the tick's hits, at tick `i`: the stretch goes on while the window is dense (`need` of the last `window`
    /// ticks changed), and is kept once no later window can touch it, if it lasted `min` ticks.
    pub(super) fn advance(
        &mut self,
        i: u32,
        (need, window): (u32, u32),
        min: u32,
        found: &mut Vec<Found>,
    ) {
        if self.open.is_some_and(|s| i > s.dense_at + window) {
            self.finish(found, min);
        }
        if self.ring.count_ones() >= need {
            let last = i - self.ring.trailing_zeros();
            match &mut self.open {
                Some(s) => (s.last, s.dense_at) = (last, i),
                None => {
                    let first = i - (127 - self.ring.leading_zeros());
                    self.open = Some(Stretch {
                        first,
                        last,
                        dense_at: i,
                    });
                }
            }
        }
        if self.ring == 0 && self.open.is_none() {
            self.rect = None;
        }
    }

    /// The stretch so far is kept (if it lasted `min` ticks), and the slot is freed.
    pub(super) fn end(&mut self, found: &mut Vec<Found>, min: u32) {
        self.finish(found, min);
        *self = Slot::FREE;
    }

    fn finish(&mut self, found: &mut Vec<Found>, min: u32) {
        let (Some(s), Some(rect)) = (self.open.take(), self.rect) else {
            return;
        };
        if s.last + 1 - s.first >= min {
            found.push(Found {
                from: s.first,
                to: s.last + 1,
                rect,
            });
        }
    }
}
