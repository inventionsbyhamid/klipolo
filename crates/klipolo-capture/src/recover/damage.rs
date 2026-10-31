//! Keeping what recovery cuts: the bytes past a video run's damage that aren't zeros go beside the file as
//! `<file>.damaged` before the cut, so nothing the person recorded is destroyed (a later tool, or support, can look at
//! them). Zeros are pages that never reached the disk, so they aren't kept.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use klipolo_platform::durable::{self, Durability};
use klipolo_platform::file::PositionedIo;

/// The most damage kept beside a file. A power cut damages at most the last second or two written (a few MB); more
/// than this is footage after corruption, which recovery won't cut (the run is left as it is).
const MAX_DAMAGED: u64 = 64 << 20;
/// Copies and comparisons go in pieces of this size: one buffer.
const PIECE: u64 = 1 << 20;
/// `<file>.damaged`, then `<file>.damaged-2` … up to this, when an earlier, different one is there.
const NAMES: u32 = 99;

pub(super) enum Kept {
    /// These non-zero bytes were kept (0: the tail is all zeros, or there is none).
    Bytes(u64),
    /// The tail's non-zero bytes reach this far past its start: too many to keep. Nothing was written.
    TooBig(u64),
}

/// Keeps `[from, len)` of `file` (at `path`), up to its last non-zero byte, as `<file>.damaged`: a temporary file
/// beside it, copied, fully synced, then renamed in. A `.damaged` already holding these bytes (an earlier repair cut
/// short before its cut) is kept as it is; a different one is never replaced, the next free name is used instead.
pub(super) fn keep_tail(path: &Path, file: &File, from: u64, len: u64) -> io::Result<Kept> {
    let end = last_nonzero_end(file, from, len)?;
    let n = end - from;
    if n == 0 {
        return Ok(Kept::Bytes(0));
    }
    if n > MAX_DAMAGED {
        return Ok(Kept::TooBig(n));
    }
    let mut buf = vec![0u8; PIECE.min(n) as usize];
    for i in 1..=NAMES {
        let target = name(path, i);
        match File::open(&target) {
            Ok(old) if same(&old, file, from, n, &mut buf)? => return Ok(Kept::Bytes(n)),
            Ok(_) => continue,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                copy_out(file, from, n, &target, &mut buf)?;
                return Ok(Kept::Bytes(n));
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "too many .damaged files beside this run",
    ))
}

fn name(path: &Path, i: u32) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(if i == 1 {
        ".damaged".to_owned()
    } else {
        format!(".damaged-{i}")
    });
    path.with_file_name(name)
}

/// `[from, len)` holds a byte that isn't zero: something was recorded there (zeros are pages that never landed).
pub(super) fn holds_data(file: &File, from: u64, len: u64) -> io::Result<bool> {
    Ok(len > from && last_nonzero_end(file, from, len)? > from)
}

/// One past the last non-zero byte of `[from, len)`, or `from` when it is all zeros: read backwards in 64 KiB pieces,
/// so a non-zero end is found at once.
fn last_nonzero_end(file: &File, from: u64, len: u64) -> io::Result<u64> {
    let mut buf = vec![0u8; (len - from).min(64 * 1024) as usize];
    let mut end = len;
    while end > from {
        let n = (end - from).min(buf.len() as u64);
        let piece = &mut buf[..n as usize];
        file.read_exact_at(piece, end - n)?;
        if let Some(i) = piece.iter().rposition(|&b| b != 0) {
            return Ok(end - n + i as u64 + 1);
        }
        end -= n;
    }
    Ok(from)
}

/// `old` holds exactly `n` bytes, equal to `file`'s from `from`.
fn same(old: &File, file: &File, from: u64, n: u64, buf: &mut [u8]) -> io::Result<bool> {
    if old.metadata()?.len() != n {
        return Ok(false);
    }
    let mut theirs = vec![0u8; buf.len()];
    let mut at = 0;
    while at < n {
        let k = (n - at).min(buf.len() as u64) as usize;
        file.read_exact_at(&mut buf[..k], from + at)?;
        old.read_exact_at(&mut theirs[..k], at)?;
        if buf[..k] != theirs[..k] {
            return Ok(false);
        }
        at += k as u64;
    }
    Ok(true)
}

fn copy_out(file: &File, from: u64, n: u64, target: &Path, buf: &mut [u8]) -> io::Result<()> {
    let tmp = durable::temp_path(target);
    let copied = (|| {
        let mut out = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        let mut at = 0;
        while at < n {
            let k = (n - at).min(buf.len() as u64) as usize;
            file.read_exact_at(&mut buf[..k], from + at)?;
            out.write_all(&buf[..k])?;
            at += k as u64;
        }
        durable::sync(&out, Durability::Full)?;
        drop(out);
        std::fs::rename(&tmp, target)
    })();
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    match target.parent() {
        Some(dir) => durable::fsync_dir(dir),
        None => Ok(()),
    }
}
