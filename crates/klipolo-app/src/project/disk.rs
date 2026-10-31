//! The project file on disk: an atomic save (`klipolo_platform::durable`: tmp + sync + rename) and why a save failed,
//! in words the person can act on.

use std::fs;
use std::io;
use std::path::Path;

use klipolo_model::project::Project;
use klipolo_platform::durable::{self, DiskReason, Durability, Steps};

/// Why a save failed: what the person reads (`Display`) and the details for the log (`detail`).
#[derive(Clone, Debug)]
pub(crate) struct SaveError {
    reason: DiskReason,
    detail: String,
}

impl SaveError {
    pub(crate) fn io(e: &io::Error) -> Self {
        Self {
            reason: durable::classify(e),
            detail: e.to_string(),
        }
    }

    fn other(detail: impl Into<String>) -> Self {
        Self {
            reason: DiskReason::Other,
            detail: detail.into(),
        }
    }

    /// A save that didn't finish in time: the disk doesn't answer.
    pub(crate) fn timed_out() -> Self {
        Self::other("the save didn't finish in time")
    }

    /// The details, for the log.
    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

/// What the person reads: the cause, when it is one they can fix, and what to do.
impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.reason {
            DiskReason::Full => "Your disk is full, so changes aren't being saved. Free up some space.",
            DiskReason::ReadOnly | DiskReason::Denied | DiskReason::Locked => {
                "Klipolo can't write to that folder, so changes aren't being saved. Check its permissions."
            }
            DiskReason::Other => "Klipolo couldn't save your changes. The disk reported an error.",
        })
    }
}

/// Makes the project's folder, then saves into it (`save_with`).
pub(crate) fn save(
    file: &Path,
    project: &Project,
    durability: Durability,
) -> Result<(), SaveError> {
    let dir = file
        .parent()
        .ok_or_else(|| SaveError::other("the project file has no folder"))?;
    fs::create_dir_all(dir).map_err(|e| SaveError::io(&e))?;
    save_with(&durable::Os, file, project, durability)
}

/// Writes `<file>.<pid>.tmp`, pushes it to the disk, then renames it over `file`, so a crash or power cut never leaves
/// half a file. A failed write removes the temporary file. Through `steps` (the saver's disk: the real one, or a
/// test's).
pub(crate) fn save_with(
    steps: &dyn Steps,
    file: &Path,
    project: &Project,
    durability: Durability,
) -> Result<(), SaveError> {
    let bytes = serde_json::to_vec(project).map_err(|e| SaveError::other(e.to_string()))?;
    durable::write_atomic_with(steps, file, &bytes, durability).map_err(|e| SaveError::io(&e))
}
