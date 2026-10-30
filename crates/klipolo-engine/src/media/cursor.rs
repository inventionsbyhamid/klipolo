//! Recorded cursor data: a recording's `cursor.jsonl` opened and read into a
//! `klipolo_cursor::CursorTrack`, the take built from it (`TakeCursor`: its facts with its screen activity, its shape
//! images' checks, its smoothed paths and its follow-crop paths), and the engine's cache of takes (`tracks`). The cursor
//! layer draws a take, Auto zooms plan from its facts and raw track (`plan::zoom::camera`), and the follow-crop samples
//! its follow paths.
//!
//! The file's lines, their checks, the decimation and the memory bound are `klipolo_cursor`'s (format 2, and format 1 for
//! takes recorded before it). This module keeps only the IO: the regular-file check, the open and the 64 KB reader.
//! What the read passed over (lines out of order, a different header, the size limit, a read error part-way) is
//! logged once per load.
//!
//! A missing, empty or unreadable file is "no cursor data": no cursor is drawn over the take, and its Auto zooms stay
//! on the centre. It is never an error that stops playback (export asks). A damaged file keeps the lines it can
//! read. Only a regular file is read: a path to a pipe or a device (a crafted project) is refused before it is opened.
//!
//! **Memory:** 8 bytes a position (`CursorTrack::bytes`), at most `CursorTrack::MAX_BYTES` a track, plus 4 bytes a step
//! of motion for each smoothed path; the engine's takes share a byte budget (`tracks::BUDGET_BYTES`).
//!
//! A shape whose image turns out not to decode is refused on its take (`refuse`), so it plans the Arrow look in
//! preview and export alike.
//!
//! **Threads:** takes load, smooth and build their activity facts and follow paths on their own utility-QoS thread
//! (`queue`), never the render thread or the actor. The session asks for a project's takes when it is published, and
//! when a screen file's activity grows (`wanted`), and plans again when one arrives (`CursorTracks::wake`). Export loads
//! and builds them before its first frame (`load_all`), so preview, scrubbing and export draw, zoom and crop the same.

use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;

use klipolo_cursor::{CursorTrack, Limits};

mod follow;
mod queue;
mod refuse;
mod shapes;
mod state;
mod take;
mod tracks;
mod variants;
mod want;

pub use follow::{ActivityOf, FollowKey, Screens};
pub use take::TakeCursor;
pub use tracks::{BUDGET_BYTES, CursorTracks};
pub use want::{MAX_FOLLOWS, Want, draws_any, take_paths, wanted};

/// `path`'s take: its track (`load_track`), its facts and its shape images' checks; the take's folder is the file's.
/// On the loader thread (or export's blocking load): O(samples + shapes).
pub fn load_take(path: &Path) -> io::Result<TakeCursor> {
    let track = load_track(path)?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let dir = dir.unwrap_or(Path::new(".")).to_path_buf();
    Ok(TakeCursor::new(track, dir))
}

/// `path`'s track: refused before opening unless it is a regular file (opening a pipe waits for a writer,
/// and a device is never opened), opened with `open_regular`, then read with `Limits::for_file_len`.
pub fn load_track(path: &Path) -> io::Result<CursorTrack> {
    let not_regular = || io::Error::new(io::ErrorKind::InvalidInput, "not a regular file");
    if !std::fs::metadata(path)?.is_file() {
        return Err(not_regular());
    }
    let (file, meta) = open_regular(path)?.ok_or_else(not_regular)?;
    let track = CursorTrack::read(
        BufReader::with_capacity(64 * 1024, file),
        Limits::for_file_len(meta.len()),
    );
    let notes = track.notes();
    if notes.lost_any() {
        tracing::warn!(
            file = %crate::log_name(path),
            skipped = notes.skipped,
            other_headers = notes.other_headers,
            refused = notes.refused,
            limited = notes.limited,
            read_failed = notes.read_failed,
            "cursor file: some lines weren't kept"
        );
    }
    Ok(track)
}

/// `path` opened for reading, with its metadata, if it is a regular file (`Ok(None)` if it isn't). On unix the open
/// never waits for a writer (`O_NONBLOCK`): a path swapped for a pipe after the caller's check is
/// refused at once instead of holding the loader thread forever, and reads of a regular file ignore the flag. The
/// check is on the open file, so what is read is what was checked.
pub(crate) fn open_regular(path: &Path) -> io::Result<Option<(File, std::fs::Metadata)>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    Ok(meta.is_file().then_some((file, meta)))
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_pipe_swapped_in_after_the_check_is_refused_without_waiting_for_a_writer() {
        // A cursor file or shape swapped for a pipe between its check and its open used to block the
        // loader thread in the open, until a writer came (never). The open itself must refuse it at once.
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("cursor.jsonl");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(made.is_ok_and(|s| s.success()), "mkfifo");
        let (tx, rx) = mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(open_regular(&path).map(|f| f.is_some()));
        });
        let opened = rx.recv_timeout(Duration::from_secs(5));
        if opened.is_err() {
            // A writer lets the waiting open go, so the test fails instead of hanging.
            let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
        }
        assert!(matches!(opened, Ok(Ok(false))), "{opened:?}");
        // A regular file opens as one.
        let file = dir.path().join("take.jsonl");
        std::fs::write(&file, b"{}").unwrap();
        assert_eq!(open_regular(&file).unwrap().map(|(_, m)| m.len()), Some(2));
    }
}
