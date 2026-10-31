//! Screen Recording that macOS still refuses after Reopen Klipolo.
//!
//! macOS applies a Screen Recording grant only to a relaunched app, so the recorder's step ends with Reopen Klipolo.
//! When the switch shows on but its entry belongs to another copy of Klipolo (one signed differently, like an earlier
//! ad hoc build), macOS still says no after the relaunch, and the step would ask again in a loop. So Reopen Klipolo is
//! remembered as it goes ahead (`remember`), in the recorder's own files (`<app data>/recorder`, never a project). A
//! later setup that still reads Screen Recording as not allowed says so (`RecorderSetup::screen_refused_after_reopen`),
//! and the step explains how to remove the old entry and allow Klipolo again. Once it is allowed, the memory goes.
//! Klipolo never resets or changes the setting itself.
//!
//! The memory also names the project that was open. A launch soon after (`back_from`) is that reopen's, and the page
//! opens the project again with the Record dialog, where the person was.
//!
//! macOS's own alert offers the same relaunch: once Klipolo is turned on in the list, "Quit & Reopen" quits it with no
//! word beforehand and opens it again. So a run that found Screen Recording refused while the person chose what to
//! record (the Record dialog's step showed, or they pressed Allow) remembers that as it exits, with the project open
//! then (`remember_asked`, a file of its own). The next launch takes that memory, once: when it comes soon after the
//! exit and Screen Recording is allowed now (`Back::once_allowed`), it is back too, whichever button reopened Klipolo.
//! Any other launch is an ordinary one.

use std::path::{Path, PathBuf};

use klipolo_capture::backend::Permission;
use klipolo_platform::durable::{Durability, write_atomic};
use serde::{Deserialize, Serialize};

use super::setup::{Files, read_json};
use super::types::BackFromReopen;

/// A launch this soon after Reopen Klipolo went ahead is its reopen: the waiter opens Klipolo within about 10 s of the
/// press, and a slow Mac may take a while more to start it.
const BACK_WITHIN_MS: u64 = 120_000;

/// When Reopen Klipolo last went ahead, and the project open then.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reopened {
    /// ms since 1970.
    at_ms: u64,
    /// The open project's file; None on the start screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<PathBuf>,
}

/// `<app data>/recorder/screen-reopen.json`.
fn path(files: &Files) -> PathBuf {
    files.dir.join("screen-reopen.json")
}

/// `<app data>/recorder/screen-asked.json`: Klipolo quit with Screen Recording refused while the person chose what to
/// record.
fn asked_path(files: &Files) -> PathBuf {
    files.dir.join("screen-asked.json")
}

/// Reopen Klipolo goes ahead, at `at_ms` (ms since 1970), with `project` open: remembered for the next opening.
pub(super) fn remember(files: &Files, at_ms: u64, project: Option<&Path>) -> std::io::Result<()> {
    write(&path(files), files, at_ms, project)
}

/// Klipolo exits at `at_ms` (ms since 1970), and in this run Screen Recording was refused while the person chose what
/// to record, with `project` open: remembered for the next opening, should macOS's own "Quit & Reopen" be what quit it.
pub(super) fn remember_asked(
    files: &Files,
    at_ms: u64,
    project: Option<&Path>,
) -> std::io::Result<()> {
    write(&asked_path(files), files, at_ms, project)
}

fn write(path: &Path, files: &Files, at_ms: u64, project: Option<&Path>) -> std::io::Result<()> {
    std::fs::create_dir_all(&files.dir)?;
    let reopened = Reopened {
        at_ms,
        project: project.map(Path::to_path_buf),
    };
    let bytes = serde_json::to_vec(&reopened)?;
    write_atomic(path, &bytes, Durability::Ordered)
}

/// Klipolo opened soon after it quit to apply Screen Recording: where the person was.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Back {
    pub(super) to: BackFromReopen,
    /// Klipolo quit with Screen Recording refused (perhaps by macOS's "Quit & Reopen"): this launch is back only if
    /// Screen Recording is allowed now. False after Reopen Klipolo, which is back either way.
    pub(super) once_allowed: bool,
}

/// Klipolo opens at `now_ms` (ms since 1970): when Reopen Klipolo went ahead just before, the project open then; else,
/// when Klipolo quit just before with Screen Recording refused, the project open then, once Screen Recording is
/// allowed. The quit's memory is taken now, so it counts for this launch only; Reopen Klipolo's stays, for the setup's
/// `refused_after_reopen`.
pub(super) fn back_from(files: &Files, now_ms: u64) -> Option<Back> {
    let asked = read_once(&asked_path(files));
    let soon = |memory: &Reopened| now_ms.saturating_sub(memory.at_ms) <= BACK_WITHIN_MS;
    if let Some(reopened) = read_json::<Reopened>(&path(files)).filter(soon) {
        tracing::info!("recorder: Klipolo is back from Reopen Klipolo");
        return Some(Back {
            to: reopened.into(),
            once_allowed: false,
        });
    }
    let asked = asked.filter(soon)?;
    Some(Back {
        to: asked.into(),
        once_allowed: true,
    })
}

/// The memory at `path`, which goes as it is read.
fn read_once(path: &Path) -> Option<Reopened> {
    let memory = read_json::<Reopened>(path);
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, "recorder: the Screen Recording memory wasn't removed")
        }
    }
    memory
}

impl From<Reopened> for BackFromReopen {
    fn from(memory: Reopened) -> Self {
        Self {
            project: (memory.project).map(|file| file.to_string_lossy().into_owned()),
        }
    }
}

/// Whether macOS still refuses Screen Recording (`screen`, as the setup reads it) after Reopen Klipolo. Once it is
/// allowed, the memory goes, so a later refusal starts again from the first ask.
pub(super) fn refused_after_reopen(files: &Files, screen: Permission) -> bool {
    let path = path(files);
    if screen == Permission::Granted {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!("recorder: Screen Recording is allowed after Reopen Klipolo"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%error, "recorder: the Reopen Klipolo memory wasn't removed")
            }
        }
        return false;
    }
    let Some(reopened) = read_json::<Reopened>(&path) else {
        return false;
    };
    tracing::warn!(
        reopened_ms = reopened.at_ms,
        ?screen,
        "recorder: macOS still refuses Screen Recording after Reopen Klipolo (another copy's entry?)"
    );
    true
}
