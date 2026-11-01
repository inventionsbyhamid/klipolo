//! What the UI receives: a whole project on open, create and resync, and a revision-chained patch after every edit,
//! undo and redo.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{Change, Document, HistoryState};
use crate::patch::ProjectPatch;
use crate::project::Project;

/// Everything the UI needs after a change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Snapshot {
    pub project: Project,
    pub history: HistoryState,
    /// Set for a project from a newer Klipolo: the code `model.readOnly` (`contrib::read_only_notice`), which the UI
    /// words and says once, and then it doesn't edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub read_only: Option<String>,
    /// Which opening of the project this is, when the host says (revisions restart at 0 on each): a change the host
    /// tells of later, with no reply of its own, is matched to it by project, opening and revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub opening: Option<u64>,
    /// The name the host's project list shows for this project when it isn't the project's own (`ListedAs`), set by the
    /// host as it sends the whole project; left out of the JSON while None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub listed_as: Option<ListedAs>,
}

/// The name a project is listed by in place of its own `name`, while it keeps that name: its file's name, when another
/// project in the library has the same name (a project made before new projects took their file's number). The UI shows
/// it wherever it names the project, so every place names it alike; the project's file is never rewritten for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListedAs {
    /// The project's own name this stands in for: once it is renamed, its own name shows.
    pub name: String,
    /// The name it is listed by.
    pub file_name: String,
}

/// What the UI needs after an edit, undo or redo: the change (to apply to the revision it was made on) and the new
/// history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotPatch {
    pub base_revision: u64,
    pub history: HistoryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub read_only: Option<String>,
    pub patch: ProjectPatch,
}

impl Document {
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            project: Project::clone(&self.project),
            history: self.history(),
            read_only: crate::contrib::read_only_notice(&self.project).map(str::to_owned),
            opening: None,
            listed_as: None,
        }
    }

    pub fn snapshot_patch(&self, change: Change) -> SnapshotPatch {
        SnapshotPatch {
            base_revision: change.base_revision,
            history: self.history(),
            read_only: crate::contrib::read_only_notice(&self.project).map(str::to_owned),
            patch: change.patch,
        }
    }
}
