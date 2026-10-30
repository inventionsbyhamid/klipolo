//! One face of a font collection (`.ttc`, `.otc`) as a standalone font: WebKit's `FontFace` takes a
//! single font, so an installed face inside a collection (Avenir Next, Hiragino) is sent as its own sfnt. A plain font
//! is answered as it is, without a copy.
//!
//! Every read goes through `get`, which bounds-checks the range against the file: a collection's offsets and lengths
//! come from the file, never trusted.
//!
//! A face is read from a memory map, never into memory whole, and a face over `MAX_FACE_BYTES` is refused before any
//! byte is copied (Apple Color Emoji's faces are 180 MB each).

use std::borrow::Cow;
use std::ops::Range;

/// Why a face couldn't be taken out of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SfntError {
    /// A header, a table directory or a table runs past the end of the file.
    Truncated,
    /// The file has no face at this index (or its directory is unreasonable: more than 64 tables).
    NotAFace(u32),
    /// The face is bigger than the cap (its size in bytes): never read or sent.
    TooLarge(u64),
    /// The file couldn't be opened or mapped.
    Unreadable,
}

/// The most bytes one face may have to be read for the webview: the downloads' cap.
pub const MAX_FACE_BYTES: u64 = 50 * 1024 * 1024;

impl std::fmt::Display for SfntError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SfntError::Truncated => f.write_str("the font file is cut short"),
            SfntError::NotAFace(i) => write!(f, "the font file has no face {i}"),
            SfntError::TooLarge(n) => write!(f, "the face is {n} bytes, over the cap"),
            SfntError::Unreadable => f.write_str("the font file can't be read"),
        }
    }
}

impl std::error::Error for SfntError {}

/// The most tables one face's directory may list (real fonts have about 10 to 30).
const MAX_TABLES: usize = 64;
/// An sfnt header: version, table count, search range, entry selector, range shift.
const HEADER: usize = 12;
/// One table record: tag, checksum, offset, length.
const RECORD: usize = 16;

fn get(file: &[u8], range: Range<usize>) -> Result<&[u8], SfntError> {
    file.get(range).ok_or(SfntError::Truncated)
}

fn u16_at(file: &[u8], at: usize) -> Result<u16, SfntError> {
    let b = get(file, at..at.checked_add(2).ok_or(SfntError::Truncated)?)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

fn u32_at(file: &[u8], at: usize) -> Result<u32, SfntError> {
    let b = get(file, at..at.checked_add(4).ok_or(SfntError::Truncated)?)?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// The sfnt bytes of face `index` in `file`: the file itself when it isn't a collection (then `index` must be 0), else
/// a new font holding that face's header and tables, each table copied at a 4-byte-aligned offset with its checksum.
pub fn face_bytes(file: &[u8], index: u32) -> Result<Cow<'_, [u8]>, SfntError> {
    if get(file, 0..4)? != b"ttcf" {
        return if index == 0 {
            Ok(Cow::Borrowed(file))
        } else {
            Err(SfntError::NotAFace(index))
        };
    }
    let count = u32_at(file, 8)?;
    if index >= count {
        return Err(SfntError::NotAFace(index));
    }
    let start = u32_at(file, 12 + 4 * index as usize)? as usize;
    let tables = u16_at(file, start.checked_add(4).ok_or(SfntError::Truncated)?)? as usize;
    if tables > MAX_TABLES {
        return Err(SfntError::NotAFace(index));
    }
    let header = get(file, start..start + HEADER)?;
    let records = get(file, start + HEADER..start + HEADER + RECORD * tables)?;
    // The tables, each checked against the file before anything is written.
    let mut bodies: Vec<(&[u8], &[u8])> = Vec::with_capacity(tables);
    for record in records.chunks_exact(RECORD) {
        let offset = u32_at(record, 8)? as usize;
        let length = u32_at(record, 12)? as usize;
        let end = offset.checked_add(length).ok_or(SfntError::Truncated)?;
        bodies.push((&record[..8], get(file, offset..end)?));
    }
    let data_start = HEADER + RECORD * tables;
    let total = data_start
        + bodies
            .iter()
            .map(|(_, b)| b.len().next_multiple_of(4))
            .sum::<usize>();
    // One face's tables never overlap, so a face is never bigger than its file: a directory pointing every table at
    // the whole file is refused rather than copied 64 times.
    if total > file.len() + data_start + 4 * tables {
        return Err(SfntError::NotAFace(index));
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(header);
    let mut at = data_start;
    for (tag_and_checksum, body) in &bodies {
        out.extend_from_slice(tag_and_checksum);
        out.extend_from_slice(&(at as u32).to_be_bytes());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        at += body.len().next_multiple_of(4);
    }
    for (_, body) in &bodies {
        out.extend_from_slice(body);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    Ok(Cow::Owned(out))
}

/// The size `face_bytes` would answer for face `index` of `file`, without copying anything: the file's own length, or
/// for a collection face, its header, directory and tables (each 4-byte aligned).
pub fn face_len(file: &[u8], index: u32) -> Result<u64, SfntError> {
    if get(file, 0..4)? != b"ttcf" {
        return if index == 0 {
            Ok(file.len() as u64)
        } else {
            Err(SfntError::NotAFace(index))
        };
    }
    if index >= u32_at(file, 8)? {
        return Err(SfntError::NotAFace(index));
    }
    let start = u32_at(file, 12 + 4 * index as usize)? as usize;
    let tables = u16_at(file, start.checked_add(4).ok_or(SfntError::Truncated)?)? as usize;
    if tables > MAX_TABLES {
        return Err(SfntError::NotAFace(index));
    }
    let records = get(file, start + HEADER..start + HEADER + RECORD * tables)?;
    let mut total = (HEADER + RECORD * tables) as u64;
    for record in records.chunks_exact(RECORD) {
        total += u64::from(u32_at(record, 12)?).next_multiple_of(4);
    }
    Ok(total)
}

/// Face `index` of the font file at `path`, as its own font, read from a memory map: refused with `TooLarge` when it
/// is over `cap`, before any byte is copied.
pub fn read_face(path: &std::path::Path, index: u32, cap: u64) -> Result<Vec<u8>, SfntError> {
    let file = std::fs::File::open(path).map_err(|_| SfntError::Unreadable)?;
    // SAFETY: the map is read-only and dropped before this returns; every read of it is bounds-checked (`get`). A
    // font file changed by another process while it is read gives wrong bytes, which the webview's `FontFace` refuses,
    // as fontdb's own maps accept.
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|_| SfntError::Unreadable)?;
    let len = face_len(&map, index)?;
    if len > cap {
        return Err(SfntError::TooLarge(len));
    }
    Ok(face_bytes(&map, index)?.into_owned())
}
