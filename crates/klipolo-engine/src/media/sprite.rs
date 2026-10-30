//! Thumbnail sprites: a file's timeline pictures, one 72 px tile per second ("slot"), 64 slots per sprite ("chunk",
//! an 8 × 8 grid). A chunk is the unit a scan finishes and writes: a stopped scan loses at most the one it was
//! filling.

use klipolo_model::time::Us;

/// One slot per second of the source.
pub const SLOT_US: Us = 1_000_000;
/// Tiles are this tall; their width follows the picture's shape (`tile_size`).
pub const TILE_H: u32 = 72;
pub const COLS: u32 = 8;
pub const ROWS: u32 = 8;
pub const PER_CHUNK: u32 = COLS * ROWS;
/// A chunk's length in source time (64 s).
pub const CHUNK_US: Us = SLOT_US * PER_CHUNK as Us;
/// Very wide or very tall pictures keep a usable tile (32:9 at most, 1:9 at least).
const MAX_TILE_W: u32 = 4 * TILE_H;
const MIN_TILE_W: u32 = 8;

/// The tile for a picture shown `w × h`: `TILE_H` tall, as wide as its shape asks (even, for 4:2:0).
pub fn tile_size(w: u32, h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (128, TILE_H);
    }
    let half = (f64::from(w) * f64::from(TILE_H) / f64::from(h) / 2.0).round() as u32;
    ((half * 2).clamp(MIN_TILE_W, MAX_TILE_W), TILE_H)
}

/// The slot showing source time `us`.
pub fn slot_of(us: Us) -> u64 {
    (us.max(0) / SLOT_US) as u64
}

/// The chunk holding `slot`.
pub fn chunk_of(slot: u64) -> u32 {
    u32::try_from(slot / u64::from(PER_CHUNK)).unwrap_or(u32::MAX)
}

/// Where `chunk` starts in source time.
pub fn chunk_start_us(chunk: u32) -> Us {
    Us::from(chunk) * CHUNK_US
}

/// How many chunks a file of `duration_us` fills.
pub fn chunks_for(duration_us: Us) -> u32 {
    u32::try_from((duration_us.max(0) as u64).div_ceil(CHUNK_US as u64)).unwrap_or(u32::MAX)
}

/// One chunk being filled: planar YUV 4:2:0, full range (JPEG), `COLS × ROWS` tiles.
pub struct Sprite {
    pub chunk: u32,
    tile: (u32, u32),
    planes: [Vec<u8>; 3],
    filled: u64,
}

impl Sprite {
    /// A grey sprite (Y 16, U and V 128) for `chunk` with `tile`-sized slots (even sides).
    pub fn new(chunk: u32, tile: (u32, u32)) -> Self {
        let (w, h) = (tile.0 * COLS, tile.1 * ROWS);
        let luma = w as usize * h as usize;
        let chroma = (w / 2) as usize * (h / 2) as usize;
        Self {
            chunk,
            tile,
            planes: [vec![16; luma], vec![128; chroma], vec![128; chroma]],
            filled: 0,
        }
    }

    pub fn tile(&self) -> (u32, u32) {
        self.tile
    }

    /// The whole sprite's size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.tile.0 * COLS, self.tile.1 * ROWS)
    }

    /// The Y, U and V planes and their strides.
    pub fn planes(&self) -> ([&[u8]; 3], [usize; 3]) {
        let w = self.size().0 as usize;
        (
            [&self.planes[0], &self.planes[1], &self.planes[2]],
            [w, w / 2, w / 2],
        )
    }

    /// No slot has a tile.
    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// Slot `i` (0–63) has a tile of its own.
    pub fn has(&self, i: u32) -> bool {
        i < PER_CHUNK && self.filled & (1 << i) != 0
    }

    /// The slot with the latest tile.
    fn last(&self) -> Option<u32> {
        (self.filled != 0).then(|| 63 - self.filled.leading_zeros())
    }

    /// Plane `p`'s tile geometry: (offset of slot `i`'s top-left byte, row stride, tile width, tile height).
    fn at(&self, p: usize, i: u32) -> (usize, usize, usize, usize) {
        let sub = if p == 0 { 1 } else { 2 };
        let (tw, th) = ((self.tile.0 / sub) as usize, (self.tile.1 / sub) as usize);
        let stride = tw * COLS as usize;
        let (col, row) = ((i % COLS) as usize, (i / COLS) as usize);
        (row * th * stride + col * tw, stride, tw, th)
    }

    /// Copies one tile (Y, U, V rows `strides` apart) into slot `slot % PER_CHUNK`.
    pub fn put(&mut self, slot: u64, planes: [&[u8]; 3], strides: [usize; 3]) {
        let i = (slot % u64::from(PER_CHUNK)) as u32;
        for (p, (src, src_stride)) in planes.into_iter().zip(strides).enumerate() {
            let (at, stride, tw, th) = self.at(p, i);
            for r in 0..th {
                let (Some(from), Some(to)) = (
                    src.get(r * src_stride..r * src_stride + tw),
                    self.planes[p].get_mut(at + r * stride..at + r * stride + tw),
                ) else {
                    break;
                };
                to.copy_from_slice(from);
            }
        }
        self.filled |= 1 << i;
    }

    fn copy_tile(&mut self, from: u32, to: u32) {
        for p in 0..3 {
            let (src, stride, tw, th) = self.at(p, from);
            let (dst, ..) = self.at(p, to);
            for r in 0..th {
                let row = src + r * stride;
                self.planes[p].copy_within(row..row + tw, dst + r * stride);
            }
        }
    }

    /// Starts this sprite with the previous chunk's last tile in its first slot: the nearest earlier keyframe shows
    /// until this chunk's first one.
    pub fn carry_from(&mut self, previous: &Sprite) {
        let Some(last) = previous.last().filter(|_| previous.tile == self.tile) else {
            return;
        };
        let mut planes = [&[][..]; 3];
        let mut strides = [0; 3];
        for p in 0..3 {
            let (at, stride, ..) = previous.at(p, last);
            planes[p] = &previous.planes[p][at..];
            strides[p] = stride;
        }
        self.put(0, planes, strides);
    }

    /// Empty slots show the nearest earlier tile; leading empty slots show the first one.
    pub fn fill_holes(&mut self) {
        let Some(first) = (0..PER_CHUNK).find(|&i| self.has(i)) else {
            return;
        };
        let mut source = first;
        for i in 0..PER_CHUNK {
            if self.has(i) {
                source = i;
            } else {
                self.copy_tile(source, i);
            }
        }
    }

    /// The luma at (`x`, `y`) of slot `slot`'s tile.
    pub fn luma_at(&self, slot: u32, x: u32, y: u32) -> u8 {
        let (at, stride, ..) = self.at(0, slot % PER_CHUNK);
        self.planes[0][at + y as usize * stride + x as usize]
    }
}
