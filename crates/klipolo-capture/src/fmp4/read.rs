//! Reading a run file back: `walk` goes over the top-level boxes and keeps every complete `moof`+`mdat` pair before
//! damage (salvage, recovery and the remux use it); `samples` reads a whole file in memory (tests and fixtures).
//!
//! The walk reads only box headers, `moof`s and the head's `trak`: three small reads a fragment (about 22,000 for an
//! hour at 7,200 fragments, four once remuxed) and the index's 2.9 MB, never the media.

use std::fmt;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

use super::boxes::{Rd, children};
use super::finalize::{flipped_from, tail_starts};
use super::parse::{Track, moof_index, trak};
use super::{FragIndex, Index};
use crate::media::StreamInfo;

/// A `moof` or a head `trak` bigger than these is damage, never read into memory (ours are about 400 bytes and 1 KB).
const MAX_MOOF: u64 = 1 << 20;
const MAX_TRAK: u64 = 4 << 20;
/// A `moof` claiming more samples than this (about what 1 MB of 16-byte `trun` entries holds), or more than its `mdat`
/// has bytes, is damage: a corrupt `trun` never makes recovery reserve more than 768 KB.
const MAX_MOOF_SAMPLES: u64 = 1 << 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    pub info: StreamInfo,
    pub timescale: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Walk {
    /// The head's video track: from the head `moov`, which the remux flips to `free`.
    pub head: Option<TrackInfo>,
    /// The head `moov`'s first byte.
    pub head_moov_at: Option<u64>,
    /// Every complete fragment before damage, flipped ones included.
    pub index: Index,
    /// The end of the media: the head and every complete fragment (and boxes that are skipped, such as FFmpeg's
    /// `mfra`). Past it there is nothing, a remux tail, whole or cut short (which the remux rebuilds), or damage.
    pub valid_len: u64,
    /// Where the bytes the walk couldn't use begin (always `valid_len`). Recovery saves them before truncating.
    pub damaged_at: Option<u64>,
    /// The soft remux finished: a trailing `moov` without `mvex`, with the head and every `moof` flipped to `free`.
    pub remuxed: bool,
}

#[derive(Debug)]
pub enum ReadError {
    /// No head `moov` the walk could read.
    NoHead,
    /// Damage at this offset.
    Damaged {
        at: u64,
    },
    Io(io::Error),
}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHead => write!(f, "no MP4 head"),
            Self::Damaged { at } => write!(f, "MP4 damaged at byte {at}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Walks the first `len` bytes. Top-level boxes:
/// - `ftyp`, and `free`, `skip`, `mfra`, `sidx`, `styp`, `prft`, `emsg`, `uuid`, and an `mdat` before the head, are
///   skipped;
/// - the first `moov`-shaped box (starting with `mvhd`, typed `moov`, or `free` once flipped) is the head;
/// - a `moof`, or a `free` starting with `mfhd` (a flipped one), counts only with its `mdat` wholly inside `len`;
/// - a flip a power cut tore (a type mixing the box's own and `free`'s bytes, `finalize::flipped_from`) is read as the
///   box, not flipped yet, so the remux runs again;
/// - a later `moov`-shaped box is a remux tail and ends the media; anything after it is damage;
/// - so is the remux's own tail cut short by a full disk, a kill or a power cut in `write_tail` (`tail_starts`): it
///   ends the media without damage, and a re-run of the remux replaces it;
/// - a lone `mdat` after the head is damage: its `moof` is garbled, flipped or not, so going on would leave a hole
///   whose missing frames later ones refer to;
/// - a box of size 0, a size under its header or past `len`, or any other type ends the walk at `damaged_at`.
pub fn walk(file: &mut (impl Read + Seek), len: u64) -> io::Result<Walk> {
    let mut w = Walk::default();
    let mut track = Track::default();
    let (mut at, mut next_dts) = (0u64, 0u64);
    let (mut tail, mut tail_moov, mut unflipped, mut damaged) = (false, false, false, false);
    while at < len {
        // Nothing may follow a remux tail.
        let Some(h) = header(file, at, len)?.filter(|_| !tail) else {
            damaged = tail
                || w.head.is_none()
                || w.index.fragments.is_empty()
                || !torn_tail(file, at, len)?;
            break;
        };
        // What a `moov`, or a box the remux flipped (`free`, or a flip a power cut tore), holds: its first child's type.
        let (moof, moov) = (
            flipped_from(&h.kind, b"moof"),
            flipped_from(&h.kind, b"moov"),
        );
        let first = if (moof || moov || &h.kind == b"skip") && &h.kind != b"moof" {
            header(file, h.body, h.end)?.map(|c| c.kind)
        } else {
            None
        };
        if &h.kind == b"moof" || (first == Some(*b"mfhd") && &h.kind != b"moov") {
            let Some((frag, end)) = fragment(file, &h, len, &track, next_dts)? else {
                damaged = true;
                break;
            };
            next_dts = frag.first_dts
                + frag
                    .samples
                    .iter()
                    .map(|s| u64::from(s.duration))
                    .sum::<u64>();
            unflipped |= moof && &h.kind != b"free";
            w.index.fragments.push(frag);
            at = end;
        } else if first == Some(*b"mvhd") {
            if w.head.is_none() && w.index.fragments.is_empty() {
                let Some((info, t)) = head(file, &h)? else {
                    damaged = true;
                    break;
                };
                (w.head, w.head_moov_at, track) = (Some(info), Some(h.at), t);
                unflipped |= moov && &h.kind != b"free";
                at = h.end;
            } else {
                tail = true;
                tail_moov =
                    &h.kind == b"moov" && !kids(file, &h)?.iter().any(|c| &c.kind == b"mvex");
                at = h.end;
                continue;
            }
        } else if matches!(
            &h.kind,
            b"ftyp" | b"free" | b"skip" | b"mfra" | b"sidx" | b"styp" | b"prft" | b"emsg" | b"uuid"
        ) || (&h.kind == b"mdat" && w.head.is_none())
        {
            at = h.end;
        } else {
            damaged = true;
            break;
        }
        w.valid_len = at;
    }
    w.damaged_at = damaged.then_some(w.valid_len);
    w.remuxed = tail_moov && !unflipped && w.head.is_some();
    Ok(w)
}

/// Every sample of an in-memory file, as (decode time, duration, key, bytes) in the track's timescale. Damage is an
/// error. For tests and fixtures; recovery walks the file instead.
#[allow(clippy::type_complexity)]
pub fn samples(bytes: &[u8]) -> Result<(TrackInfo, Vec<(u64, u32, bool, Vec<u8>)>), ReadError> {
    let walk = walk(&mut Cursor::new(bytes), bytes.len() as u64)?;
    if let Some(at) = walk.damaged_at {
        return Err(ReadError::Damaged { at });
    }
    let track = walk.head.ok_or(ReadError::NoHead)?;
    let mut out = Vec::with_capacity(walk.index.fragments.iter().map(|f| f.samples.len()).sum());
    for f in &walk.index.fragments {
        let (mut dts, mut at) = (f.first_dts, f.data_at as usize);
        for s in &f.samples {
            out.push((
                dts,
                s.duration,
                s.key,
                bytes[at..at + s.size as usize].to_vec(),
            ));
            dts += u64::from(s.duration);
            at += s.size as usize;
        }
    }
    Ok((track, out))
}

struct Header {
    kind: [u8; 4],
    at: u64,
    body: u64,
    end: u64,
}

/// The box at `at`, if its header reads and it ends by `limit`; None for a size of 0 or under its header.
fn header(file: &mut (impl Read + Seek), at: u64, limit: u64) -> io::Result<Option<Header>> {
    let left = limit.saturating_sub(at);
    if left < 8 {
        return Ok(None);
    }
    let mut b = [0u8; 16];
    let n = if left >= 16 { 16 } else { 8 };
    if !read_at(file, at, &mut b[..n])? {
        return Ok(None);
    }
    let kind = b[4..8].try_into().expect("4 bytes");
    let (header, size) = match u32::from_be_bytes(b[..4].try_into().expect("4 bytes")) {
        0 => return Ok(None),
        1 if n == 16 => (
            16,
            u64::from_be_bytes(b[8..16].try_into().expect("8 bytes")),
        ),
        1 => return Ok(None),
        s => (8, u64::from(s)),
    };
    match at.checked_add(size) {
        Some(end) if size >= header && end <= limit => Ok(Some(Header {
            kind,
            at,
            body: at + header,
            end,
        })),
        _ => Ok(None),
    }
}

/// `[at, len)` starts with the remux's own tail cut short: see `tail_starts`.
fn torn_tail(file: &mut (impl Read + Seek), at: u64, len: u64) -> io::Result<bool> {
    let mut b = [0u8; 16];
    let n = len.saturating_sub(at).min(16) as usize;
    Ok(read_at(file, at, &mut b[..n])? && tail_starts(&b[..n], len - at))
}

/// Fills `buf` from `at`; false when the file ends first.
fn read_at(file: &mut (impl Read + Seek), at: u64, buf: &mut [u8]) -> io::Result<bool> {
    file.seek(SeekFrom::Start(at))?;
    match file.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

/// A box's body, when it is at most `max` bytes and the file holds it.
fn body(file: &mut (impl Read + Seek), h: &Header, max: u64) -> io::Result<Option<Vec<u8>>> {
    if h.end - h.body > max {
        return Ok(None);
    }
    let mut buf = vec![0; (h.end - h.body) as usize];
    Ok(read_at(file, h.body, &mut buf)?.then_some(buf))
}

/// A box's children, read header by header (a tail `moov` of megabytes costs a few reads).
fn kids(file: &mut (impl Read + Seek), h: &Header) -> io::Result<Vec<Header>> {
    let (mut out, mut at) = (Vec::new(), h.body);
    while at < h.end && out.len() < 64 {
        let Some(c) = header(file, at, h.end)? else {
            break;
        };
        at = c.end;
        out.push(c);
    }
    Ok(out)
}

/// The head: its video track and `trex` defaults.
fn head(file: &mut (impl Read + Seek), h: &Header) -> io::Result<Option<(TrackInfo, Track)>> {
    let (mut found, mut trex) = (None, None);
    for c in kids(file, h)? {
        match &c.kind {
            b"trak" if found.is_none() => found = body(file, &c, MAX_TRAK)?.and_then(|b| trak(&b)),
            b"mvex" => trex = body(file, &c, MAX_TRAK)?,
            _ => {}
        }
    }
    let Some((info, id)) = found else {
        return Ok(None);
    };
    let mut track = Track {
        id: Some(id),
        ..Track::default()
    };
    for (kind, t) in children(trex.as_deref().unwrap_or_default()) {
        let mut r = Rd::new(t);
        if &kind == b"trex" && r.version_flags().is_some() && r.u32() == Some(id) {
            r.u32();
            track.duration = r.u32().unwrap_or(0);
            track.size = r.u32().unwrap_or(0);
            track.flags = r.u32().unwrap_or(0);
        }
    }
    Ok(Some((info, track)))
}

/// A `moof` and its `mdat`, when both are whole and the samples lie inside the `mdat`: the fragment and the `mdat`'s
/// end.
fn fragment(
    file: &mut (impl Read + Seek),
    h: &Header,
    len: u64,
    track: &Track,
    next_dts: u64,
) -> io::Result<Option<(FragIndex, u64)>> {
    let Some(mdat) = header(file, h.end, len)?.filter(|m| &m.kind == b"mdat") else {
        return Ok(None);
    };
    let Some(moof) = body(file, h, MAX_MOOF)? else {
        return Ok(None);
    };
    let max_samples = (mdat.end - mdat.body).min(MAX_MOOF_SAMPLES);
    Ok(moof_index(&moof, h.at, track, next_dts, max_samples)
        .filter(|f| f.data_at >= mdat.body && f.data_end() <= mdat.end)
        .map(|f| (f, mdat.end)))
}
