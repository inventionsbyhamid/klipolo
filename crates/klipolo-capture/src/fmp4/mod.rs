//! Video runs as one growing fragmented MP4 each (OBS's Hybrid MP4 approach).
//!
//! - **Head** (`Fmp4Writer::head`): `ftyp` and a `moov` with `mvex`, written and synced before any media.
//! - **Fragments** (`Fmp4Writer::push`): a `moof`+`mdat` pair every 0.5 s (`FRAGMENT_US`), whatever the keyframes do
//!   (the encoder's GOP is 1 s). Each pair stands alone, so whatever reached `write()` survives a process kill, and a
//!   walk keeps every complete pair before damage (`read::walk`).
//! - **The soft remux at Stop** (`finalize`): a trailing `moov` with the whole sample table is written typed `free` and
//!   synced, then four-byte flips turn the head `moov` and every `moof` into `free` and the trailing box into `moov`.
//!   Nothing is copied; Stop needs only the sample table's bytes (about 2 MB an hour). The flips are labels, never
//!   deletions: the walk still reads a flipped `moof` and a flipped head, so a remux cut short by a crash finishes on a
//!   re-run, and every step is idempotent.
//!
//! Salvage follows the design of Cap's recovery (`crates/recording/src/recovery.rs` and `crates/enc-ffmpeg/src/mux/
//! fragmented_mp4.rs`, `tail_is_complete`, at d327051: "a complete `moof`+`mdat` pair"; design only, no code).
//! Ours keeps every complete pair of a truncated file instead of refusing it.
//!
//! Times in the files are 90 kHz ticks (`TIMESCALE`), each sample's start rounded from its absolute µs, so durations
//! never drift.

pub(crate) mod boxes;
pub mod finalize;
mod moov;
mod parse;
pub mod read;
mod write;

pub use write::{Fmp4Writer, Fragment};

use klipolo_model::time::Us;

/// The video track's timescale: 90 kHz.
pub const TIMESCALE: u32 = 90_000;
/// A fragment closes once it holds this much media.
pub const FRAGMENT_US: Us = 500_000;

/// Where every fragment's samples sit, enough to write the remux's sample table. Built by the writer as it goes, or by
/// a walk over a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub fragments: Vec<FragIndex>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragIndex {
    /// The `moof`'s first byte in the file.
    pub moof_at: u64,
    /// The first sample's first byte (inside the following `mdat`); the samples follow each other from there.
    pub data_at: u64,
    /// The first sample's decode time, in the track's timescale.
    pub first_dts: u64,
    pub samples: Vec<SampleIndex>,
}

/// 12 bytes in memory (padding): an hour at 60 fps is about 2.6 MB plus 48 bytes for each of its 7,200 fragments
/// (2.9 MB); 6 hours is 17.6 MB, or about 33 MB at 120 fps (`finalize` adds the tail at Stop).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleIndex {
    pub size: u32,
    /// In the track's timescale.
    pub duration: u32,
    pub key: bool,
}

impl Index {
    /// The media it holds: every sample's duration, at `timescale` ticks a second, in µs (rounded). What a run's file
    /// measures for the manifest, at Stop and in recovery alike.
    pub fn duration_us(&self, timescale: u32) -> Us {
        let ticks: u64 = self
            .fragments
            .iter()
            .flat_map(|f| &f.samples)
            .map(|s| u64::from(s.duration))
            .sum();
        let scale = u128::from(timescale.max(1));
        ((u128::from(ticks) * 1_000_000 + scale / 2) / scale) as Us
    }
}

impl FragIndex {
    /// The byte after the fragment's last sample.
    pub fn data_end(&self) -> u64 {
        self.data_at + self.samples.iter().map(|s| u64::from(s.size)).sum::<u64>()
    }
}

/// `trun` sample flags: a sync sample, and a sample that depends on others and isn't one.
pub(crate) const KEY_FLAGS: u32 = 0x0200_0000;
pub(crate) const OTHER_FLAGS: u32 = 0x0101_0000;
/// The `sample_is_non_sync_sample` bit of sample flags.
pub(crate) const NON_SYNC: u32 = 0x0001_0000;
