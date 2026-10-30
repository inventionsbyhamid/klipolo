//! What the editing-copy worker tells the app: one JSON object per stdout line, as export's worker does. Logs go
//! to stderr. The app's one command, `pause`, arrives as a line on the worker's stdin; closing stdin (the app quit)
//! stops it too.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::run::{FailCode, PauseReason};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProxyLine {
    /// `done` of `total` frames are in verified segments (earlier runs' included).
    Progress { done: u64, total: u64 },
    /// Stopped between segments or mid-segment: every finished segment is kept.
    Paused { reason: PauseReason },
    /// The copy is in place at `path`.
    Done { path: PathBuf },
    /// The copy can't be made: why (`code`; `missing` is `code == Missing`), in words. Not retried by itself.
    Failed {
        message: String,
        missing: bool,
        code: FailCode,
    },
    /// Thumbnail chunk `chunk` was refilled from the copy's full decode (`Fill::Full`): the filmstrip can refresh.
    Thumbs { chunk: u32 },
}

/// One worker line, or None for anything else on stdout (a library printing there is skipped).
pub fn parse_line(line: &str) -> Option<ProxyLine> {
    serde_json::from_str(line.trim()).ok()
}
