//! Sound runs (mic, game or computer sound) as Core Audio Format files of 24-bit little-endian PCM.
//! There is no encoder to fail, and drift correction is sample-exact.
//!
//! - **Header** (`header`, 68 bytes): `caff` v1, a `desc` chunk (`lpcm`, the little-endian flag, 24 bits, one frame
//!   per packet), then a `data` chunk whose size is −1, "unknown". Apple's `CAFFile.h` says such a file "is still well
//!   defined" if the program crashes while recording, and FFmpeg and AVFoundation read it to the end of the file. So
//!   every whole frame that reached the disk is readable as is, with no finishing step.
//! - **Samples** (`encode`) follow the header as they come.
//! - **Salvage** (`scan`, `keep_frames`): the scan counts the whole frames, leaving out a torn last one. A power cut
//!   can leave zeros where the file grew but the data never landed; `keep_frames` drops a trailing run of all-zero
//!   frames, but only past the journal's durable mark: silence before it is real silence.
//! - **Finalize** (`finalize`, at Stop or in recovery): truncates to the frames kept and writes the data size, then
//!   syncs. It checks first and refuses (InvalidData, nothing written) to cut anything but zero frames and a torn frame,
//!   or to pad the file. Idempotent: a re-run after a crash or a write error at any step finishes it, and a size torn
//!   by one reads as unknown, never as damage.
//!
//! Memory: salvage reads in 64 KiB pieces into one buffer; nothing holds the media.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use klipolo_platform::durable::Durability;
use klipolo_platform::file::PositionedIo;

use crate::io::Io;
use crate::media::{AudioFormat, PartialBlock, PcmBlock};

/// The header's length; the first sample follows it.
pub const HEADER_LEN: u64 = 68;
/// Bytes per sample: 24-bit PCM.
const SAMPLE_BYTES: u64 = 3;
/// `kCAFLinearPCMFormatFlagIsLittleEndian` (integer samples: `IsFloat` is clear).
const LITTLE_ENDIAN: u32 = 2;
/// The `desc` chunk's body.
const DESC_LEN: i64 = 32;
/// The largest 24-bit sample; the clip is symmetric.
const FULL_SCALE: i32 = (1 << 23) - 1;
/// How much salvage and `finalize` read at a time.
const STEP_BYTES: u64 = 64 * 1024;

/// A sound run's file as found on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CafScan {
    pub format: AudioFormat,
    /// The first sample's first byte (after the `data` chunk's edit count).
    pub data_at: u64,
    /// Whole frames: for an unknown size, as many as the file holds; for a set size, the size's, but never more than
    /// the file holds.
    pub frames: u64,
    /// The data size is set (the file was finalized), not unknown (negative).
    pub sized: bool,
}

impl CafScan {
    /// Bytes per frame: 3 × channels.
    pub fn frame_bytes(&self) -> u64 {
        frame_bytes(self.format)
    }

    /// The whole frames inside the file's first `len` bytes: a durable length (the journal's `Durable` mark) in frames.
    pub fn frames_within(&self, len: u64) -> u64 {
        (len.saturating_sub(self.data_at) / self.frame_bytes()).min(self.frames)
    }
}

/// The header for a run of `format`, with the data size −1 ("unknown") and an edit count of 0.
pub fn header(format: AudioFormat) -> Vec<u8> {
    let channels = u32::from(format.channels);
    let mut h = Vec::with_capacity(HEADER_LEN as usize);
    h.extend_from_slice(b"caff");
    h.extend_from_slice(&1u16.to_be_bytes()); // file version
    h.extend_from_slice(&0u16.to_be_bytes()); // file flags
    h.extend_from_slice(b"desc");
    h.extend_from_slice(&DESC_LEN.to_be_bytes());
    h.extend_from_slice(&f64::from(format.rate).to_be_bytes());
    h.extend_from_slice(b"lpcm");
    for v in [LITTLE_ENDIAN, 3 * channels, 1, channels, 24] {
        // flags, bytes per packet, frames per packet, channels per frame, bits per channel
        h.extend_from_slice(&v.to_be_bytes());
    }
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(-1i64).to_be_bytes());
    h.extend_from_slice(&0u32.to_be_bytes()); // edit count
    debug_assert_eq!(h.len() as u64, HEADER_LEN);
    h
}

/// `block`'s samples as 24-bit little-endian PCM, clipped to ±(2²³ − 1). One allocation of the exact size.
pub fn encode(block: &PcmBlock) -> Vec<u8> {
    let mut out = Vec::with_capacity(block.samples.len() * SAMPLE_BYTES as usize);
    encode_into(&block.samples, &mut out);
    out
}

/// Appends `samples` to `out` as `encode` does, for a caller that reuses its buffer. A sample of k / 2²³ (what a
/// 24-bit device delivers as float) comes back exactly k; NaN becomes 0.
pub fn encode_into(samples: &[f32], out: &mut Vec<u8>) {
    out.reserve(samples.len() * SAMPLE_BYTES as usize);
    for &s in samples {
        let v = ((s * 8_388_608.0).round() as i32).clamp(-FULL_SCALE, FULL_SCALE);
        out.extend_from_slice(&v.to_le_bytes()[..3]);
    }
}

/// Appends `block` to `out` as `encode_into` does when it holds whole frames of `format`, and gives their count; a
/// partial block is refused with nothing appended (`PcmBlock::whole_frames`). The core gathers sound this way.
pub fn encode_frames(
    format: AudioFormat,
    block: &PcmBlock,
    out: &mut Vec<u8>,
) -> Result<u64, PartialBlock> {
    let frames = block.whole_frames(format.channels)?;
    encode_into(&block.samples, out);
    Ok(frames)
}

/// Reads the chunks up to `data`; `len` is the file's length. InvalidData for a file that isn't a CAF of our PCM, or
/// whose chunks run past `len`; UnexpectedEof for one cut inside its header.
pub fn scan(file: &mut (impl Read + Seek), len: u64) -> io::Result<CafScan> {
    let mut head = [0u8; 8];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    if &head[..4] != b"caff" || head[4..6] != 1u16.to_be_bytes() {
        return Err(invalid("not a CAF file"));
    }
    let (mut at, mut format) = (8u64, None);
    loop {
        if at + 12 > len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the file ends before its data chunk",
            ));
        }
        let mut chunk = [0u8; 12];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut chunk)?;
        let size = i64::from_be_bytes(chunk[4..].try_into().expect("8 bytes"));
        let body = at + 12;
        if &chunk[..4] == b"data" {
            let format = format.ok_or_else(|| invalid("the data chunk comes before desc"))?;
            let data_at = body + 4;
            if data_at > len {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the data chunk's header is cut",
                ));
            }
            let held = (len - data_at) / frame_bytes(format);
            let (frames, sized) = match size {
                // Unknown, as FFmpeg reads any negative size: −1, or a size `finalize` tore (each byte the new size's
                // or −1's, so negative, or at least the true size, which the clamp below turns into what's held).
                s if s < 0 => (held, false),
                s if s >= 4 => (((s as u64 - 4) / frame_bytes(format)).min(held), true),
                _ => return Err(invalid("a bad data chunk size")),
            };
            return Ok(CafScan {
                format,
                data_at,
                frames,
                sized,
            });
        }
        if size < 0 || size as u64 > len.saturating_sub(body) {
            return Err(invalid("a chunk runs past the end of the file"));
        }
        if &chunk[..4] == b"desc" {
            if size != DESC_LEN {
                return Err(invalid("a bad desc chunk size"));
            }
            let mut desc = [0u8; DESC_LEN as usize];
            file.read_exact(&mut desc)?;
            format = Some(desc_format(&desc)?);
        }
        at = body + size as u64;
    }
}

/// The format of a `desc` body that describes our PCM: integer, little-endian, 24 bits, one frame a packet.
fn desc_format(d: &[u8; DESC_LEN as usize]) -> io::Result<AudioFormat> {
    let rate = f64::from_be_bytes(d[..8].try_into().expect("8 bytes"));
    let word = |i: usize| u32::from_be_bytes(d[i..i + 4].try_into().expect("4 bytes"));
    let (flags, packet, per_packet, channels, bits) =
        (word(12), word(16), word(20), word(24), word(28));
    let ours = &d[8..12] == b"lpcm"
        && flags == LITTLE_ENDIAN
        && bits == 24
        && per_packet == 1
        && (1..=u32::from(u16::MAX)).contains(&channels)
        && u64::from(packet) == SAMPLE_BYTES * u64::from(channels)
        && rate.fract() == 0.0
        && rate >= 1.0
        && rate <= f64::from(u32::MAX);
    if !ours {
        return Err(invalid("not 24-bit little-endian PCM"));
    }
    Ok(AudioFormat {
        rate: rate as u32,
        channels: channels as u16,
    })
}

/// Frames to keep: all of them, except a trailing run of all-zero frames past `durable_frames` (written but never
/// flushed before a power cut); silence before that mark is real silence. Reads backwards from the end in 64 KiB
/// pieces, never before the mark.
pub fn keep_frames(
    file: &mut (impl Read + Seek),
    scan: &CafScan,
    durable_frames: u64,
) -> io::Result<u64> {
    let bpf = scan.frame_bytes();
    let floor = durable_frames.min(scan.frames);
    let step = (STEP_BYTES / bpf).max(1).min(scan.frames - floor);
    let mut buf = vec![0u8; (step * bpf) as usize];
    let mut end = scan.frames;
    while end > floor {
        let n = step.min(end - floor);
        let piece = &mut buf[..(n * bpf) as usize];
        file.seek(SeekFrom::Start(scan.data_at + (end - n) * bpf))?;
        file.read_exact(piece)?;
        if let Some(last) = piece.iter().rposition(|&b| b != 0) {
            return Ok(end - n + last as u64 / bpf + 1);
        }
        end -= n;
    }
    Ok(end)
}

/// Truncates `file` to `frames` whole frames and writes the data size, then syncs to the disk (`Durability::Full`).
///
/// Checks first, and returns InvalidData with nothing written, unless the `data` chunk is where `scan` found it and
/// the bytes cut are only zero frames (`keep_frames`' tail) and a torn last frame. Never pads: `frames` past the end of
/// the file is refused too.
///
/// Every step can be cut short and re-run: the cut is ordered before the size, so a crash leaves the unsized file or a
/// sized one; a size write that fails puts −1 back (best effort); and a size torn anyway still scans every frame. The
/// writes and syncs go through `io` (`path` names the file in its log).
pub fn finalize(
    io: &dyn Io,
    file: &File,
    path: &Path,
    scan: &CafScan,
    frames: u64,
) -> io::Result<()> {
    let bpf = scan.frame_bytes();
    let data_bytes = frames
        .checked_mul(bpf)
        .filter(|&b| b <= i64::MAX as u64 - 4)
        .ok_or_else(|| invalid("too many frames"))?;
    let keep_end = scan.data_at + data_bytes;
    // The chunk's type, size and edit count come before the data.
    let chunk_at = scan
        .data_at
        .checked_sub(16)
        .ok_or_else(|| invalid("not a scanned data chunk"))?;
    let len = file.metadata()?.len();
    if keep_end > len {
        return Err(invalid("finalize would pad the sound"));
    }
    let mut kind = [0u8; 4];
    file.read_exact_at(&mut kind, chunk_at)?;
    if &kind != b"data" {
        return Err(invalid("the data chunk isn't where the scan found it"));
    }
    let whole_end = keep_end + (len - keep_end) / bpf * bpf;
    if !all_zero(file, keep_end, whole_end)? {
        return Err(invalid("finalize would cut sound that isn't a zero tail"));
    }
    if len != keep_end {
        io.truncate(file, path, keep_end)?;
        crate::io::sync(io, file, path, Durability::Ordered)?;
    }
    let size = (4 + data_bytes as i64).to_be_bytes();
    if let Err(e) = io.write_at(file, path, chunk_at + 4, &size) {
        let _ = io.write_at(file, path, chunk_at + 4, &(-1i64).to_be_bytes());
        return Err(e);
    }
    io.barrier(file, path)
}

/// `[from, to)` holds only zero bytes: read in 64 KiB pieces.
fn all_zero(file: &File, from: u64, to: u64) -> io::Result<bool> {
    let mut buf = vec![0u8; (to - from).min(STEP_BYTES) as usize];
    let mut at = from;
    while at < to {
        let piece = &mut buf[..(to - at).min(STEP_BYTES) as usize];
        file.read_exact_at(piece, at)?;
        if piece.iter().any(|&b| b != 0) {
            return Ok(false);
        }
        at += piece.len() as u64;
    }
    Ok(true)
}

/// The bytes of one frame of `format` (24-bit samples, every channel).
pub fn frame_bytes(format: AudioFormat) -> u64 {
    SAMPLE_BYTES * u64::from(format.channels)
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}
