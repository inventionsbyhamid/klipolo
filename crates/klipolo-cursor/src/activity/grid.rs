//! The change grid: each 128 × 72 luma frame box-filtered 2 × 2 into 64 × 36 cells, the cells whose mean moved by at
//! least `CHANGE` since the frame before, and the two largest 8-connected regions of them (an iterative flood fill).
//!
//! **Cost:** 9,216 reads for the filter, then 2,304 cells compared and each changed cell filled once (its 8
//! neighbours, with no bounds checks: the marks have a border that never changes). No allocation after `new`: the
//! stack holds each cell at most once, so it never grows past the 2,304 it starts with. Measured in release on
//! a 2013 Xeon server: about 6 µs a frame of a busy screen, and 30 µs when every cell changes (a whole-screen
//! flash), against the 25 ms a frame a pass at 4× real time has.

use super::{CHANGE, FRAME, GRID, Region, Tick};

const W: usize = GRID.0;
const H: usize = GRID.1;
const CELLS: usize = W * H;
/// The marks' row length: the grid's, plus a border cell at each end (and a border row above and below).
const PW: usize = W + 2;
const PADDED: usize = PW * (H + 2);
/// A bordered cell's 8 neighbours, as steps from it.
const AROUND: [isize; 8] = {
    let row = PW as isize;
    [-row - 1, -row, -row + 1, -1, 1, row - 1, row, row + 1]
};

/// Turns successive frames into ticks. Every buffer is allocated in `new`; `push` allocates nothing.
pub struct Differ {
    prev: Box<[u8; CELLS]>,
    cur: Box<[u8; CELLS]>,
    /// Per cell, bordered: 0 unchanged (the border always), 1 changed and not yet in a region, 2 in a region.
    mark: Box<[u8; PADDED]>,
    stack: Vec<u16>,
    primed: bool,
}

impl Default for Differ {
    fn default() -> Self {
        Self::new()
    }
}

impl Differ {
    pub fn new() -> Self {
        Self {
            prev: Box::new([0; CELLS]),
            cur: Box::new([0; CELLS]),
            mark: Box::new([0; PADDED]),
            stack: Vec::with_capacity(CELLS),
            primed: false,
        }
    }

    /// The tick between the previous frame and `luma` (None for the first frame after `new` or `reset`).
    pub fn push(&mut self, luma: &[u8; FRAME.0 * FRAME.1]) -> Option<Tick> {
        box_filter(luma, &mut self.cur);
        let tick = self.primed.then(|| self.compare());
        std::mem::swap(&mut self.prev, &mut self.cur);
        self.primed = true;
        tick
    }

    /// Forget the previous frame (a seek, or a new file): the next push only primes.
    pub fn reset(&mut self) {
        self.primed = false;
    }

    fn compare(&mut self) -> Tick {
        let mut changed = 0u16;
        let rows = self.cur.chunks_exact(W).zip(self.prev.chunks_exact(W));
        for (y, (now, before)) in rows.enumerate() {
            let marks = &mut self.mark[(y + 1) * PW + 1..][..W];
            for (m, (&now, &before)) in marks.iter_mut().zip(now.iter().zip(before)) {
                let moved = now.abs_diff(before) >= CHANGE;
                *m = u8::from(moved);
                changed += u16::from(moved);
            }
        }
        let mut top = [Region::default(); 2];
        if changed > 0 {
            for start in PW + 1..PADDED - PW - 1 {
                if self.mark[start] == 1 {
                    let r = self.fill(start);
                    // The two largest; a tie keeps the one found first (raster order), so ticks are deterministic.
                    if r.cells > top[0].cells {
                        top = [r, top[0]];
                    } else if r.cells > top[1].cells {
                        top[1] = r;
                    }
                }
            }
        }
        Tick { changed, top }
    }

    /// The 8-connected region of changed cells around `start` (a bordered index), each marked as taken.
    fn fill(&mut self, start: usize) -> Region {
        let mut r = Region {
            x0: u8::MAX,
            y0: u8::MAX,
            x1: 0,
            y1: 0,
            cells: 0,
        };
        self.mark[start] = 2;
        self.stack.push(start as u16);
        while let Some(p) = self.stack.pop() {
            let p = usize::from(p);
            let (x, y) = ((p % PW - 1) as u8, (p / PW - 1) as u8);
            r.x0 = r.x0.min(x);
            r.x1 = r.x1.max(x);
            r.y0 = r.y0.min(y);
            r.y1 = r.y1.max(y);
            r.cells += 1;
            for n in AROUND.map(|d| p.wrapping_add_signed(d)) {
                if self.mark[n] == 1 {
                    self.mark[n] = 2;
                    self.stack.push(n as u16);
                }
            }
        }
        r
    }
}

/// Each 2 × 2 block of `luma` averaged (rounded) into its cell.
fn box_filter(luma: &[u8; FRAME.0 * FRAME.1], out: &mut [u8; CELLS]) {
    for (rows, cells) in luma.chunks_exact(2 * FRAME.0).zip(out.chunks_exact_mut(W)) {
        let (top, bottom) = rows.split_at(FRAME.0);
        let pairs = top.chunks_exact(2).zip(bottom.chunks_exact(2));
        for (cell, (a, b)) in cells.iter_mut().zip(pairs) {
            let sum = u16::from(a[0]) + u16::from(a[1]) + u16::from(b[0]) + u16::from(b[1]);
            *cell = ((sum + 2) / 4) as u8;
        }
    }
}
