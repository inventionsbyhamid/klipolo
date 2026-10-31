//! A video run's file in recovery: salvage every whole fragment, rebuild a damaged head, keep what it cuts, finish the
//! soft remux. The resumed recorder runs the same steps on its crashed runs.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::Path;

use klipolo_platform::durable::Durability;
use klipolo_platform::file::PositionedIo;

use super::damage::{self, Kept};
use super::{RunOutcome, RunRepair};
use super::{head, nal};
use crate::fmp4::finalize;
use crate::fmp4::read::{self, Walk};
use crate::io::Io;
use crate::journal::Entry;
use crate::media::StreamInfo;

/// A sample past the durable length bigger than this isn't checked or kept: ours are a few MB at most (a fragment
/// closes at 16 MiB).
const MAX_SAMPLE: u64 = 32 << 20;

/// Repairs the video run at `path` (journal key `key`). Steps:
/// 1. **Walk** the file. Without a head that reads, or with one that doesn't describe the run's `StreamInfo` from the
///    journal (garbled inside an intact `moov`), rebuild it: `Fmp4Writer::new(info).head()` is byte
///    for byte the head our writer made. It is written only once a walk with it in place finds the file's first
///    fragment right after it (`head::rebuild`); a head that reads but doesn't match, and can't be rebuilt, is kept.
/// 2. **Trust** every fragment inside the durable length `durable` (the journal's; None: nothing is known durable).
///    Past it, keep each fragment whose samples are whole NAL units (`nal::whole`), up to the first that isn't: a
///    power cut can land box headers without the media behind them, which the walk alone accepts.
/// 3. **Cut** at the end of the kept media, after copying the non-zero bytes past it (at most 64 MB) to
///    `<file>.damaged` (`damage::keep_tail`). Zeros, and the remux's own tail, aren't damage. A bigger non-zero tail
///    leaves the file untouched and the run out (None).
/// 4. **Remux** unless the walk already says `remuxed`: push the media to the disk, then journal its length as
///    durable (so the bound never cuts what is kept now, and never vouches for bytes that haven't landed), `write_tail`,
///    journal `Remux { done: false }`, `flip`, journal `Remux { done: true }`.
///    Each is idempotent, so a remux cut short is finished here. Then walk again: only a file the walk reads as
///    remuxed, with the same fragments, is measured (the import reads regular MP4s only).
///
/// `Empty`: no file, or nothing recorded past its head (or only zeros). `LeftOut` (logged; the manifest notes it): a
/// head that can't be rebuilt, no whole fragment, damage too big to keep, or a remux guard's refusal; the file is
/// kept as the step found it. An I/O error is returned, and a later repair goes on from where this one stopped. Every
/// write and sync goes through `io`.
pub fn repair_video(
    io: &dyn Io,
    path: &Path,
    key: &str,
    stream: Option<&StreamInfo>,
    durable: Option<u64>,
    journal: &mut dyn FnMut(&Entry) -> io::Result<()>,
) -> io::Result<RunOutcome> {
    repair_video_watched(io, path, key, stream, durable, journal, &mut |_| true)
}

/// `repair_video`, telling `step` how many bytes past the durable mark its check has read; when `step` returns false
/// the check stops with an `Interrupted` error (the repair was cancelled; nothing was written yet).
pub(crate) fn repair_video_watched(
    io: &dyn Io,
    path: &Path,
    key: &str,
    stream: Option<&StreamInfo>,
    durable: Option<u64>,
    journal: &mut dyn FnMut(&Entry) -> io::Result<()>,
    step: &mut dyn FnMut(u64) -> bool,
) -> io::Result<RunOutcome> {
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tracing::warn!(file = key, "recovery: a run's file is missing");
            return Ok(RunOutcome::Empty);
        }
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    let mut walk = read::walk(&mut &file, len)?;
    let mut new_head = None;
    let unlike = |w: &Walk| {
        w.head
            .as_ref()
            .zip(stream)
            .is_some_and(|(t, info)| t.info != *info)
    };
    if unlike(&walk) {
        tracing::warn!(
            file = key,
            "recovery: a head that doesn't match the journal's stream"
        );
        if let Some(info) = stream
            && let Some((bytes, rebuilt)) = head::rebuild(&file, len, info)?
        {
            (new_head, walk) = (Some(bytes), rebuilt);
        }
    }
    if walk.head.is_none() {
        let rebuilt = match stream {
            Some(info) => head::rebuild(&file, len, info)?,
            None => None,
        };
        let Some((bytes, rebuilt)) = rebuilt else {
            tracing::warn!(
                file = key,
                "recovery: a video run without a head that reads or one to rebuild"
            );
            return left_out(&file, 0, len);
        };
        (new_head, walk) = (Some(bytes), rebuilt);
    }
    let Some(track) = walk.head.clone() else {
        return Ok(RunOutcome::Empty);
    };
    let keep = trusted(&file, &walk, &track.info, durable.unwrap_or(0), step)?;
    if keep == 0 {
        tracing::warn!(file = key, "recovery: a video run without a whole fragment");
        let media = walk
            .index
            .fragments
            .first()
            .map_or(walk.valid_len, |f| f.moof_at);
        return left_out(&file, media, len);
    }
    let whole = keep == walk.index.fragments.len();
    let media_end = if whole {
        walk.valid_len
    } else {
        walk.index.fragments[keep].moof_at
    };
    let cut = !whole || walk.damaged_at.is_some();
    let mut damaged_bytes = 0;
    if cut {
        match damage::keep_tail(path, &file, media_end, len)? {
            Kept::Bytes(n) => damaged_bytes = n,
            Kept::TooBig(n) => {
                tracing::error!(
                    file = key,
                    bytes = n,
                    "recovery: the damage is too big to keep beside the file, so the run is left as it is"
                );
                return Ok(RunOutcome::LeftOut);
            }
        }
    }
    if let Some(bytes) = &new_head {
        io.write_at(&file, path, 0, bytes)?;
    }
    if cut {
        io.truncate(&file, path, media_end)?;
        walk.index.fragments.truncate(keep);
        (walk.valid_len, walk.damaged_at) = (media_end, None);
    }
    let changed = new_head.is_some() || cut || !walk.remuxed;
    if changed && !remux(io, &file, path, key, &walk, journal)? {
        return Ok(RunOutcome::LeftOut);
    }
    Ok(RunOutcome::Repaired(RunRepair {
        duration_us: walk.index.duration_us(track.timescale),
        damaged_bytes,
        changed,
    }))
}

/// A run that can't be imported: `LeftOut` when `[from, len)` holds something recorded, else `Empty` (nothing past
/// the head, or only zeros that never landed).
fn left_out(file: &File, from: u64, len: u64) -> io::Result<RunOutcome> {
    Ok(if damage::holds_data(file, from, len)? {
        RunOutcome::LeftOut
    } else {
        RunOutcome::Empty
    })
}

/// How many of the walk's fragments to keep: every one that ends inside `durable`, then each later one whose samples
/// are all whole NAL units, up to the first that isn't. Reads only the media past `durable`: at most about the last
/// second's (every sample without a journal), one sample at a time into one buffer, telling `step` after each fragment.
fn trusted(
    file: &File,
    walk: &Walk,
    info: &StreamInfo,
    durable: u64,
    step: &mut dyn FnMut(u64) -> bool,
) -> io::Result<usize> {
    let mut buf = Vec::new();
    let mut read = 0;
    for (i, f) in walk.index.fragments.iter().enumerate() {
        if f.data_end() <= durable {
            continue;
        }
        if !step(read) {
            return Err(io::Error::new(
                ErrorKind::Interrupted,
                "the repair was cancelled",
            ));
        }
        read += f.data_end() - f.data_at;
        let mut at = f.data_at;
        for s in &f.samples {
            let size = u64::from(s.size);
            if size > MAX_SAMPLE {
                return Ok(i);
            }
            buf.resize(size as usize, 0);
            file.read_exact_at(&mut buf, at)?;
            if !nal::whole(&buf, info) {
                return Ok(i);
            }
            at += size;
        }
    }
    Ok(walk.index.fragments.len())
}

/// The soft remux of the walked media (step 4). False: a guard refused a step, or the result doesn't read as remuxed.
fn remux(
    io: &dyn Io,
    file: &File,
    path: &Path,
    key: &str,
    walk: &Walk,
    journal: &mut dyn FnMut(&Entry) -> io::Result<()>,
) -> io::Result<bool> {
    let Some(plan) = finalize::plan(walk) else {
        return Ok(false);
    };
    // The tick's order: the media is on the disk before the line vouches for it. After a kill, the fragments kept past
    // the old mark may still be only in the page cache; a power cut after the line would otherwise leave it trusting
    // them unchecked.
    crate::io::sync(io, file, path, Durability::Ordered)?;
    journal(&Entry::Durable {
        through: BTreeMap::from([(key.to_owned(), walk.valid_len)]),
    })?;
    if refused(finalize::write_tail(io, file, path, &plan), key)? {
        return Ok(false);
    }
    journal(&Entry::Remux {
        file: key.to_owned(),
        done: false,
    })?;
    if refused(finalize::flip(io, file, path, &plan), key)? {
        return Ok(false);
    }
    journal(&Entry::Remux {
        file: key.to_owned(),
        done: true,
    })?;
    let check = read::walk(&mut &*file, file.metadata()?.len())?;
    if !check.remuxed || check.index != walk.index {
        tracing::error!(file = key, "recovery: the remux didn't read back as one");
        return Ok(false);
    }
    Ok(true)
}

/// A remux guard's refusal (InvalidData, nothing written) leaves the run out, logged; other errors go up.
fn refused(step: io::Result<()>, key: &str) -> io::Result<bool> {
    match step {
        Ok(()) => Ok(false),
        Err(e) if e.kind() == ErrorKind::InvalidData => {
            tracing::error!(file = key, error = %e, "recovery: the remux refused a step");
            Ok(true)
        }
        Err(e) => Err(e),
    }
}
