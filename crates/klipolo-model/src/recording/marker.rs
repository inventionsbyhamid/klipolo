//! A recording's markers (moments on its clock) and the game a gameplay take captured. Re-exported from
//! `recording`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::Id;
use crate::time::Us;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum MarkerKind {
    /// Placed with the marker key while recording.
    Hotkey,
    /// Where a replay was saved.
    ReplaySave,
    /// Found by Klipolo.
    Highlight,
    /// Placed in the editor.
    Manual,
}

/// A moment of the recording, on the recording clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Marker {
    pub id: Id,
    pub at_us: Us,
    pub kind: MarkerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
}

/// The game a gameplay recording captured. `app_id` is a bundle id (macOS), an executable path (Windows) or a
/// desktop-file id or executable (Linux).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GameInfo {
    pub app_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub profile_id: Option<Id>,
}
