//! The soft remux at Stop: a fragmented run file becomes a regular MP4 without copying a byte of media.
//!
//! 1. `plan`: the tail `moov` (the whole sample table: `stts`, `stss`, `stsc` with one chunk per fragment, `stsz` and
//!    `co64` at each fragment's samples) and the flips: the head `moov`'s and every `moof`'s type field.
//! 2. `write_tail`: the tail goes after the media typed `free`, then a full sync. A crash now leaves the fragmented
//!    file as it was, with a trailing `free` every reader skips. A full disk, a kill or a power cut part-way through
//!    leaves part of that `free`: on an error `write_tail` cuts it off itself, and otherwise the walk recognises it as
//!    the remux's own (never damage) and a re-run replaces it.
//! 3. `flip`: checks the tail is in place and every flip lands on a `moov`, `moof` or `free` type, then writes `free`
//!    over each and `moov` over the tail's type, and syncs. The walk reads flipped boxes, so a crash part-way through
//!    leaves a file the next walk plans again, and the same three steps finish it.
//!
//! Every step is idempotent, and nothing is destroyed: the only bytes ever cut are a previous tail of the same kind,
//! whole or cut short. Their writes and syncs go through `Io` (`path` names the file in its log), so the power-loss
//! replay can cut any of them short. Costs for an hour at 60 fps: a tail of about 1-3 MB (built in memory once), 7,200 four-byte
//! reads and writes for the flips, and two full syncs.
//!
//! Memory at Stop is linear in the samples: the index (12 bytes a sample, 48 a fragment) plus the
//! tail, built whole in memory with 16 bytes a sample reserved. For 6 hours at 60 fps that is 17.6 MB of index and a
//! 21 MB reservation (about 6-16 MB used, depending on how much the durations vary); at 120 fps, about 33 MB and
//! 41 MB. Streaming `stts` and `stsz` into the file would remove the tail's share if that ever matters.

use std::fs::File;
use std::io;
use std::path::Path;

use klipolo_platform::file::PositionedIo;

use super::boxes::{be32, be64, full, patch32};
use super::moov::moov;
use super::read::{TrackInfo, Walk};
use super::{Fmp4Writer, Index, SampleIndex, TIMESCALE};
use crate::fail::fail_point;
use crate::io::Io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemuxPlan {
    /// Where the tail goes: the end of the media.
    pub tail_at: u64,
    /// The tail, typed `moov` (written typed `free` until the flips).
    pub tail: Vec<u8>,
    /// The type fields to turn into `free`: the head `moov`'s, then every `moof`'s.
    pub flips: Vec<u64>,
}

/// The plan for a walked file; None without a head or with no fragments.
pub fn plan(walk: &Walk) -> Option<RemuxPlan> {
    plan_parts(
        walk.head.as_ref()?,
        walk.head_moov_at?,
        &walk.index,
        walk.valid_len,
    )
}

impl Fmp4Writer {
    /// Stop's plan from the writer's own index, with no walk. It holds only while the file is exactly the head and
    /// every fragment emitted (`bytes_written` bytes): only when the durable writer's last snapshot says
    /// `wrote_exactly(key, bytes_written())`. After a lost or failed write the file holds less, so Stop walks it instead;
    /// `write_tail` refuses a shorter file anyway.
    pub fn remux_plan(&self) -> Option<RemuxPlan> {
        let track = TrackInfo {
            info: self.info().clone(),
            timescale: TIMESCALE,
        };
        plan_parts(
            &track,
            self.head_moov_at(),
            self.index(),
            self.bytes_written(),
        )
    }
}

fn plan_parts(
    track: &TrackInfo,
    head_moov_at: u64,
    index: &Index,
    tail_at: u64,
) -> Option<RemuxPlan> {
    if index.fragments.is_empty() {
        return None;
    }
    let samples: usize = index.fragments.iter().map(|f| f.samples.len()).sum();
    let duration = all(index).map(|s| u64::from(s.duration)).sum();
    // stts at worst 8 bytes a sample, stsz 4, stss 4 at most; stsc 12 and co64 8 a fragment.
    let mut tail = Vec::with_capacity(
        1024 + track.info.config.len() + samples * 16 + index.fragments.len() * 20,
    );
    moov(
        &mut tail,
        &track.info,
        track.timescale,
        duration,
        false,
        |o| tables(o, index, samples),
    );
    let mut flips = Vec::with_capacity(1 + index.fragments.len());
    flips.push(head_moov_at + 4);
    flips.extend(index.fragments.iter().map(|f| f.moof_at + 4));
    Some(RemuxPlan {
        tail_at,
        tail,
        flips,
    })
}

fn all(index: &Index) -> impl Iterator<Item = &SampleIndex> {
    index.fragments.iter().flat_map(|f| &f.samples)
}

/// Sample durations as (count, duration) runs.
fn runs(index: &Index) -> impl Iterator<Item = (u32, u32)> {
    let mut durations = all(index).map(|s| s.duration).peekable();
    std::iter::from_fn(move || {
        let d = durations.next()?;
        let mut n = 1;
        while durations.next_if_eq(&d).is_some() {
            n += 1;
        }
        Some((n, d))
    })
}

/// `stbl`'s tables after `stsd`.
fn tables(o: &mut Vec<u8>, index: &Index, samples: usize) {
    // Decode durations, run-length coded.
    full(o, b"stts", 0, 0, |o| {
        let at = o.len();
        be32(o, 0);
        let mut entries = 0u32;
        for (n, d) in runs(index) {
            be32(o, n);
            be32(o, d);
            entries += 1;
        }
        patch32(o, at, entries);
    });
    // Sync samples, numbered from 1.
    full(o, b"stss", 0, 0, |o| {
        let at = o.len();
        be32(o, 0);
        let mut n = 0;
        for (i, _) in all(index).enumerate().filter(|(_, s)| s.key) {
            be32(o, i as u32 + 1);
            n += 1;
        }
        patch32(o, at, n);
    });
    // One chunk per fragment; an entry only where the samples per chunk change.
    full(o, b"stsc", 0, 0, |o| {
        let at = o.len();
        be32(o, 0);
        let (mut n, mut last) = (0, None);
        for (i, f) in index.fragments.iter().enumerate() {
            let per = f.samples.len() as u32;
            if last != Some(per) {
                for v in [i as u32 + 1, per, 1] {
                    be32(o, v);
                }
                (n, last) = (n + 1, Some(per));
            }
        }
        patch32(o, at, n);
    });
    full(o, b"stsz", 0, 0, |o| {
        be32(o, 0);
        be32(o, samples as u32);
        for s in all(index) {
            be32(o, s.size);
        }
    });
    full(o, b"co64", 0, 0, |o| {
        be32(o, index.fragments.len() as u32);
        for f in &index.fragments {
            be64(o, f.data_at);
        }
    });
}

/// Puts the tail after the media typed `free` and syncs it to the disk. Idempotent: a tail already in place is only
/// synced again. Refuses (InvalidData) a file shorter than `tail_at`, and bytes past it that aren't one earlier tail,
/// whole or cut short: damage is saved by recovery before it truncates, never cut here. On a write or sync error it
/// cuts the file back to the media (best effort), so nothing half-written stays behind.
pub fn write_tail(io: &dyn Io, file: &File, path: &Path, plan: &RemuxPlan) -> io::Result<()> {
    let len = file.metadata()?.len();
    if len == tail_end(plan) && tail_in_place(file, plan)? {
        return io.barrier(file, path);
    }
    if len < plan.tail_at {
        return Err(invalid("the run file is shorter than its media"));
    }
    if len > plan.tail_at && !one_tail(file, plan.tail_at, len)? {
        return Err(invalid("bytes past the media that aren't a remux tail"));
    }
    io.truncate(file, path, plan.tail_at)?;
    let mut header = [0u8; 8];
    header[..4].copy_from_slice(&plan.tail[..4]);
    header[4..].copy_from_slice(b"free");
    let written = io
        .write_at(file, path, plan.tail_at, &header)
        .and_then(|()| io.write_at(file, path, plan.tail_at + 8, &plan.tail[8..]))
        .and_then(|()| io.barrier(file, path));
    if written.is_err() {
        let _ = io.truncate(file, path, plan.tail_at);
        return written;
    }
    fail_point!("remux.afterTail")
}

/// Turns the head `moov` and every `moof` into `free` and the tail into `moov`, then syncs. Checks first that the tail
/// is in place, whole and last, and that every flip lands on a type it may change (its own, `free`, or a flip a power
/// cut tore between them); else InvalidData, with nothing written. Idempotent.
pub fn flip(io: &dyn Io, file: &File, path: &Path, plan: &RemuxPlan) -> io::Result<()> {
    if file.metadata()?.len() != tail_end(plan) || !tail_in_place(file, plan)? {
        return Err(invalid("the remux tail isn't in place"));
    }
    let mut kind = [0u8; 4];
    for (i, &at) in plan.flips.iter().enumerate() {
        file.read_exact_at(&mut kind, at)?;
        let expected: &[u8; 4] = if i == 0 { b"moov" } else { b"moof" };
        if !flipped_from(&kind, expected) {
            return Err(invalid("a remux flip doesn't land on a box type"));
        }
    }
    for (i, &at) in plan.flips.iter().enumerate() {
        if i == plan.flips.len() / 2 {
            fail_point!("remux.midFlips")?;
        }
        io.write_at(file, path, at, b"free")?;
    }
    io.write_at(file, path, plan.tail_at + 4, b"moov")?;
    io.barrier(file, path)
}

fn tail_end(plan: &RemuxPlan) -> u64 {
    plan.tail_at + plan.tail.len() as u64
}

/// The plan's tail is at `tail_at`, typed `free` or `moov`: compared in 64 KB pieces.
fn tail_in_place(file: &File, plan: &RemuxPlan) -> io::Result<bool> {
    let piece = plan.tail.len().clamp(1, 64 * 1024);
    let mut buf = vec![0u8; piece];
    for (i, want) in plan.tail.chunks(piece).enumerate() {
        let got = &mut buf[..want.len()];
        let at = plan.tail_at + (i * piece) as u64;
        match file.read_exact_at(got, at) {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(false),
            r => r?,
        }
        let same = match i {
            0 => {
                got[..4] == want[..4] && flipped_from(&got[4..8], b"moov") && got[8..] == want[8..]
            }
            _ => got == want,
        };
        if !same {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `[at, len)` is one earlier remux tail, whole or cut short (`tail_starts`).
fn one_tail(file: &File, at: u64, len: u64) -> io::Result<bool> {
    let mut h = [0u8; 16];
    let n = (len - at).min(16) as usize;
    file.read_exact_at(&mut h[..n], at)?;
    Ok(tail_starts(&h[..n], len - at))
}

/// `b`, the first 16 bytes of a stretch of `left` bytes (fewer when it is shorter), starts a remux tail, whole or cut
/// short: a box typed `free` (as `write_tail` writes it) or `moov` (flipped, or torn between them: `flipped_from`) whose
/// size reaches the end of the stretch
/// or runs past it, and whose first child is `mvhd` when its header is there.
pub(crate) fn tail_starts(b: &[u8], left: u64) -> bool {
    b.len() >= 8
        && flipped_from(&b[4..8], b"moov")
        && u64::from(u32::from_be_bytes(b[..4].try_into().expect("4 bytes"))) >= left
        && (b.len() < 16 || &b[12..16] == b"mvhd")
}

/// `kind` is `from`, `free`, or a flip between them torn by a power cut: each byte is one or the other's. A flip is a
/// four-byte write, and one that straddles a disk sector can land in part (`fr` + `of`: `frof`). The box's first
/// child (`mvhd`, `mfhd`) still says what it is, so a torn flip is read as unfinished, never as damage.
pub(crate) fn flipped_from(kind: &[u8], from: &[u8; 4]) -> bool {
    kind.len() == 4
        && kind
            .iter()
            .zip(from.iter().zip(b"free"))
            .all(|(k, (a, b))| k == a || k == b)
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}
