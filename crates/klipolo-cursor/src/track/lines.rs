//! Lines of a cursor file through one reused buffer.

use std::io::{self, BufRead};

use crate::format::MAX_LINE;

/// Each line of `r` without its newline, through one buffer of `MAX_LINE`, to `f`. A longer line is skipped without
/// being kept. Err when reading fails part-way (the lines before it were given).
pub(super) fn each_line(mut r: impl BufRead, mut f: impl FnMut(&[u8])) -> io::Result<()> {
    let mut buf = Vec::with_capacity(MAX_LINE);
    while let Some(whole) = next_line(&mut r, &mut buf)? {
        if whole {
            f(&buf);
        }
    }
    Ok(())
}

/// The next line of `r` into `buf`, without its newline: Some(true) for a line, Some(false) for one longer than
/// `MAX_LINE` (skipped, not kept), None at the end.
fn next_line(r: &mut impl BufRead, buf: &mut Vec<u8>) -> io::Result<Option<bool>> {
    buf.clear();
    let (mut long, mut any) = (false, false);
    loop {
        let chunk = match r.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(any.then_some(!long));
        }
        any = true;
        let end = chunk.iter().position(|&b| b == b'\n');
        let part = &chunk[..end.unwrap_or(chunk.len())];
        if !long && buf.len() + part.len() > MAX_LINE {
            long = true;
            buf.clear();
        }
        if !long {
            buf.extend_from_slice(part);
        }
        let used = end.map_or(chunk.len(), |i| i + 1);
        r.consume(used);
        if end.is_some() {
            return Ok(Some(!long));
        }
    }
}
