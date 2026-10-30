//! What a track holds in memory, and its buffers trimmed to fit once read.

use super::CursorTrack;
use crate::shapes::ShapeDef;

fn of<T>(v: &Vec<T>) -> usize {
    v.capacity() * size_of::<T>()
}

impl CursorTrack {
    /// Every buffer; the positions alone are `8 × len()`.
    pub fn bytes(&self) -> usize {
        let defs: usize = self.defs.iter().map(ShapeDef::bytes).sum();
        of(&self.t)
            + of(&self.xy)
            + of(&self.presses)
            + of(&self.primary)
            + of(&self.held)
            + of(&self.shapes)
            + defs
            + of(&self.hidden)
            + of(&self.areas)
            + of(&self.clicks_from)
    }

    pub fn position_bytes(&self) -> usize {
        self.t.len() * (size_of::<u32>() + size_of::<[u16; 2]>())
    }

    /// Each buffer shrunk to its length (`read`'s last step).
    pub(super) fn trim(&mut self) {
        self.t.shrink_to_fit();
        self.xy.shrink_to_fit();
        self.presses.shrink_to_fit();
        self.primary.shrink_to_fit();
        self.held.shrink_to_fit();
        self.shapes.shrink_to_fit();
        self.defs.shrink_to_fit();
        self.hidden.shrink_to_fit();
        self.areas.shrink_to_fit();
        self.clicks_from.shrink_to_fit();
    }
}
