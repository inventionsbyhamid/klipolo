//! The decoded audio of one file (`Pcm`): a finished or growing cache file, mapped, or samples in memory.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use memmap2::Mmap;

use super::{HEADER, MAGIC, READ_SCALE};

/// Readiness is tracked per chunk of this many frames (1.024 s, a whole number of peaks buckets), so a build can
/// decode in any order: from the playhead, jumping to a new playhead, then filling the gaps.
pub const CHUNK: u64 = 49_152;

pub(super) enum Data {
    /// Interleaved f32 (tests and synthetic sources).
    Owned(Vec<f32>),
    /// The cache file; samples start at `HEADER`.
    Mapped(Mmap),
}

/// Decoded audio of one file at 48 kHz, possibly still being written: a chunk is readable once its bit is set (all
/// of the file once complete). Anything else reads as silence.
pub struct Pcm {
    pub(super) data: Data,
    pub(super) channels: usize,
    /// Frames the mapping holds.
    pub(super) capacity: u64,
    /// One bit per `CHUNK`, set after the chunk's bytes are written.
    ready: Box<[AtomicU64]>,
    /// The file's length in frames, once its end has been decoded (`u64::MAX` until then).
    pub(super) total: AtomicU64,
    pub(super) complete: AtomicBool,
    /// Integrated loudness (LUFS, f64 bits); NaN when silent or not measured yet.
    pub(super) loudness: AtomicU64,
    /// The store replaced or forgot this file (a rebuild, a remap, a closed project): readers take the current one.
    pub(super) superseded: AtomicBool,
}

impl Pcm {
    /// Interleaved stereo f32, all ready.
    pub fn from_samples(samples: Vec<f32>) -> Pcm {
        Pcm::from_channels(samples, 2)
    }

    /// Interleaved f32 with `channels` (1 or 2), all ready.
    pub fn from_channels(samples: Vec<f32>, channels: usize) -> Pcm {
        let frames = (samples.len() / channels.max(1)) as u64;
        let complete = Some((frames, f64::NAN));
        Pcm::new(Data::Owned(samples), channels.max(1), frames, complete)
    }

    /// A cache file being written (nothing ready yet) or a finished one.
    pub(super) fn mapped(
        map: Mmap,
        channels: usize,
        capacity: u64,
        complete: Option<(u64, f64)>,
    ) -> Pcm {
        Pcm::new(Data::Mapped(map), channels, capacity, complete)
    }

    fn new(data: Data, channels: usize, capacity: u64, complete: Option<(u64, f64)>) -> Pcm {
        let (total, lufs) = complete.unwrap_or((u64::MAX, f64::NAN));
        let words = match complete {
            Some(_) => 0,
            None => capacity.div_ceil(CHUNK).div_ceil(64) as usize,
        };
        Pcm {
            data,
            channels,
            capacity,
            ready: (0..words).map(|_| AtomicU64::new(0)).collect(),
            total: AtomicU64::new(total),
            complete: AtomicBool::new(complete.is_some()),
            loudness: AtomicU64::new(lufs.to_bits()),
            superseded: AtomicBool::new(false),
        }
    }

    /// What a cache file being written without a mapping holds so far (written in order from the start), mapped and
    /// readable. None when nothing is written.
    pub(super) fn decoded_so_far(file: &File, channels: usize) -> Option<Pcm> {
        // SAFETY: the bytes mapped are written and nothing writes them again (the build has ended).
        let map = unsafe { Mmap::map(file) }.ok()?;
        let frames = (map.len().checked_sub(HEADER)? / (channels * 2)) as u64;
        if frames == 0 {
            return None;
        }
        let pcm = Pcm::mapped(map, channels, frames, None);
        (0..frames.div_ceil(CHUNK)).for_each(|c| pcm.set_chunk_ready(c));
        Some(pcm)
    }

    /// Opens a finished cache file (None if it isn't one).
    pub(super) fn open(path: &Path) -> Option<Pcm> {
        let file = File::open(path).ok()?;
        let len = file.metadata().ok()?.len();
        if len < HEADER as u64 {
            return None;
        }
        // SAFETY: cache files are finished before they get their real name and are never written again.
        let map = unsafe { Mmap::map(&file) }.ok()?;
        let h = &map[..HEADER];
        let channels = usize::from(u16::from_le_bytes([h[8], h[9]]));
        let frames = u64::from_le_bytes(h[16..24].try_into().ok()?);
        let lufs = f64::from(f32::from_le_bytes(h[24..28].try_into().ok()?));
        let complete = u32::from_le_bytes(h[28..32].try_into().ok()?) & 1 == 1;
        let capacity = (len - HEADER as u64) / (channels.max(1) as u64 * 2);
        (&h[..8] == MAGIC && (1..=2).contains(&channels) && complete && frames <= capacity)
            .then(|| Pcm::mapped(map, channels, capacity, Some((frames, lufs))))
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Whether chunk `c` is written (always, once complete).
    pub(super) fn chunk_ready(&self, c: u64) -> bool {
        self.is_complete()
            || self
                .ready
                .get((c / 64) as usize)
                .is_some_and(|w| w.load(Ordering::Acquire) & (1 << (c % 64)) != 0)
    }

    /// Marks chunk `c` written (after its bytes are). Chunks past the mapping are ignored.
    pub(super) fn set_chunk_ready(&self, c: u64) {
        if let Some(w) = self.ready.get((c / 64) as usize) {
            w.fetch_or(1 << (c % 64), Ordering::Release);
        }
    }

    /// Frames decoded so far from the start (all of them once complete).
    pub fn frames(&self) -> usize {
        let total = self.total.load(Ordering::Acquire);
        if self.is_complete() {
            return total as usize;
        }
        let leading = (0..).take_while(|&c| self.chunk_ready(c)).count() as u64;
        (leading * CHUNK).min(total).min(self.capacity) as usize
    }

    /// Whether frame `at` is decoded (playback has it).
    pub fn is_ready(&self, at: u64) -> bool {
        at < self.total.load(Ordering::Acquire)
            && (self.is_complete() || (at < self.capacity && self.chunk_ready(at / CHUNK)))
    }

    /// The store has a newer copy of this file, or forgot it: let this one go.
    pub fn is_superseded(&self) -> bool {
        self.superseded.load(Ordering::Acquire)
    }

    pub fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    /// The file's integrated loudness (EBU R128), once decoded; None for silence.
    pub fn loudness(&self) -> Option<f64> {
        Some(f64::from_bits(self.loudness.load(Ordering::Acquire))).filter(|l| l.is_finite())
    }

    /// Writes frames `[from, from + n)` into `out` (`n × channels` samples), silence where nothing is decoded.
    pub fn read(&self, from: i64, out: &mut [f32]) {
        let n = (out.len() / self.channels) as i64;
        out.fill(0.0);
        let total = self.total.load(Ordering::Acquire).min(self.capacity) as i64;
        let (a, b) = (from.max(0), (from + n).min(total));
        if a >= b {
            return;
        }
        if self.is_complete() {
            return self.copy(from, a, b, out);
        }
        let chunk = CHUNK as i64;
        let mut c = a / chunk;
        while c * chunk < b {
            if self.chunk_ready(c as u64) {
                self.copy(from, a.max(c * chunk), b.min((c + 1) * chunk), out);
            }
            c += 1;
        }
    }

    /// Copies frames `[a, b)` into `out`, which starts at frame `from`.
    fn copy(&self, from: i64, a: i64, b: i64, out: &mut [f32]) {
        let ch = self.channels;
        let dst = &mut out[(a - from) as usize * ch..(b - from) as usize * ch];
        let (a, b) = (a as usize * ch, b as usize * ch);
        match &self.data {
            Data::Owned(v) => dst.copy_from_slice(&v[a..b]),
            Data::Mapped(map) => {
                let src: &[i16] = bytemuck::cast_slice(&map[HEADER + a * 2..HEADER + b * 2]);
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = f32::from(*s) * READ_SCALE;
                }
            }
        }
    }

    /// Asks the OS to page in `frames` from `from` ahead of the mixer (after a seek).
    pub fn will_need(&self, from: i64, frames: usize) {
        if let Data::Mapped(map) = &self.data {
            let bytes_per = self.channels * 2;
            let a = (from.max(0) as usize).min(self.capacity as usize) * bytes_per;
            let len = (frames * bytes_per).min(map.len().saturating_sub(HEADER + a));
            if len > 0 {
                klipolo_platform::file::will_need(&map[HEADER + a..HEADER + a + len]);
            }
        }
    }
}
