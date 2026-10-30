//! The visual activity track: where a screen take's picture changed, every 100 ms, never what
//! it showed. The engine's background pass decodes each frame at 128 × 72 luma and pushes it through `Differ`
//! (`grid`), which gives a 14-byte `Tick`: how many cells of a 64 × 36 grid changed, and the two largest connected
//! regions. The ticks are kept in 60-second chunks with checksums (`file`), so a stopped pass resumes at the chunk it
//! lost. `derive` reads them as typing (a focus for the camera and the follow-crop), playing media (their exclusion)
//! and quiet (`ActivityTrack::quiet_spans`, for idle stretches and dead time).
//!
//! **Clocks.** Each screen file of a take (a run: the first, then one per heal, step-down or resume) has its own track.
//! Tick `i` is the change from the file's frame at `i × TICK_US` to the one at `(i + 1) × TICK_US`, on the **file's
//! own clock**, and so is everything derived from it: `tick_at`, `quiet_spans`, `typing` and `media`. The file starts
//! its `SourceRun::offset_us` into the take, so the caller adds that offset to each span when it fills the take's
//! facts (on the recording clock); runs never overlap, so the runs' lists, concatenated, stay sorted and disjoint.
//! `typing` takes the offset itself, to read the cursor's still runs (on the recording clock) at the right times.

use crate::Us;

mod derive;
mod file;
mod grid;
mod region;

pub(crate) use derive::typed_region;
pub use derive::{media, players, typing};
pub use file::{MAGIC, MAX_DURATION_US, decode, encode_chunk, header, missing, stored_duration};
pub use grid::Differ;
pub use region::Region;

/// The frames the pass decodes: 128 × 72 luma.
pub const FRAME: (usize, usize) = (128, 72);
/// The change grid: 2 × 2 pixels a cell.
pub const GRID: (usize, usize) = (64, 36);
pub const TICK_US: Us = 100_000;
/// Ticks a chunk: 60 s.
pub const CHUNK_TICKS: u32 = 600;
/// A cell is changed when its mean luma moved by at least this (of 255). Tuned on real takes.
pub const CHANGE: u8 = 6;
/// A tick's size, in memory and in the file.
pub const TICK_BYTES: usize = 14;
/// A tick with at most this many changed cells is quiet.
pub const QUIET_CELLS: u16 = 2;

const CHUNK_US: Us = CHUNK_TICKS as Us * TICK_US;

/// What changed over one 100 ms tick: the changed cells, and the two largest 8-connected regions of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tick {
    pub changed: u16,
    pub top: [Region; 2],
}

impl Tick {
    /// A tick the pass hasn't made (or couldn't: a frame the decoder didn't give).
    pub const UNKNOWN: Tick = Tick {
        changed: u16::MAX,
        top: [Region::NONE; 2],
    };

    pub fn is_known(&self) -> bool {
        self.changed != u16::MAX
    }

    /// At most `QUIET_CELLS` changed (never an unknown tick).
    pub fn quiet(&self) -> bool {
        self.changed <= QUIET_CELLS
    }
}

/// One video file's activity: a tick every 100 ms over its duration, unknown where its chunk isn't made yet.
/// 14 bytes a tick (≤ 0.5 MB an hour), allocated once by `decode`.
#[derive(Debug, Default)]
pub struct ActivityTrack {
    duration_us: Us,
    ticks: Vec<Tick>,
    /// A flag a chunk: made (read with a matching checksum).
    made: Vec<bool>,
    /// The end of the last whole chunk read: where the pass appends (`file` notes).
    append_at: usize,
}

impl ActivityTrack {
    /// The video's duration the track covers.
    pub fn duration_us(&self) -> Us {
        self.duration_us
    }

    /// Where a resumed pass truncates the file before it appends: the end of the last whole chunk read, the header's
    /// end with none, and 0 when the file must be written again from its header (`file` notes).
    pub fn append_at(&self) -> usize {
        self.append_at
    }

    /// The tick around `t` (the file's clock). None where unknown. O(1).
    pub fn tick_at(&self, t: Us) -> Option<Tick> {
        let i = usize::try_from(t.div_euclid(TICK_US)).ok()?;
        self.ticks.get(i).copied().filter(Tick::is_known)
    }

    /// Every chunk is made up to here, on the file's own clock (a later screen file of a take adds its run's offset):
    /// the follow and camera keys carry it. A tick inside a made chunk can still be unknown (a frame the
    /// decoder couldn't give): it is never made again.
    pub fn complete_through(&self) -> Us {
        let made = self.made.iter().take_while(|&&m| m).count() as Us;
        made.saturating_mul(CHUNK_US).min(self.duration_us)
    }

    /// Every chunk is made (an empty track, of a 0 µs video, is).
    pub fn is_complete(&self) -> bool {
        self.made.iter().all(|&m| m)
    }

    /// The stretches of known quiet ticks lasting at least `min_us`, sorted and disjoint, on the file's clock. O(ticks).
    pub fn quiet_spans(&self, min_us: Us) -> Vec<(Us, Us)> {
        let mut out = Vec::new();
        let mut from = None;
        let ends = self.ticks.iter().map(|t| t.is_known() && t.quiet());
        for (i, quiet) in ends.chain([false]).enumerate() {
            match (quiet, from) {
                (true, None) => from = Some(i),
                (false, Some(f)) => {
                    let span = (self.at(f), self.at(i));
                    if span.1 - span.0 >= min_us {
                        out.push(span);
                    }
                    from = None;
                }
                _ => {}
            }
        }
        out
    }

    /// Its memory, in bytes.
    pub fn bytes(&self) -> usize {
        size_of::<Self>() + self.ticks.capacity() * size_of::<Tick>() + self.made.capacity()
    }

    /// Where tick `i` starts (the end, for the tick after the last).
    fn at(&self, i: usize) -> Us {
        (i as Us * TICK_US).min(self.duration_us)
    }
}
