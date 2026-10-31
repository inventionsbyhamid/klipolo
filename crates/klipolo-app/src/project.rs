//! The open project. An edit, undo or redo returns at once with what it changed, as a patch on the revision it was
//! made on. The new revision is published once, as an `Arc` the engine and the saver share (no copy per edit); the
//! saver writes it 2 s after the last change (`saver`), and the project is flushed to the disk before it closes or
//! switches and on quit.
//!
//! **One apply path.** Hand edits (and later chat) commit through `with_open`, so every edit is one undo step, reaches
//! the engine and is saved. `apply_transaction` applies a batch, refusing it when the project moved on from the base
//! the caller built it on (never rebased). Every command fails with a code the UI words, or a plain sentence (`Said`).

mod apply;
mod chat;
mod create;
mod disk;
mod library;
mod recent;
mod saver;
mod session;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

#[cfg(test)]
use apply::apply_and_commit;
pub(crate) use apply::{
    ApplyError, apply_transaction, build_and_apply, build_named_and_apply, step, with_open,
};
pub(crate) use create::{create_in, new_project, open_in, snapshot_now};
pub(crate) use disk::{SaveError, save};
use klipolo_engine::session::{FeedRx, ProjectFeed};
use klipolo_model::Transaction;
use klipolo_model::coded::Said;
use klipolo_model::contrib::AssetPath;
use klipolo_model::history::{Change, Document, HistoryLimits, Snapshot, SnapshotPatch};
use klipolo_model::project::{AssetKind, FormatKind, Project};
use klipolo_platform::pressure::{self, Pressure};
pub(crate) use library::{library_dir, read};
use recent::{ProjectRow, RECENT_FILE, RECENT_LIMIT, recent_in};
pub(crate) use saver::{SaveStatus, Saver, Version};
pub(crate) use session::ProjectSessionState;
#[cfg(test)]
pub(crate) use session::opened;
use tauri::{AppHandle, Manager, State};

use crate::pack::Pack;
use crate::state::EngineState;
use tokio::sync::watch;

/// A project file's extension: `~/Movies/Klipolo/<name>.klipolo`.
pub(crate) const PROJECT_EXTENSION: &str = "klipolo";
/// How long closing, switching or quitting waits for the last save.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct OpenProject {
    pub(crate) document: Document,
    /// Where it is saved.
    pub(crate) file: PathBuf,
    /// Set when the file came from a newer Klipolo (`Project.requires`): edits are refused (`model.readOnly`), so
    /// saving can't drop data this build doesn't understand.
    pub(crate) read_only: bool,
}

/// A project from a newer Klipolo: the UI says so once on open (`Snapshot.readOnly`) and doesn't edit; edits that still
/// arrive are refused.
fn is_read_only(project: &Project) -> bool {
    klipolo_model::contrib::read_only_notice(project).is_some()
}

/// What an edit of a read-only project is refused with.
fn read_only_refusal() -> Said {
    Said::code(klipolo_model::contrib::READ_ONLY_NOTICE)
}

/// Reads `file` into an open project; one from a newer Klipolo opens read-only.
pub(crate) fn open_file(file: &Path, pack: &Pack) -> Result<OpenProject, Said> {
    let mut project = read(file)?;
    // Saved where another install (or a moved app) had the pack: its files are this install's.
    pack.repoint(&mut project);
    let read_only = is_read_only(&project);
    let document = Document::new(project).map_err(|e| Said::Coded(e.coded()))?;
    Ok(OpenProject {
        document,
        file: file.to_path_buf(),
        read_only,
    })
}

/// Whether `path` is a file of the open project (one of its assets): the only files the timeline's media reads.
pub(crate) fn uses_path(state: &ProjectSessionState, path: &str) -> bool {
    lock(state).as_ref().is_some_and(|open| {
        open.document
            .project()
            .assets
            .iter()
            .any(|asset| asset.path == path)
    })
}

/// The kind of the open project's file at `path`; None when it uses no such file.
pub(crate) fn asset_kind(state: &ProjectSessionState, path: &str) -> Option<AssetKind> {
    lock(state).as_ref().and_then(|open| {
        let assets = &open.document.project().assets;
        assets
            .iter()
            .find(|asset| asset.path == path)
            .map(|asset| asset.kind)
    })
}

/// A command that panicked must not leave the project locked for the rest of the session.
fn lock(state: &ProjectSessionState) -> MutexGuard<'_, Option<OpenProject>> {
    state.open.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Replaces the open project (create, open, close) and publishes it to the engine under the same lock; returns the
/// opening it made. The project being replaced is saved first: a save that fails keeps it open, with what went wrong.
fn set_open(state: &ProjectSessionState, open: Option<OpenProject>) -> Result<u64, Said> {
    let mut guard = lock(state);
    if guard.is_some() {
        state.flush().map_err(|e| Said::Text(e.to_string()))?;
    }
    // The switch is sure now (the old project saved, the new one read): analysis lets the old project go.
    state.switching();
    if let Some(open) = &open {
        state.remember_last(&open.file);
    }
    *guard = open;
    state.publish(guard.as_ref(), true);
    Ok(state.opens.load(Ordering::SeqCst))
}

/// Whether the page may be sent the bytes of `path`, a file the open project uses (`uses_path`): a local file of a media
/// type Klipolo imports, never a pack's file or anything else a project file names (a project edited by hand to point at
/// `~/.ssh/id_rsa`).
pub(crate) fn servable(path: &str) -> bool {
    matches!(
        AssetPath::parse(path),
        Some(AssetPath::File(file)) if crate::media::supported(Path::new(file))
    )
}

/// A new project named `name` (the first format is its primary), saved in the library and opened. The open project's
/// analysis stops once the switch is sure (`ProjectSessionState::on_switch`).
#[tauri::command(async)]
pub fn project_create(
    state: State<'_, ProjectSessionState>,
    name: String,
    formats: Vec<FormatKind>,
) -> Result<Snapshot, Said> {
    create_in(&state, &library_dir()?, new_project(&name, formats))
}

/// Opens the project file at `path` (the open one is saved and closed first, and its analysis stops).
#[tauri::command(async)]
pub fn project_open(state: State<'_, ProjectSessionState>, path: String) -> Result<Snapshot, Said> {
    open_in(&state, Path::new(&path))
}

/// The start screen's recent projects (`recent::recent_in`): the library's project files, last opened first. Nothing is
/// opened. Each card's thumbnail sprite is let through the asset protocol (the cache folder's scope holds it already).
/// A projects folder that can't be listed fails as `project.libraryFailed`, never as an empty library.
#[tauri::command(async)]
#[expect(dead_code, reason = "registered with the start screen")]
pub fn projects_recent(
    app: AppHandle,
    engine: State<'_, EngineState>,
) -> Result<Vec<ProjectRow>, Said> {
    // Without an app data folder nothing was stamped: the list goes by last save.
    let stamps = app
        .path()
        .app_data_dir()
        .map(|dir| dir.join(RECENT_FILE))
        .unwrap_or_default();
    let thumbs = engine.caches().map(|caches| caches.thumbs());
    let mut rows =
        recent_in(&library_dir()?, &stamps, thumbs.as_deref(), RECENT_LIMIT).map_err(|error| {
            tracing::warn!(%error, "the projects folder couldn't be listed");
            Said::code("project.libraryFailed")
        })?;
    let scope = app.asset_protocol_scope();
    for row in &mut rows {
        // A sprite that can't be let through goes: the card shows the clip art instead.
        if row
            .thumb
            .as_ref()
            .is_some_and(|tile| scope.allow_file(&tile.sprite).is_err())
        {
            row.thumb = None;
        }
    }
    Ok(rows)
}

/// Closes the open project once its edits are saved; its analysis stops.
#[tauri::command(async)]
pub fn project_close(state: State<'_, ProjectSessionState>) -> Result<(), Said> {
    set_open(&state, None).map(|_| ())
}

/// A hand edit. `project_id` and `base_revision` are the UI copy's: a batch for another project, or for a revision the
/// project moved on from, is refused with `project.moved` (the UI resyncs), never rebased. None for a caller with no
/// copy (then the open project, as it is).
#[tauri::command(async)]
pub fn project_apply(
    state: State<'_, ProjectSessionState>,
    tx: Transaction,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    let id = match project_id {
        Some(id) => id,
        None => lock(&state)
            .as_ref()
            .map(|open| open.document.project().id.clone())
            .ok_or_else(|| ApplyError::NoProject.said())?,
    };
    apply_transaction(&state, &id, &tx, base_revision).map_err(|e| e.said())
}

/// Undo, as the UI's copy saw the history: when the open project moved on from the copy's revision (a change the UI
/// hasn't merged yet is on top), it is refused with `project.moved`, so Undo never takes back a change the person
/// hasn't seen. None: the top step as it is.
#[tauri::command(async)]
pub fn project_undo(
    state: State<'_, ProjectSessionState>,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    step_at(&state, false, base_revision, project_id.as_deref())
}

/// Redo, refused like [`project_undo`] when the open project moved on from the copy's revision.
#[tauri::command(async)]
pub fn project_redo(
    state: State<'_, ProjectSessionState>,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    step_at(&state, true, base_revision, project_id.as_deref())
}

/// [`step`] on the open project when it is still `project_id` at `base` (each when given), checked under the lock it
/// steps in; else `project.moved`.
fn step_at(
    state: &ProjectSessionState,
    redo: bool,
    base: Option<u64>,
    project_id: Option<&str>,
) -> Result<SnapshotPatch, Said> {
    with_open(state, |open| {
        let other = project_id.is_some_and(|id| id != open.document.project().id);
        if other || base.is_some_and(|r| r != open.document.revision()) {
            return Err(ApplyError::Moved.said());
        }
        step(open, redo)
    })
}

/// The whole open project (the UI resynchronises with it when a patch doesn't follow its revision, and reads it when
/// the page loads).
#[tauri::command(async)]
pub fn project_snapshot(state: State<'_, ProjectSessionState>) -> Result<Snapshot, Said> {
    snapshot_now(&state)
}
