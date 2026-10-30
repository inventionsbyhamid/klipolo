//! A connected part of the change grid, and the box arithmetic the derivations follow regions with.

use super::GRID;
use crate::Rect01;

/// A connected part of the grid that changed: its cell box (inclusive) and how many cells changed in it; `cells` 0 is
/// no region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub x0: u8,
    pub y0: u8,
    pub x1: u8,
    pub y1: u8,
    pub cells: u16,
}

impl Region {
    pub(crate) const NONE: Region = Region {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
        cells: 0,
    };

    /// The cell box as a part of the picture (0–1 from its top left).
    pub fn rect(&self) -> Rect01 {
        let (gw, gh) = (GRID.0 as f32, GRID.1 as f32);
        Rect01 {
            x: f32::from(self.x0) / gw,
            y: f32::from(self.y0) / gh,
            w: f32::from(self.w()) / gw,
            h: f32::from(self.h()) / gh,
        }
    }

    pub(crate) fn w(&self) -> u8 {
        self.x1 - self.x0 + 1
    }

    pub(crate) fn h(&self) -> u8 {
        self.y1 - self.y0 + 1
    }

    /// The box's area in cells (changed or not).
    pub(crate) fn area(&self) -> u32 {
        u32::from(self.w()) * u32::from(self.h())
    }

    /// The cells the two boxes share (0: none).
    pub(crate) fn overlap(&self, o: &Region) -> u32 {
        let w = self.x1.min(o.x1).checked_sub(self.x0.max(o.x0));
        let h = self.y1.min(o.y1).checked_sub(self.y0.max(o.y0));
        w.zip(h)
            .map_or(0, |(w, h)| (u32::from(w) + 1) * (u32::from(h) + 1))
    }

    /// The boxes' intersection over their union is at least `num / den`.
    pub(crate) fn iou_at_least(&self, o: &Region, num: u32, den: u32) -> bool {
        let both = self.overlap(o);
        both * den >= (self.area() + o.area() - both) * num
    }

    /// The smallest box holding both (the cell count is `self`'s).
    pub(crate) fn union(&self, o: &Region) -> Region {
        Region {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
            cells: self.cells,
        }
    }

    /// The empty cells between the boxes, across and down (0 where they touch or overlap).
    pub(crate) fn gap(&self, o: &Region) -> (u8, u8) {
        let apart = |a0: u8, a1: u8, b0: u8, b1: u8| {
            b0.saturating_sub(a1.saturating_add(1))
                .max(a0.saturating_sub(b1.saturating_add(1)))
        };
        (
            apart(self.x0, self.x1, o.x0, o.x1),
            apart(self.y0, self.y1, o.y0, o.y1),
        )
    }
}
