//! The takes' cursor facts chat's `autoZoom` plans zooms from: a recording's cursor file read as the engine and a new
//! take's auto zoom read it (`load_track`, `TakeFacts::of_cursor`), only when the tool asks for it. A file that is
//! missing or can't be read gives none, with a warning in the log: the recording has no clicks to zoom on.

use std::path::Path;
use std::sync::Arc;

use klipolo_chat::CursorTakes;
use klipolo_cursor::facts::StillRuns;
use klipolo_cursor::timeline::TakeFacts;
use klipolo_engine::media::cursor::load_track;
use klipolo_model::project::Recording;

/// Each take's cursor file, read when asked.
#[derive(Debug, Default)]
pub(crate) struct CursorFiles;

impl CursorTakes for CursorFiles {
    fn facts(&self, recording: &Recording) -> Option<Arc<TakeFacts>> {
        let path = recording.cursor_path()?;
        match load_track(Path::new(path)) {
            Ok(track) => Some(Arc::new(TakeFacts::of_cursor(
                &track,
                &StillRuns::of(&track),
            ))),
            Err(error) => {
                tracing::warn!(
                    %error,
                    take = recording.id,
                    "chat: the take's cursor file couldn't be read, so it has no clicks to zoom on"
                );
                None
            }
        }
    }
}
