//! The waveform peaks written in the PCM decode pass, for the timeline (`<hash>.peaks`, next to the `.pcm`).
//!
//! Layout (all little-endian): a 32-byte header — magic `KLPOPKS1`, channels u16, levels u16, frames per level-0
//! bucket u32 (`BUCKET`), source frames u64, 8 reserved bytes — then the levels one after another. Level `k` has
//! `ceil(frames / (BUCKET × 4^k))` buckets; a bucket is `[min, max]` as i16 (full scale = ±32767) per channel. The
//! layout follows BBC audiowaveform's .dat v2 (min/max pairs per bucket) with a 4× pyramid on top.

use klipolo_platform::durable::{self, Durability};
use klipolo_platform::file::PositionedIo;
use std::fs::File;
use std::io;
use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

/// Source frames per level-0 bucket (187.5 buckets per second at 48 kHz).
pub const BUCKET: u64 = 256;
pub const LEVELS: usize = 4;
const HEADER: u64 = 32;
const MAGIC: &[u8; 8] = b"KLPOPKS1";
/// Level-0 buckets written per `pwrite`.
const BATCH: usize = 4096;
/// The most buckets one `read` returns (128 KiB of stereo: 16,384 × 2 channels × [min, max] × 2 bytes).
pub const MAX_READ: u32 = 16_384;

/// A finished peaks file's header, for the timeline (`media_peaks_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeaksInfo {
    pub channels: u16,
    pub levels: u16,
    /// Source frames (48 kHz) per level-0 bucket; each level up has 4× fewer buckets.
    pub bucket: u32,
    pub frames: u64,
}

/// The header of the peaks file at `path`; None when it isn't one.
pub fn info(path: &Path) -> Option<PeaksInfo> {
    let mut header = [0u8; HEADER as usize];
    File::open(path).ok()?.read_exact_at(&mut header, 0).ok()?;
    if &header[..8] != MAGIC {
        return None;
    }
    let info = PeaksInfo {
        channels: u16::from_le_bytes([header[8], header[9]]),
        levels: u16::from_le_bytes([header[10], header[11]]),
        bucket: u32::from_le_bytes(header[12..16].try_into().ok()?),
        frames: u64::from_le_bytes(header[16..24].try_into().ok()?),
    };
    (info.channels > 0 && info.levels > 0 && info.bucket > 0).then_some(info)
}

/// Where level `level` (at most the coarsest) starts in the file, and how many buckets it has.
fn level_at(info: &PeaksInfo, level: u32) -> (u64, u64) {
    let entry = u64::from(info.channels) * 4;
    let mut offset = HEADER;
    let mut len = info.frames.div_ceil(u64::from(info.bucket));
    for _ in 0..level.min(u32::from(info.levels) - 1) {
        offset += len * entry;
        len = len.div_ceil(4);
    }
    (offset, len)
}

/// Buckets `[from, from + count)` of `level` (clamped to the level's length, at most `MAX_READ`), as stored: i16
/// min/max per channel, little-endian.
pub fn read(path: &Path, level: u32, from: u64, count: u32) -> io::Result<Vec<u8>> {
    let info = info(path).ok_or_else(|| io::Error::other("not a peaks file"))?;
    let entry = u64::from(info.channels) * 4;
    let (offset, len) = level_at(&info, level);
    let from = from.min(len);
    let count = u64::from(count.min(MAX_READ)).min(len - from);
    let mut out = vec![0u8; (count * entry) as usize];
    File::open(path)?.read_exact_at(&mut out, offset + from * entry)?;
    Ok(out)
}

/// The loudest sample of each channel over the whole file, 0–1 (which side of a one-sided recording
/// carries the sound, for `centreSource`). Read from the coarsest level, whose buckets keep every sample's extremes, in
/// `MAX_READ`-bucket steps into one buffer: about 84 KB for an hour of stereo. None when `path` isn't a finished peaks
/// file or can't be read.
pub fn loudest(path: &Path) -> Option<Vec<f32>> {
    let info = info(path)?;
    let (offset, len) = level_at(&info, u32::from(info.levels) - 1);
    let channels = usize::from(info.channels);
    let entry = channels * 4;
    let file = File::open(path).ok()?;
    let mut buf = vec![0u8; len.min(u64::from(MAX_READ)) as usize * entry];
    let mut peak = vec![0u16; channels];
    let mut at = 0;
    while at < len {
        let count = (len - at).min(u64::from(MAX_READ));
        let bytes = &mut buf[..count as usize * entry];
        file.read_exact_at(bytes, offset + at * entry as u64).ok()?;
        for bucket in bytes.chunks_exact(entry) {
            for (c, p) in peak.iter_mut().enumerate() {
                let lo = i16::from_le_bytes([bucket[c * 4], bucket[c * 4 + 1]]);
                let hi = i16::from_le_bytes([bucket[c * 4 + 2], bucket[c * 4 + 3]]);
                *p = (*p).max(lo.unsigned_abs()).max(hi.unsigned_abs());
            }
        }
        at += count;
    }
    Some(
        peak.into_iter()
            .map(|p| (f32::from(p) / 32767.0).min(1.0))
            .collect(),
    )
}

pub struct Peaks {
    file: File,
    channels: usize,
    /// The bucket being filled, the frame expected next and how many frames it has.
    bucket: u64,
    next: u64,
    count: u64,
    lo: [f32; 2],
    hi: [f32; 2],
    batch: Vec<i16>,
    batch_first: u64,
}

fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

impl Peaks {
    pub fn create(path: &Path, channels: usize) -> io::Result<Peaks> {
        Ok(Peaks {
            file: File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)?,
            channels,
            bucket: 0,
            next: 0,
            count: 0,
            lo: [f32::MAX; 2],
            hi: [f32::MIN; 2],
            batch: Vec::with_capacity(BATCH * channels * 2),
            batch_first: 0,
        })
    }

    fn entry_bytes(&self) -> u64 {
        self.channels as u64 * 4
    }

    /// Interleaved samples from frame `first`; a new run (a jump) starts at a bucket boundary.
    pub fn push(&mut self, first: u64, samples: &[f32]) -> io::Result<()> {
        if first != self.next {
            self.end_run(false)?;
            self.bucket = first / BUCKET;
            self.count = first % BUCKET;
        }
        for frame in samples.chunks_exact(self.channels) {
            for (c, &x) in frame.iter().enumerate() {
                self.lo[c] = self.lo[c].min(x);
                self.hi[c] = self.hi[c].max(x);
            }
            self.count += 1;
            if self.count == BUCKET {
                self.emit()?;
            }
        }
        self.next = first + (samples.len() / self.channels) as u64;
        Ok(())
    }

    fn emit(&mut self) -> io::Result<()> {
        if self.batch.is_empty() {
            self.batch_first = self.bucket;
        }
        for c in 0..self.channels {
            self.batch.extend([to_i16(self.lo[c]), to_i16(self.hi[c])]);
        }
        self.lo = [f32::MAX; 2];
        self.hi = [f32::MIN; 2];
        self.bucket += 1;
        self.count = 0;
        if self.batch.len() >= BATCH * self.channels * 2 {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.batch.is_empty() {
            let at = HEADER + self.batch_first * self.entry_bytes();
            self.file
                .write_all_at(bytemuck::cast_slice(&self.batch), at)?;
            self.batch.clear();
        }
        Ok(())
    }

    /// Ends a decode run. At the end of the file its last, partial bucket is written (`keep`); a run that stopped
    /// early drops it (a later run decodes that bucket whole).
    pub fn end_run(&mut self, keep: bool) -> io::Result<()> {
        if keep && self.count > 0 {
            self.emit()?;
        }
        self.lo = [f32::MAX; 2];
        self.hi = [f32::MIN; 2];
        self.count = 0;
        self.next = u64::MAX;
        self.flush()
    }

    /// Writes the coarser levels (from level 0, read back) and the header, then syncs the file (`Ordered`: once it is
    /// renamed, a crash or power cut can't leave the header without its buckets).
    pub fn finish(mut self, frames: u64) -> io::Result<()> {
        self.end_run(false)?;
        let entry = self.entry_bytes() as usize;
        let values = self.channels * 2;
        let mut n = frames.div_ceil(BUCKET) as usize;
        let mut offset = HEADER + (n * entry) as u64;
        // Level 1 from level 0 in chunks; each further level from the one before, in memory (a third of level 0).
        let mut level = Vec::with_capacity(n.div_ceil(4) * values);
        let mut chunk = vec![0i16; BATCH * values];
        let mut at = 0;
        while at < n {
            let count = (n - at).min(BATCH);
            let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut chunk[..count * values]);
            self.file
                .read_exact_at(bytes, HEADER + (at * entry) as u64)?;
            reduce(&chunk[..count * values], values, &mut level);
            at += count;
        }
        for _ in 1..LEVELS {
            self.file
                .write_all_at(bytemuck::cast_slice(&level), offset)?;
            offset += (level.len() * 2) as u64;
            n = level.len() / values;
            let mut next = Vec::with_capacity(n.div_ceil(4) * values);
            reduce(&level, values, &mut next);
            level = next;
        }
        self.file.set_len(offset)?;
        let mut header = [0u8; HEADER as usize];
        header[..8].copy_from_slice(MAGIC);
        header[8..10].copy_from_slice(&(self.channels as u16).to_le_bytes());
        header[10..12].copy_from_slice(&(LEVELS as u16).to_le_bytes());
        header[12..16].copy_from_slice(&(BUCKET as u32).to_le_bytes());
        header[16..24].copy_from_slice(&frames.to_le_bytes());
        self.file.write_all_at(&header, 0)?;
        durable::sync(&self.file, Durability::Ordered)
    }
}

/// Appends one coarser bucket per four of `buckets` (each `values` i16s: min, max per channel).
fn reduce(buckets: &[i16], values: usize, out: &mut Vec<i16>) {
    for group in buckets.chunks(values * 4) {
        for v in 0..values {
            let column = group.iter().skip(v).step_by(values);
            out.push(if v % 2 == 0 {
                column.copied().min().unwrap_or(0)
            } else {
                column.copied().max().unwrap_or(0)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loudest_sample_of_each_channel_is_read_from_the_coarsest_level() {
        // Ten seconds of [voice, near-silence] (more buckets at the coarsest level than one read takes would need an
        // hour; the stepping is the same loop), with one loud sample late on the left.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mic.peaks");
        let mut peaks = Peaks::create(&path, 2).unwrap();
        let frames = 480_000u64;
        let mut block = Vec::with_capacity(2 * 4_800);
        for first in (0..frames).step_by(4_800) {
            block.clear();
            for i in 0..4_800u64 {
                let left = if first + i == 400_123 { -0.9 } else { 0.5 };
                block.extend([left, 0.0004]);
            }
            peaks.push(first, &block).unwrap();
        }
        peaks.end_run(true).unwrap();
        peaks.finish(frames).unwrap();
        let got = loudest(&path).unwrap();
        assert_eq!(got.len(), 2);
        assert!((got[0] - 0.9).abs() < 1e-3, "{got:?}");
        assert!(got[1] < 0.001, "{got:?}");
        assert_eq!(loudest(&dir.path().join("none.peaks")), None);
    }
}
