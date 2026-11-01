//! Making or opening the project to work in.

use std::path::{Path, PathBuf};

use klipolo_model::coded::Said;
use klipolo_model::history::{Document, Snapshot};
use klipolo_model::project::{FormatKind, Project};

use super::library::new_file;
use super::{OpenProject, ProjectSessionState, open_file, save, set_open, with_open};
use klipolo_platform::durable::Durability;

/// The name a project gets when none is given.
pub(crate) const UNTITLED: &str = "Untitled project";

/// A project named `name` (trimmed) in `formats` (the first is its primary; none: the defaults).
pub(crate) fn new_project(name: &str, formats: Vec<FormatKind>) -> Project {
    let name = name.trim();
    let mut project = Project::new(if name.is_empty() { UNTITLED } else { name });
    if let Some(&first) = formats.first() {
        project.settings.primary_format = first;
        project.settings.formats = formats;
    }
    project
}

/// Saves `project` as a new file in `library` (a name whose file is taken gets the file's number too: `new_file`) and
/// opens it (the open one is saved and closed first, as `open_in` does): its snapshot, with the opening it made. When
/// the open one can't be saved, it stays open and the new file goes.
pub(crate) fn create_in(
    state: &ProjectSessionState,
    library: &Path,
    project: Project,
) -> Result<Snapshot, Said> {
    create_file_in(state, library, project).map(|(snapshot, _)| snapshot)
}

/// `create_in`, and the new project's file.
pub(crate) fn create_file_in(
    state: &ProjectSessionState,
    library: &Path,
    project: Project,
) -> Result<(Snapshot, PathBuf), Said> {
    let (file, name) = new_file(library, &project.name);
    let mut project = project;
    project.name = name;
    let document = Document::new(project).map_err(|e| Said::Coded(e.coded()))?;
    save(&file, document.project(), Durability::Ordered).map_err(|error| {
        tracing::warn!(error = error.detail(), "a new project couldn't be saved");
        Said::code("project.createFailed")
    })?;
    let mut snapshot = document.snapshot();
    let opened = set_open(
        state,
        Some(OpenProject {
            document,
            file: file.clone(),
            read_only: false,
        }),
    );
    // The open project couldn't be saved, so it stays open: the new one's file goes again.
    let opening = opened.inspect_err(|_| {
        let _ = std::fs::remove_file(&file);
    })?;
    snapshot.opening = Some(opening);
    listed(state, &file, &mut snapshot);
    Ok((snapshot, file))
}

/// Opens the project in `file` (the open one is saved and closed first): its snapshot, with the opening it made.
pub(crate) fn open_in(state: &ProjectSessionState, file: &Path) -> Result<Snapshot, Said> {
    let open = open_file(file, &state.pack())?;
    let mut snapshot = open.document.snapshot();
    snapshot.opening = Some(set_open(state, Some(open))?);
    listed(state, file, &mut snapshot);
    Ok(snapshot)
}

/// The open project's whole snapshot, as `project_snapshot` gives it.
pub(crate) fn snapshot_now(state: &ProjectSessionState) -> Result<Snapshot, Said> {
    let (mut snapshot, file) = with_open(state, |open| {
        let mut snapshot = open.document.snapshot();
        snapshot.opening = Some(state.opens.load(std::sync::atomic::Ordering::SeqCst));
        Ok((snapshot, open.file.clone()))
    })?;
    listed(state, &file, &mut snapshot);
    Ok(snapshot)
}

/// Sets the name the start screen and Open recent list `snapshot`'s project by, in `file`'s folder, when it isn't the
/// project's own (`recent::listed_as`), so the page names it alike everywhere.
fn listed(state: &ProjectSessionState, file: &Path, snapshot: &mut Snapshot) {
    let stamps = state.stamps().unwrap_or_default();
    snapshot.listed_as = super::recent::listed_as(file, &stamps, &snapshot.project.name);
}
