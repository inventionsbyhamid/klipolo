//! Where shapes go in the cursor atlas: a quadtree of square cells, each a power of two from 16 to 256 pixels and
//! aligned to its own size. A freed cell joins its three siblings back into their parent once all four are free, so
//! letting the least recently drawn shapes go always makes room again (a shelf packer keeps its holes), and no mip
//! level up to `atlas::MAX_LOD` ever averages two cells together. Fixed bitsets, so packing never allocates after `new`.

/// The atlas's side, in pixels.
pub const SIDE: u32 = 1024;
/// The smallest cell's side.
pub const MIN_CELL: u32 = 16;
/// Levels from the whole atlas (0) to the smallest cell (6).
const LEVELS: usize = (SIDE / MIN_CELL).trailing_zeros() as usize + 1;

/// A cell: its corner in pixels and its side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub x: u32,
    pub y: u32,
    pub side: u32,
}

pub struct Packer {
    /// Per level, a bit per cell (row-major in that level's grid): free.
    free: [Vec<u64>; LEVELS],
}

impl Default for Packer {
    fn default() -> Self {
        let mut free: [Vec<u64>; LEVELS] = Default::default();
        for (level, bits) in free.iter_mut().enumerate() {
            *bits = vec![0; (cells(level) * cells(level)).div_ceil(64) as usize];
        }
        free[0][0] = 1;
        Self { free }
    }
}

/// Cells across one side at `level`.
fn cells(level: usize) -> u32 {
    1 << level
}

/// The level whose cells hold a `w` × `h` image (None past 256 on a side).
pub fn level_for(w: u32, h: u32) -> Option<usize> {
    let side = w.max(h).max(MIN_CELL).next_power_of_two();
    (side <= 256).then(|| (SIDE / side).trailing_zeros() as usize)
}

impl Packer {
    /// A free cell at `level`, split from a larger one if none is free. O(cells at that level / 64).
    pub fn take(&mut self, level: usize) -> Option<Cell> {
        let (x, y) = self.take_at(level)?;
        let side = SIDE >> level;
        Some(Cell {
            x: x * side,
            y: y * side,
            side,
        })
    }

    fn take_at(&mut self, level: usize) -> Option<(u32, u32)> {
        let n = cells(level);
        if let Some(i) = self.first_free(level) {
            self.set(level, i % n, i / n, false);
            return Some((i % n, i / n));
        }
        let (px, py) = self.take_at(level.checked_sub(1)?)?;
        for (dx, dy) in [(1, 0), (0, 1), (1, 1)] {
            self.set(level, 2 * px + dx, 2 * py + dy, true);
        }
        Some((2 * px, 2 * py))
    }

    /// Gives `cell` back, joining it with its siblings while all four are free.
    pub fn give(&mut self, cell: Cell) {
        let mut level = (SIDE / cell.side).trailing_zeros() as usize;
        let (mut x, mut y) = (cell.x / cell.side, cell.y / cell.side);
        loop {
            let (bx, by) = (x & !1, y & !1);
            let siblings = [(bx, by), (bx + 1, by), (bx, by + 1), (bx + 1, by + 1)];
            let joined = level > 0
                && siblings
                    .iter()
                    .all(|&(sx, sy)| (sx, sy) == (x, y) || self.is_free(level, sx, sy));
            if !joined {
                self.set(level, x, y, true);
                return;
            }
            for (sx, sy) in siblings {
                self.set(level, sx, sy, false);
            }
            (level, x, y) = (level - 1, x / 2, y / 2);
        }
    }

    fn first_free(&self, level: usize) -> Option<u32> {
        let words = &self.free[level];
        let w = words.iter().position(|&w| w != 0)?;
        Some(w as u32 * 64 + words[w].trailing_zeros())
    }

    fn is_free(&self, level: usize, x: u32, y: u32) -> bool {
        let i = y * cells(level) + x;
        self.free[level][(i / 64) as usize] & (1 << (i % 64)) != 0
    }

    fn set(&mut self, level: usize, x: u32, y: u32, free: bool) {
        let i = y * cells(level) + x;
        let word = &mut self.free[level][(i / 64) as usize];
        if free {
            *word |= 1 << (i % 64);
        } else {
            *word &= !(1 << (i % 64));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_fill_the_atlas_and_join_back_when_freed() {
        let mut p = Packer::default();
        let level = level_for(128, 100).unwrap();
        let all: Vec<Cell> = std::iter::from_fn(|| p.take(level)).collect();
        assert_eq!(all.len(), 64, "8 × 8 cells of 128");
        assert!(
            all.iter()
                .all(|c| c.side == 128 && c.x % 128 == 0 && c.y % 128 == 0)
        );
        assert!(p.take(level_for(16, 16).unwrap()).is_none(), "full");
        for c in all {
            p.give(c);
        }
        assert_eq!(
            p.take(0),
            Some(Cell {
                x: 0,
                y: 0,
                side: SIDE
            }),
            "joined back whole"
        );
        assert_eq!(level_for(257, 1), None);
        assert_eq!(level_for(8, 8), Some(6));
    }
}
