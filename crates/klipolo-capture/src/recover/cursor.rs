//! The cursor file after a crash: a line file, trusted to its durable length like the media (the
//! journal's `Durable` lines list it), and past that only the whole lines that read; the cut bytes that aren't zeros go
//! beside it as `cursor.jsonl.damaged` first (`damage::keep_tail`). Its shapes were written whole or not at all (a temp
//! file renamed in), so a missing PNG isn't damage: that shape draws as the Arrow look.
//!
//! Cost: the lines past the durable length are read once (normally the last second); none before it. Idempotent.

use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom};

use klipolo_cursor::format::{MAX_LINE, parse_line};
use klipolo_platform::durable::Durability;

use super::damage::{Kept, keep_tail};
use crate::io::{self as file_io, Io};
use crate::session::SessionDir;

/// Cuts `cursor.jsonl` after its last whole line that reads, from `durable` on (None: from the start). No file: Ok.
pub(crate) fn finish_cursor(io: &dyn Io, dir: &SessionDir, durable: Option<u64>) -> io::Result<()> {
    let path = dir.cursor();
    let file = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    let mut keep = durable.unwrap_or(0).min(len);
    let mut reader = BufReader::new(&file);
    reader.seek(SeekFrom::Start(keep))?;
    let mut line = Vec::with_capacity(MAX_LINE + 1);
    loop {
        line.clear();
        let n = (&mut reader)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if n == 0 || line.last() != Some(&b'\n') {
            break;
        }
        let body = line[..n - 1].trim_ascii();
        if !body.is_empty() && parse_line(body).is_none() {
            break;
        }
        keep += n as u64;
    }
    drop(reader);
    if keep == len {
        return Ok(());
    }
    match keep_tail(&path, &file, keep, len)? {
        Kept::TooBig(n) => {
            tracing::warn!(
                past = n,
                "recovery: the cursor file's tail is too long to be a crash's, so it is left as it is"
            );
            Ok(())
        }
        Kept::Bytes(kept) => {
            tracing::info!(
                cut = len - keep,
                kept,
                "recovery: the cursor file is cut after its last whole line"
            );
            io.truncate(&file, &path, keep)?;
            file_io::sync(io, &file, &path, Durability::Ordered)
        }
    }
}
