//! A screen file's activity files in the cache folder (`<root>/<stem>.activity`, finished; its part file while it is
//! made; and the mark of a file that can't be analysed) and their reads, shared by the pass (`run`) and the store.

use std::path::{Path, PathBuf};

use klipolo_cursor::activity::{
    ActivityTrack, CHUNK_TICKS, TICK_US, decode, missing, stored_duration,
};
use klipolo_model::time::Us;

/// A chunk's length: a minute.
pub(crate) const CHUNK_US: Us = CHUNK_TICKS as Us * TICK_US;

/// `<root>/<stem>.activity`, and its part file.
pub(crate) fn files(root: &Path, stem: &str) -> (PathBuf, PathBuf) {
    let done = root.join(format!("{stem}.activity"));
    (done.clone(), done.with_extension("activity.part"))
}

/// `<root>/<stem>.activity.failed`: the screen file can't be analysed, so every store sharing the folder
/// plans without its activity (`ActivityStore::fail`). Empty.
pub(crate) fn failed_mark(root: &Path, stem: &str) -> PathBuf {
    root.join(format!("{stem}.activity.failed"))
}

/// How many chunks a file of `duration_us` has.
pub(crate) fn chunks_of(duration_us: Us) -> usize {
    usize::try_from((duration_us + CHUNK_US - 1) / CHUNK_US).unwrap_or(0)
}

/// Some chunk of `track` is made (a part file from an earlier opening has something to show).
pub(crate) fn any_made(track: &ActivityTrack) -> bool {
    missing(track).len() < chunks_of(track.duration_us())
}

/// An activity file's track (the finished one, or a part file read without its lock), read with the duration its header
/// stores (None: there is none, or its header can't be read). O(bytes).
pub(crate) fn finished(done: &Path) -> Option<ActivityTrack> {
    let bytes = std::fs::read(done).ok()?;
    Some(decode(&bytes, stored_duration(&bytes)?))
}
