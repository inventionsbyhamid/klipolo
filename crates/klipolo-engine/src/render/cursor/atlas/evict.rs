//! Making room in the cursor atlas: a free cell for a shape, and when none is free (and room may be made), the region
//! of the cell's size whose shapes were least recently drawn goes whole, never one holding a shape
//! drawn in this frame. Among equal cells that is the least recently drawn shape, whatever its take.

use std::collections::HashMap;

use super::super::pack::{Cell, SIDE, level_for};
use super::super::prepare::ShapeImage;
use super::{CursorAtlas, Entry, State};

impl CursorAtlas {
    /// A free cell for `image`; with `evict`, making room by letting regions of the cell's size go (`evict_region`).
    pub(super) fn cell_for(&mut self, image: &ShapeImage, evict: bool) -> Option<Cell> {
        let level = level_for(image.width, image.height)?;
        loop {
            if let Some(cell) = self.packer.take(level) {
                return Some(cell);
            }
            if !evict || !self.evict_region(level) {
                return None;
            }
        }
    }

    /// Lets every shape go in the region of `level`'s cell size whose most recently drawn shape is the oldest, never
    /// one holding a shape drawn this frame (one by one by age, a large cell could empty many regions).
    /// Among equal cells this is the least recently drawn shape. False when no region can go.
    fn evict_region(&mut self, level: usize) -> bool {
        let (side, frame) = (SIDE >> level, self.frame);
        let region = |c: &Cell| (c.x / side, c.y / side);
        let mut regions: HashMap<(u32, u32), (u64, bool)> = HashMap::new();
        for e in self.slots.values() {
            if let State::Resident(c, _) = &e.state {
                let r = regions.entry(region(c)).or_insert((0, false));
                *r = (r.0.max(e.used), r.1 || e.drawn == frame);
            }
        }
        let free = regions.into_iter().filter(|(_, r)| !r.1);
        let Some(oldest) = free.min_by_key(|(_, r)| r.0).map(|(k, _)| k) else {
            return false;
        };
        let inside = |e: &Entry| matches!(&e.state, State::Resident(c, _) if region(c) == oldest);
        let gone: Vec<_> = self
            .slots
            .iter()
            .filter(|(_, e)| inside(e))
            .map(|(k, _)| k.clone())
            .collect();
        for k in gone {
            if let Some(Entry {
                state: State::Resident(c, _),
                ..
            }) = self.slots.remove(&k)
            {
                self.packer.give(c);
            }
        }
        true
    }
}
