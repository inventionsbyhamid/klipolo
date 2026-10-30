//! The activity file, in the media cache as `<stem>.activity`: a header, then 60-second chunks appended as the pass
//! makes them, each with its own CRC-32. Reading keeps every chunk whose checksum matches; the rest read as unknown
//! ticks and are made again.
//!
//! **Resuming**: `decode` the file with the screen file's own duration (the one `header` stored,
//! `stored_duration`; never the take's, which a later run of a resumed take doesn't span), truncate it to
//! `ActivityTrack::append_at` (0: write it again from `header`), then append the chunks `missing` lists. Truncating
//! first drops a chunk cut off mid-write, which would otherwise hide every chunk appended after it; so a stop loses at
//! most the chunk being written. A chunk made again after a damaged one further back is still read: the later copy
//! wins.
//!
//! **Layout** (little-endian):
//! - header: `MAGIC`, the duration in µs (i64), then a CRC-32 of both (u32): 20 bytes;
//! - chunk: its index (u32), its tick count (u16), the ticks (14 bytes each: changed u16, then each region's
//!   x0, y0, x1, y1 as u8 and cells u16), then a CRC-32 (u32) of the index, the count and the ticks. The checksum
//!   covers the index too, so a damaged index can't place a whole chunk at the wrong minute.
//!
//! A chunk holds every tick of its minute, or fewer only where the video ended early: the ticks it lacks stay unknown
//! and are never made again. Reading is one pass with a CRC over each chunk: O(bytes).

use super::{ActivityTrack, CHUNK_TICKS, GRID, Region, TICK_BYTES, TICK_US, Tick};
use crate::Us;

pub const MAGIC: &[u8; 8] = b"KLPOACT1";
/// The longest file a track covers: 24 hours (12 MB of ticks). `header` clamps to it, so a longer take's later
/// activity stays unknown.
pub const MAX_DURATION_US: Us = 24 * 3_600 * 1_000_000;
const HEADER_BYTES: usize = 8 + 8 + 4;
/// A chunk's index and tick count, before its ticks.
const CHUNK_HEAD: usize = 4 + 2;
const CHUNK: usize = CHUNK_TICKS as usize;

/// The file's first bytes. `decode` reads a file whose header can't be read, or that is for another duration, as all
/// missing, to be written again from here.
pub fn header(duration_us: Us, out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&duration_us.clamp(0, MAX_DURATION_US).to_le_bytes());
    let sum = crc32fast::hash(&out[start..]);
    out.extend_from_slice(&sum.to_le_bytes());
}

/// One chunk: index u32, tick count u16, the ticks (14 B each), CRC-32 u32 (of the index, the count and the ticks).
/// At most `CHUNK_TICKS` ticks are written.
pub fn encode_chunk(index: u32, ticks: &[Tick], out: &mut Vec<u8>) {
    debug_assert!(ticks.len() <= CHUNK, "a chunk is a minute");
    let ticks = &ticks[..ticks.len().min(CHUNK)];
    out.reserve(CHUNK_HEAD + ticks.len() * TICK_BYTES + 4);
    let start = out.len();
    out.extend_from_slice(&index.to_le_bytes());
    out.extend_from_slice(&(ticks.len() as u16).to_le_bytes());
    for t in ticks {
        out.extend_from_slice(&t.changed.to_le_bytes());
        for r in &t.top {
            out.extend_from_slice(&[r.x0, r.y0, r.x1, r.y1]);
            out.extend_from_slice(&r.cells.to_le_bytes());
        }
    }
    let sum = crc32fast::hash(&out[start..]);
    out.extend_from_slice(&sum.to_le_bytes());
}

/// The activity of a video of `duration_us` (clamped to `MAX_DURATION_US`): every chunk whose checksum matches; the rest
/// read as unknown ticks, and are missing. A short tail (a write cut off) is ignored. A file whose header can't be read,
/// or that is for another duration, reads as all missing with `append_at` 0.
pub fn decode(bytes: &[u8], duration_us: Us) -> ActivityTrack {
    let duration_us = duration_us.clamp(0, MAX_DURATION_US);
    let n = usize::try_from((duration_us + TICK_US - 1) / TICK_US).unwrap_or(0);
    let mut track = ActivityTrack {
        duration_us,
        ticks: vec![Tick::UNKNOWN; n],
        made: vec![false; n.div_ceil(CHUNK)],
        append_at: 0,
    };
    if read_header(bytes) != Some(duration_us) {
        return track;
    }
    track.append_at = HEADER_BYTES;
    let mut at = HEADER_BYTES;
    while let Some(head) = bytes.get(at..at + CHUNK_HEAD) {
        let index = le_u32(head) as usize;
        let count = usize::from(u16::from_le_bytes([head[4], head[5]]));
        if count > CHUNK {
            // The chunks' boundaries are lost: the rest reads as unknown, and is made again.
            break;
        }
        let len = CHUNK_HEAD + count * TICK_BYTES + 4;
        let Some(chunk) = bytes.get(at..at + len) else {
            break;
        };
        at += len;
        let (body, sum) = chunk.split_at(len - 4);
        if crc32fast::hash(body) != le_u32(sum) {
            continue;
        }
        // A whole chunk ends here, so the next one can go no earlier.
        track.append_at = at;
        if index >= track.made.len() {
            continue;
        }
        // This copy replaces any earlier one of the chunk, whole. Ticks past the duration (a decoder that gave a frame
        // more than it allows) are left out, so the chunk isn't made again forever.
        let first = index * CHUNK;
        let minute = &mut track.ticks[first..(first + CHUNK).min(n)];
        minute.fill(Tick::UNKNOWN);
        let raw = body[CHUNK_HEAD..].chunks_exact(TICK_BYTES);
        let mut made = true;
        for (tick, raw) in minute.iter_mut().zip(raw) {
            match read_tick(raw) {
                Some(t) => *tick = t,
                None => made = false,
            }
        }
        if !made {
            minute.fill(Tick::UNKNOWN);
        }
        track.made[index] = made;
    }
    track
}

/// The duration a file's header stores (None when the header can't be read): a finished file is read with it.
pub fn stored_duration(bytes: &[u8]) -> Option<Us> {
    read_header(bytes)
}

/// The chunks a pass still has to make.
pub fn missing(track: &ActivityTrack) -> Vec<u32> {
    let todo = track.made.iter().enumerate().filter(|(_, made)| !**made);
    todo.map(|(i, _)| i as u32).collect()
}

fn read_header(bytes: &[u8]) -> Option<Us> {
    let head = bytes.get(..HEADER_BYTES)?;
    let (body, sum) = head.split_at(HEADER_BYTES - 4);
    if &body[..8] != MAGIC || crc32fast::hash(body) != le_u32(sum) {
        return None;
    }
    let duration = Us::from_le_bytes(body[8..].try_into().ok()?);
    (0..=MAX_DURATION_US)
        .contains(&duration)
        .then_some(duration)
}

fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// A stored tick, or None if it can't be one (a box outside the grid or upside down): the chunk is then made again.
fn read_tick(b: &[u8]) -> Option<Tick> {
    let region = |b: &[u8]| {
        let r = Region {
            x0: b[0],
            y0: b[1],
            x1: b[2],
            y1: b[3],
            cells: u16::from_le_bytes([b[4], b[5]]),
        };
        let inside = r.x0 <= r.x1 && r.y0 <= r.y1 && usize::from(r.x1) < GRID.0;
        let fits = inside && usize::from(r.y1) < GRID.1 && u32::from(r.cells) <= r.area();
        match r.cells {
            0 => Some(Region::default()),
            _ => fits.then_some(r),
        }
    };
    let changed = u16::from_le_bytes([b[0], b[1]]);
    let known = usize::from(changed) <= GRID.0 * GRID.1 || changed == u16::MAX;
    let top = [region(&b[2..8])?, region(&b[8..14])?];
    known.then_some(Tick { changed, top })
}
