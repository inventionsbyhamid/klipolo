//! Thumbnail scans: a file's missing sprite chunks cut from its keyframes (after its sound, never in the
//! sound's pass). The video stream is set to `AVDISCARD_NONKEY` and every other stream to `AVDISCARD_ALL`, so only
//! keyframes become packets. Two ways through a file:
//! - **Runs** (most video): adjacent missing chunks are one run from a seek to the first of them, reading keyframes in
//!   order until one lands past the last. The first keyframe of each one-second slot is decoded (on the hardware
//!   decoder where there is one), scaled straight to the 72 px tile and put in its slot (`cut::Cutter`).
//! - **Slots** (intra-only codecs: ProRes, MJPEG, DNxHD…, where every frame is a keyframe): one seek per slot and one
//!   frame decoded there, instead of reading every frame.
//!
//! A chunk is written only when every keyframe it shows was read, so a stopped scan loses at most the chunk it was
//! filling and the next scan redoes only that one. Runs also record the keyframes for the decoders' index
//! (`keyframes`); for intra-only files the index comes from the demuxer's own sample table.

mod cut;
mod intra;
#[cfg(test)]
mod tests;
mod tile;

pub(crate) use tile::{plain_layout, turn};

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::sprite::chunk_start_us;
use super::store::Shared;
use cut::Cutter;

/// Damaged packets skipped in a row before the file counts as unreadable (the demuxer resyncs past one).
const DAMAGED_IN_A_ROW: u32 = 1000;

/// Sets the stream discards for one pass: `keep` is read whole, `keys` keyframes only, every other stream not at all.
pub fn set_discards(
    input: &mut ffmpeg::format::context::Input,
    keep: Option<usize>,
    keys: Option<usize>,
) {
    for i in 0..input.nb_streams() as usize {
        let discard = if Some(i) == keep {
            ffmpeg::ffi::AVDiscard::AVDISCARD_DEFAULT
        } else if Some(i) == keys {
            ffmpeg::ffi::AVDiscard::AVDISCARD_NONKEY
        } else {
            ffmpeg::ffi::AVDiscard::AVDISCARD_ALL
        };
        if let Some(mut stream) = input.stream_mut(i) {
            // SAFETY: the stream belongs to `input`, which outlives this write; `discard` is a plain field FFmpeg
            // reads when demuxing (ffmpeg-next has no setter).
            unsafe { (*stream.as_mut_ptr()).discard = discard };
        }
    }
}

/// Bytes the demuxer has read from the file so far.
pub fn bytes_read(input: &ffmpeg::format::context::Input) -> u64 {
    // SAFETY: `pb` belongs to `input` (null for formats without one); `bytes_read` is a plain statistic.
    unsafe {
        let pb = (*input.as_ptr()).pb;
        if pb.is_null() {
            0
        } else {
            u64::try_from((*pb).bytes_read).unwrap_or(0)
        }
    }
}

/// How a scan ended.
#[derive(Debug, PartialEq, Eq)]
pub enum ScanEnd {
    /// Every chunk asked for was read (or the file has no video to cut).
    Done,
    /// Cancelled between packets: every chunk already written is kept.
    Stopped,
    /// The disk is full: thumbnails stop quietly.
    DiskFull,
    /// Reading failed part-way (an unplugged drive, a network share gone): tried again later.
    Unreadable(String),
    /// The file can't be opened or decoded.
    Failed(String),
}

/// What a scan did and what it cost.
#[derive(Debug)]
pub struct Scanned {
    pub end: ScanEnd,
    /// At least one chunk (or the keyframe index) was written.
    pub wrote: bool,
    pub bytes_read: u64,
    /// Keyframes decoded.
    pub decoded: u32,
}

/// The next packet: None at the end of the file, Err when reading fails (FFmpeg latches I/O errors, so the iterator
/// `Input::packets` would end there as if at the end). The sound cache's decode reads through it too.
pub(crate) fn next_packet(
    input: &mut ffmpeg::format::context::Input,
) -> Result<Option<(usize, ffmpeg::Packet)>, ffmpeg::Error> {
    let mut packet = ffmpeg::Packet::empty();
    for _ in 0..DAMAGED_IN_A_ROW {
        match packet.read(input) {
            Ok(()) => return Ok(Some((packet.stream(), packet))),
            Err(ffmpeg::Error::Eof) => return Ok(None),
            Err(ffmpeg::Error::InvalidData) => {}
            Err(e) => return Err(e),
        }
    }
    Err(ffmpeg::Error::InvalidData)
}

/// Scans `path`'s `missing` chunks (in the order given) and stores its keyframe index when it isn't stored yet (with
/// no chunk missing, a run over the whole file does only that). `proceed` is called between packets: it blocks while
/// scans are held and returns false once cancelled.
pub(crate) fn scan_thumbs(
    shared: &Arc<Shared>,
    path: &str,
    missing: &[u32],
    proceed: &dyn Fn() -> bool,
) -> Scanned {
    let mut scanned = Scanned {
        end: ScanEnd::Done,
        wrote: false,
        bytes_read: 0,
        decoded: 0,
    };
    let _ = ffmpeg::init();
    let Some(mut cutter) = Cutter::new(shared.clone(), path) else {
        scanned.end = ScanEnd::Failed("The file can't be opened.".into());
        return scanned;
    };
    let mut input = match crate::media::open::input(&path) {
        Ok(input) => input,
        Err(e) => {
            // FFmpeg's reason names no path; the warning names the file by its name only.
            scanned.end = ScanEnd::Failed(format!("The file can't be opened: {e}"));
            return scanned;
        }
    };
    let Some(index) = cutter.attach(&input) else {
        return scanned;
    };
    set_discards(&mut input, None, Some(index));
    scanned.end = if cutter.intra_only() {
        intra::slots(&mut cutter, &mut input, index, missing, proceed)
    } else {
        runs(&mut cutter, &mut input, path, index, missing, proceed)
    };
    scanned.wrote = cutter.wrote();
    scanned.decoded = cutter.decoded();
    scanned.bytes_read = bytes_read(&input);
    if cutter.stopped() && shared.disk_full.load(Ordering::SeqCst) {
        scanned.end = ScanEnd::DiskFull;
    }
    scanned
}

/// Where the container starts, in AV_TIME_BASE units.
pub(crate) fn start_us(input: &ffmpeg::format::context::Input) -> i64 {
    // SAFETY: a plain field of the open context (AV_NOPTS_VALUE when unknown).
    match unsafe { (*input.as_ptr()).start_time } {
        i64::MIN => 0,
        t => t,
    }
}

/// Runs over adjacent missing chunks, reading keyframes in order.
fn runs(
    cutter: &mut Cutter,
    input: &mut ffmpeg::format::context::Input,
    path: &str,
    index: usize,
    missing: &[u32],
    proceed: &dyn Fn() -> bool,
) -> ScanEnd {
    let mut spans: Vec<(u32, u32)> = Vec::new();
    for &c in missing {
        match spans.last_mut() {
            Some(span) if span.1 == c => span.1 = c + 1,
            _ => spans.push((c, c + 1)),
        }
    }
    if spans.is_empty() {
        spans.push((0, u32::MAX));
    }
    let start = start_us(input);
    let mut fresh = true;
    for (from, until) in spans {
        if !proceed() {
            return ScanEnd::Stopped;
        }
        let mut begin_us = chunk_start_us(from);
        if from > 0 || !fresh {
            let ts = begin_us + start;
            if input.seek(ts, ..ts).is_err() {
                // It can't seek there: read it from the start (only missing chunks are written).
                match crate::media::open::input(&path) {
                    Ok(again) => *input = again,
                    Err(e) => return ScanEnd::Unreadable(format!("Input Open / {e}")),
                }
                set_discards(input, None, Some(index));
                begin_us = 0;
            }
        }
        fresh = false;
        cutter.begin(begin_us);
        let until_us = chunk_start_us(until.min(u32::MAX - 1));
        let mut eof = true;
        loop {
            if !proceed() {
                cutter.end(false);
                return ScanEnd::Stopped;
            }
            let packet = match next_packet(input) {
                Ok(Some((stream, packet))) if stream == index && packet.is_key() => packet,
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(e) => {
                    // What was read in full is kept; the rest is tried again later.
                    cutter.end(false);
                    return ScanEnd::Unreadable(e.to_string());
                }
            };
            // The keyframe past the run is recorded, not decoded: it joins this run to the next in the index.
            let past = packet
                .pts()
                .and_then(|pts| cutter.us(pts))
                .is_some_and(|us| us >= until_us);
            cutter.packet(&packet, !past);
            if past || cutter.stopped() {
                eof = false;
                break;
            }
        }
        cutter.end(eof);
        if cutter.stopped() {
            break;
        }
    }
    ScanEnd::Done
}
