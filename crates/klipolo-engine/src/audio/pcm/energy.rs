//! The loudness energies written in the PCM decode pass (`<hash>.energy`, next to the `.pcm` and `.peaks`): for each
//! 100 ms window (`WINDOW` frames) of the file, each channel's K-weighted mean square (ITU-R BS.1770, the weighting
//! R128 measures through). The preview's loudness sums them as powers over the timeline (`loudness::estimate_until`)
//! instead of mixing the whole timeline again after every edit, and the file's own integrated loudness comes from
//! them (`loudness::integrated`), so the pass weights each sample once.
//!
//! Layout (all little-endian): a 32-byte header — magic `KLPOENG1`, channels u16, format version u16 (`VERSION`),
//! frames per window u32 (`WINDOW`), source frames u64, the CRC-32 of the windows u32, 4 reserved bytes — then
//! `ceil(frames / WINDOW)` windows of one f32 per channel. That is 10 windows a second: **144,000 bytes per
//! channel-hour** (288 KB for an hour of stereo, 0.04 % of its 691 MB of PCM), mapped when read.
//!
//! A file is used only when it matches its cache (`Energies::open`): its header, version, channel count, exact length
//! for the source's length, and checksum. A short file (a crash mid-write) or a valid header over zeroed windows (a
//! power cut that kept the header but not the data) never reads as silence: the preview measures exactly instead, and
//! the next opening of the cache makes the energies again from the decoded sound (`rebuild`, no new decode). The CRC
//! costs ~30 µs and one read of the file per hour of stereo, at write and at each open. The file is written through
//! the durable path: a temporary file, synced (`Ordered`), then renamed.
//!
//! The build decodes in runs (from the playhead, then the gaps), and a run that stops part-way through a chunk
//! leaves that chunk to a later run. So a run writes a chunk's windows once it has decoded the whole chunk, or at the
//! end of the file, and only the first time: a window across a chunk's edge gets the two chunks' shares added, and
//! frames decoded twice count once. Each run's weighting starts at rest (it settles within milliseconds).

use klipolo_platform::durable::{self, Durability};
use klipolo_platform::file::PositionedIo;
use std::fs::{self, File};
use std::io;
use std::path::Path;

use memmap2::Mmap;

use super::handle::{CHUNK, Pcm};
use crate::audio::loudness::{KWeight, integrated};

/// Frames per window: 100 ms at 48 kHz.
pub const WINDOW: usize = 4_800;
const HEADER: u64 = 32;
const MAGIC: &[u8; 8] = b"KLPOENG1";
/// The format: 2 added the checksum (an older file is made again).
const VERSION: u16 = 2;
/// Windows read at a time to check or sum a file (64 KB of stereo).
const READ_WINDOWS: usize = 8_192;
/// The most windows a chunk touches (49,152 frames over 4,800-frame windows: 11, or 12 when both ends cut one).
const PER_CHUNK: usize = (CHUNK as usize).div_ceil(WINDOW) + 1;

fn window_of(frame: u64) -> u64 {
    frame / WINDOW as u64
}

/// A finished energies file, mapped.
pub struct Energies {
    map: Mmap,
    channels: usize,
    frames: u64,
    windows: usize,
}

impl Energies {
    /// Opens the finished energies file of a cache holding `frames` frames of `channels` (1 or 2): None unless its
    /// header, version, channels, exact length and checksum all match (see the module docs). O(file): one CRC pass.
    pub fn open(path: &Path, channels: usize, frames: u64) -> Option<Energies> {
        let file = File::open(path).ok()?;
        // SAFETY: an energies file gets its name once finished and is never written again.
        let map = unsafe { Mmap::map(&file) }.ok()?;
        let h = map.get(..HEADER as usize)?;
        let windows = window_of(frames + WINDOW as u64 - 1) as usize;
        let ok = &h[..8] == MAGIC
            && usize::from(u16::from_le_bytes([h[8], h[9]])) == channels
            && (1..=2).contains(&channels)
            && u16::from_le_bytes([h[10], h[11]]) == VERSION
            && u32::from_le_bytes(h[12..16].try_into().ok()?) as usize == WINDOW
            && u64::from_le_bytes(h[16..24].try_into().ok()?) == frames
            && map.len() as u64 == HEADER + (windows * channels * 4) as u64
            && u32::from_le_bytes(h[24..28].try_into().ok()?)
                == crc32fast::hash(&map[HEADER as usize..]);
        ok.then_some(Energies {
            map,
            channels,
            frames,
            windows,
        })
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Window `w`'s power: the sum over channels of the mean square, a mono file counted on both speakers (as the
    /// mix plays it, and as R128 counts dual mono).
    pub fn power(&self, w: usize) -> f64 {
        self.heard(w, None)
    }

    /// Window `w`'s power as the mix plays the file: [`Self::power`], or channel `centre` alone on both speakers, as a
    /// mono file is, when it plays centred from that channel (`AudioSource::centre_from`; one the file has).
    fn heard(&self, w: usize, centre: Option<u8>) -> f64 {
        let at = HEADER as usize + w * self.channels * 4;
        let value = |c: usize| {
            let b = &self.map[at + c * 4..at + c * 4 + 4];
            f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        match (self.channels, centre.map(usize::from)) {
            (1, _) => 2.0 * value(0),
            (channels, Some(k)) if k < channels => 2.0 * value(k),
            _ => value(0) + value(1),
        }
    }

    /// The mean power over source frames `[a, b)` (fractional; silence outside the file) as the mix plays it, centred
    /// from channel `centre` or as recorded (`heard`), weighting each window by how much of it the range covers.
    /// O(windows covered).
    pub fn mean(&self, a: f64, b: f64, centre: Option<u8>) -> f64 {
        if b <= a {
            return 0.0;
        }
        let (lo, hi) = (a.max(0.0), b.min(self.frames as f64));
        if hi <= lo {
            return 0.0;
        }
        let w = WINDOW as f64;
        let first = (lo / w) as usize;
        let last = (((hi / w).ceil() as usize).max(first + 1)).min(self.windows);
        let sum: f64 = (first..last)
            .map(|k| {
                let (from, to) = (k as f64 * w, (k + 1) as f64 * w);
                (hi.min(to) - lo.max(from)).max(0.0) * self.heard(k, centre)
            })
            .sum();
        sum / (b - a)
    }
}

/// Writes a build's energies (see the module docs).
pub(crate) struct EnergyWriter {
    file: File,
    channels: usize,
    weight: [KWeight; 2],
    /// The chunk this run is summing, whether this run decoded it from its start, and the frame expected next.
    chunk: u64,
    whole: bool,
    next: u64,
    /// The chunk's windows so far: each channel's sum of squares (its first window may have begun in the chunk
    /// before, its last may go on into the next).
    sums: [f64; 2 * PER_CHUNK],
    /// One bit per chunk already written.
    written: Vec<u64>,
    /// Windows the file has room for.
    room: u64,
}

impl EnergyWriter {
    /// A new file at `path`, with room for `capacity` frames (0 when the container states no length).
    pub(crate) fn create(path: &Path, channels: usize, capacity: u64) -> io::Result<Self> {
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        let mut writer = Self {
            file,
            channels,
            weight: [KWeight::default(); 2],
            chunk: 0,
            whole: false,
            next: u64::MAX,
            sums: [0.0; 2 * PER_CHUNK],
            written: Vec::new(),
            room: 0,
        };
        writer.make_room(window_of(capacity) + 1)?;
        Ok(writer)
    }

    fn make_room(&mut self, windows: u64) -> io::Result<()> {
        if windows > self.room {
            self.file
                .set_len(HEADER + windows * self.channels as u64 * 4)?;
            self.room = windows;
        }
        Ok(())
    }

    /// A decode run starts at `from` (a chunk's start).
    pub(crate) fn begin(&mut self, from: u64) {
        self.weight = [KWeight::default(); 2];
        self.open_chunk(from / CHUNK, from.is_multiple_of(CHUNK));
        self.next = from;
    }

    fn open_chunk(&mut self, chunk: u64, whole: bool) {
        self.chunk = chunk;
        self.whole = whole;
        self.sums = [0.0; 2 * PER_CHUNK];
    }

    /// Interleaved samples from frame `first`, the run going on.
    pub(crate) fn push(&mut self, first: u64, samples: &[f32]) -> io::Result<()> {
        let ch = self.channels;
        let mut at = first;
        let mut samples = samples;
        if self.next == u64::MAX {
            self.begin(first);
        } else if first < self.next {
            // Already summed in this run.
            let skip = ((self.next - first) as usize * ch).min(samples.len());
            samples = &samples[skip..];
            at = self.next;
        } else if first > self.next {
            // A gap reads as silence (as in the cache); the weighting starts again after it.
            self.weight = [KWeight::default(); 2];
        }
        while !samples.is_empty() {
            let chunk_end = (self.chunk + 1) * CHUNK;
            if at >= chunk_end {
                // A gap past the chunk's end.
                self.commit()?;
                self.open_chunk(at / CHUNK, true);
                continue;
            }
            let window = window_of(at);
            let stop = ((window + 1) * WINDOW as u64).min(chunk_end);
            let n = ((stop - at) as usize).min(samples.len() / ch);
            let slot = (window - window_of(self.chunk * CHUNK)) as usize * ch;
            for frame in samples[..n * ch].chunks_exact(ch) {
                for (c, &x) in frame.iter().enumerate() {
                    let y = self.weight[c].run(x);
                    self.sums[slot + c] += y * y;
                }
            }
            samples = &samples[n * ch..];
            at += n as u64;
            // Written as soon as its last frame is in: the run may stop right here, at a chunk already decoded.
            if at == chunk_end {
                self.commit()?;
                self.open_chunk(self.chunk + 1, true);
            }
        }
        self.next = at;
        Ok(())
    }

    /// Ends a decode run. At the end of the file its last chunk is written as it is (`keep`); a run that stopped
    /// early drops the chunk it was in (a later run decodes it whole).
    pub(crate) fn end_run(&mut self, keep: bool) -> io::Result<()> {
        if keep {
            self.commit()?;
        }
        self.whole = false;
        self.next = u64::MAX;
        Ok(())
    }

    /// Writes the current chunk's windows, unless this run didn't decode it from its start or it was written before.
    fn commit(&mut self) -> io::Result<()> {
        let (c, ch) = (self.chunk as usize, self.channels);
        let done = self
            .written
            .get(c / 64)
            .is_some_and(|w| w & (1 << (c % 64)) != 0);
        if !self.whole || done {
            return Ok(());
        }
        if self.written.len() <= c / 64 {
            self.written.resize(c / 64 + 1, 0);
        }
        self.written[c / 64] |= 1 << (c % 64);
        let (from, to) = (self.chunk * CHUNK, (self.chunk + 1) * CHUNK);
        let (first, last) = (window_of(from), window_of(to - 1));
        self.make_room(last + 1)?;
        let count = (last - first + 1) as usize;
        let mut values = [0f32; 2 * PER_CHUNK];
        let values = &mut values[..count * ch];
        // The windows across the chunk's edges hold the neighbours' shares already (or zeros): add to them.
        let at = |w: u64| HEADER + w * ch as u64 * 4;
        let mut edge = [0u8; 8];
        for (w, cut) in [
            (first, from > first * WINDOW as u64),
            (last, to < (last + 1) * WINDOW as u64),
        ] {
            if cut {
                self.file.read_exact_at(&mut edge[..ch * 4], at(w))?;
                for c in 0..ch {
                    let b = &edge[c * 4..c * 4 + 4];
                    values[(w - first) as usize * ch + c] =
                        f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                }
            }
        }
        for (v, sum) in values.iter_mut().zip(&self.sums) {
            *v += (sum / WINDOW as f64) as f32;
        }
        let mut bytes = [0u8; 8 * PER_CHUNK];
        for (b, v) in bytes.chunks_exact_mut(4).zip(values.iter()) {
            b.copy_from_slice(&v.to_le_bytes());
        }
        self.file
            .write_all_at(&bytes[..values.len() * 4], at(first))
    }

    /// Trims the file to `frames`, writes its header (with the windows' checksum) and syncs it (`Ordered`, so a crash
    /// or power cut can't leave the header without its windows once renamed); returns the file's integrated loudness
    /// (None: silent). Reads the windows back once, `READ_WINDOWS` at a time: it holds 8 bytes per window (the
    /// powers the gating needs: 288 KB for an hour) and a buffer of at most 64 KB.
    pub(crate) fn finish(mut self, frames: u64) -> io::Result<Option<f64>> {
        self.end_run(false)?;
        let ch = self.channels;
        let windows = window_of(frames + WINDOW as u64 - 1);
        self.make_room(windows)?;
        self.file.set_len(HEADER + windows * ch as u64 * 4)?;
        // A last window shorter than the others: its mean over the frames it has.
        let tail = frames % WINDOW as u64;
        if tail > 0 {
            let at = HEADER + (windows - 1) * ch as u64 * 4;
            let mut last = [0u8; 8];
            self.file.read_exact_at(&mut last[..ch * 4], at)?;
            for v in last[..ch * 4].chunks_exact_mut(4) {
                let x = f32::from_le_bytes([v[0], v[1], v[2], v[3]]) * WINDOW as f32 / tail as f32;
                v.copy_from_slice(&x.to_le_bytes());
            }
            self.file.write_all_at(&last[..ch * 4], at)?;
        }
        // Every window goes into the checksum; the whole ones (as the meter counts) into the loudness.
        let whole = window_of(frames) as usize;
        let dual = if ch == 1 { 2.0 } else { 1.0 };
        let mut power = Vec::with_capacity(whole);
        let mut crc = crc32fast::Hasher::new();
        let mut bytes = vec![0u8; READ_WINDOWS.min(windows as usize) * ch * 4];
        let mut at = 0;
        while at < windows as usize {
            let n = (windows as usize - at).min(READ_WINDOWS);
            let block = &mut bytes[..n * ch * 4];
            self.file
                .read_exact_at(block, HEADER + (at * ch * 4) as u64)?;
            crc.update(block);
            power.extend(
                block
                    .chunks_exact(ch * 4)
                    .take(whole.saturating_sub(at))
                    .map(|w| {
                        dual * w
                            .chunks_exact(4)
                            .map(|b| f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                            .sum::<f64>()
                    }),
            );
            at += n;
        }
        drop(bytes);
        let mut header = [0u8; HEADER as usize];
        header[..8].copy_from_slice(MAGIC);
        header[8..10].copy_from_slice(&(ch as u16).to_le_bytes());
        header[10..12].copy_from_slice(&VERSION.to_le_bytes());
        header[12..16].copy_from_slice(&(WINDOW as u32).to_le_bytes());
        header[16..24].copy_from_slice(&frames.to_le_bytes());
        header[24..28].copy_from_slice(&crc.finalize().to_le_bytes());
        self.file.write_all_at(&header, 0)?;
        durable::sync(&self.file, Durability::Ordered)?;
        Ok(integrated(&power))
    }
}

/// Makes the energies of a finished cache again from its decoded sound, into `tmp`, then renames that to `path` (no
/// new decode: a pass over the mapped PCM, K-weighted as the decode pass does; about 2 s of CPU and one read of the
/// cache per hour of stereo, on the build's thread). Ok(false) when `cancelled` stopped it (a closed project, or the
/// build's caller); the temporary file is removed then, and on an error.
pub(crate) fn rebuild(
    pcm: &Pcm,
    path: &Path,
    tmp: &Path,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    let (ch, frames) = (pcm.channels(), pcm.frames() as u64);
    let made = (|| {
        let mut writer = EnergyWriter::create(tmp, ch, frames)?;
        writer.begin(0);
        let mut block = vec![0f32; READ_WINDOWS * ch];
        let mut at = 0;
        while at < frames {
            if cancelled() {
                return Ok(false);
            }
            let n = (frames - at).min(READ_WINDOWS as u64) as usize;
            pcm.read(at as i64, &mut block[..n * ch]);
            writer.push(at, &block[..n * ch])?;
            at += n as u64;
        }
        writer.end_run(true)?;
        writer.finish(frames)?;
        fs::rename(tmp, path)?;
        Ok(true)
    })();
    if !matches!(made, Ok(true)) {
        let _ = fs::remove_file(tmp);
    }
    made
}
