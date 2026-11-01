//! Rebuilding a video run's damaged head: the head carries no media, only the stream's description,
//! which the journal keeps (`RunStarted.video`). Our writer made the first head from the same `StreamInfo`, so
//! `Fmp4Writer::new(info).head()` is the same bytes, and the first fragment starts right after it.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use klipolo_platform::file::PositionedIo;

use crate::fmp4::Fmp4Writer;
use crate::fmp4::read::{self, Walk};
use crate::media::StreamInfo;

/// The head for `info` and the walk of the file with it in place, when that walk reads the head and at least one
/// fragment. Nothing is written: the walk reads through an overlay, so a head that doesn't fit the file is never
/// written over it.
pub(super) fn rebuild(
    file: &File,
    len: u64,
    info: &StreamInfo,
) -> io::Result<Option<(Vec<u8>, Walk)>> {
    let head = Fmp4Writer::new(info).head();
    if len <= head.len() as u64 {
        return Ok(None);
    }
    let mut overlay = Overlay {
        head: &head,
        file,
        len,
        at: 0,
    };
    let walk = read::walk(&mut overlay, len)?;
    let fits = walk.head.is_some() && !walk.index.fragments.is_empty();
    Ok(fits.then_some((head, walk)))
}

/// The file as it would read with `head` written over its first bytes.
struct Overlay<'a> {
    head: &'a [u8],
    file: &'a File,
    len: u64,
    at: u64,
}

impl Read for Overlay<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut n = (buf.len() as u64).min(self.len.saturating_sub(self.at)) as usize;
        if n == 0 {
            return Ok(0);
        }
        match usize::try_from(self.at)
            .ok()
            .filter(|&at| at < self.head.len())
        {
            Some(at) => {
                n = n.min(self.head.len() - at);
                buf[..n].copy_from_slice(&self.head[at..at + n]);
            }
            None => self.file.read_exact_at(&mut buf[..n], self.at)?,
        }
        self.at += n as u64;
        Ok(n)
    }
}

impl Seek for Overlay<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let at = match pos {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.at.checked_add_signed(d),
        };
        self.at = at.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start")
        })?;
        Ok(self.at)
    }
}
