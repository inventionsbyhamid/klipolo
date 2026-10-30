//! Why a source shows nothing, and the words export and the preview use for it.

use std::path::Path;
use std::sync::Arc;

use super::DecodedSet;

/// Why a file didn't open, or a frame of it couldn't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    /// It isn't there: the preview says so with `api::MEDIA_MISSING`.
    Missing,
    Unreadable,
    /// Its disk hasn't answered yet (spinning up): the preview says nothing and is redrawn when it answers (the slot
    /// is opened again at once); export stops, naming it.
    NotAnswering,
}

impl DecodedSet {
    /// Notes why `original` (a source's file) shows nothing this frame.
    pub(super) fn report(&mut self, failure: Failure, original: &Arc<str>) {
        match failure {
            Failure::Missing => self.missing.push(original.clone()),
            Failure::Unreadable => self.warnings.push(unreadable(original)),
            Failure::NotAnswering => self.not_answering.push(original.clone()),
        }
    }
}

fn file_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

/// Export's words for a source that isn't there (the preview says it with `api::MEDIA_MISSING`).
pub fn missing(path: &str) -> String {
    format!("Some media is missing: {}", file_name(path))
}

/// A file that is there but couldn't be read (the preview's warning, and export's failure).
pub fn unreadable(path: &str) -> String {
    format!("This file couldn't be read: {}", file_name(path))
}

/// Export's words for a file whose disk doesn't answer (asleep for good, a network volume gone): the job pauses.
pub fn not_answering(path: &str) -> String {
    format!(
        "The disk with {} isn't responding. Check that it's connected and awake, then resume.",
        file_name(path)
    )
}
