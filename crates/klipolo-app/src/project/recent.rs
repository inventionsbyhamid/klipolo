//! The start screen's recent projects: the library's project files, the ones opened lately first.

use klipolo_model::time::Us;
use serde::Serialize;
use ts_rs::TS;

/// A project file in the library, as the start screen shows it.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectRow {
    /// The project file, for `project_open`.
    pub path: String,
    pub name: String,
    /// When it was last opened (ms since the Unix epoch); its last save for a file never opened here.
    pub opened_ms: u64,
    /// The main track's length.
    pub duration_us: Us,
    /// Clips on the main track.
    pub clips: u32,
    /// Its first clip's first cached thumbnail; None until the clip has been scanned.
    pub thumb: Option<ThumbTile>,
}

/// The first tile of a sprite sheet of thumbnails: the top left `tile_w` by `tile_h` of the JPEG at `sprite` (read
/// through the asset protocol).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThumbTile {
    pub sprite: String,
    pub tile_w: u32,
    pub tile_h: u32,
    /// Tiles per sprite row.
    pub cols: u32,
}
