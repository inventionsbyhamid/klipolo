//! How Sync lined a recording source up: by listening, or by the person's marks, and how sure it was. It lives on the
//! source, so a sync again and a hand line-up update it in the same `updateRecordingSource`. A take Klipolo recorded
//! has none. Re-exported from `project`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What placed the source: Sync listening to both files, or the person's marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SyncedBy {
    Sound,
    Hand,
}

/// How sure the line-up is: Sure applies quietly; Check applies with "Play the check".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SyncLevel {
    Sure,
    Check,
}

/// How Sync lined a source up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Synced {
    pub by: SyncedBy,
    pub level: SyncLevel,
}
