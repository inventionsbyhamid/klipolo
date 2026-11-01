//! A take's cursor as format 4's `RecordingCursor`: the manifest's own values (how the frames show it, from when, and
//! how its clicks were recorded), with the file's path in the take's folder.
//!
//! A game take has one only when its profile asked for the cursor, and then always burned in, so nothing is
//! drawn over it, and `framing::default_framing` never follows a game take. Other games have none.

use klipolo_model::cursor_style::RecordingCursor;

use crate::manifest::Manifest;
use crate::session::SessionDir;

/// The take's cursor, if its manifest has one and its file's key stays inside the take.
pub(super) fn recording_cursor(m: &Manifest, dir: &SessionDir) -> Option<RecordingCursor> {
    let cursor = m.cursor.as_ref()?;
    let path = dir.key_path(&cursor.file).ok()?;
    Some(RecordingCursor {
        path: path.to_string_lossy().into_owned(),
        capture: cursor.capture,
        clicks: cursor.clicks,
    })
}
