//! The open project as chat reads and edits it, behind an interface the app's project session implements
//! (`ProjectSessionState` in the binary), so the chat runs and tests without Tauri. Chat reads the project as it is when
//! a message is sent (`Projects::now`) and commits only on that revision: an edit made on a revision the project moved
//! on from (a hand edit while the model answered, an undo, another project) is refused, never rebased.
//!
//! `commit_reviewed` is the one way chat's edits enter the history: additions are applied once, and a proposal the
//! person applies records the project its review already made (`Transaction::review`), applied once too. Either way it
//! is one undo step, made by the person (`Document::commit_tx`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_model::coded::Said;
use klipolo_model::history::{Change, Document, SnapshotPatch};
use klipolo_model::{ModelError, Project, Transaction};

/// Where the open project stands: which project, which opening of it (revisions restart at 0 on each), and its
/// revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct At {
    pub project_id: String,
    pub opening: u64,
    pub revision: u64,
}

impl At {
    /// The same project, opened the same time (whatever its revision).
    pub fn same_project(&self, other: &At) -> bool {
        self.project_id == other.project_id && self.opening == other.opening
    }
}

/// The open project as a message finds it.
#[derive(Clone, Debug)]
pub struct ProjectNow {
    pub at: At,
    /// The revision `at` names, shared (no copy).
    pub project: Arc<Project>,
    /// Where it is saved: pictures made for it go in its media folder.
    pub file: PathBuf,
    /// From a newer Klipolo: nothing edits it.
    pub read_only: bool,
}

/// What a commit made: the change for the page, and where the project stands now.
#[derive(Clone, Debug)]
pub struct Committed {
    pub patch: SnapshotPatch,
    pub at: At,
    pub project: Arc<Project>,
}

/// Why a commit or an undo didn't happen. Nothing changed.
#[derive(Clone, Debug, PartialEq)]
pub enum CommitError {
    NoProject,
    /// The open project isn't where the edit was made any more: another revision (a hand edit, an undo), or another
    /// project or opening.
    Moved,
    /// Refused as it is (a read-only project, ops that don't apply): what the person is told.
    Refused(Said),
}

impl CommitError {
    /// What the page is told: `project.moved` makes it resync and drop what was made on the old revision.
    pub fn said(&self) -> Said {
        match self {
            Self::NoProject => Said::code("project.none"),
            Self::Moved => Said::code("project.moved"),
            Self::Refused(said) => said.clone(),
        }
    }
}

/// The app's open project, as chat uses it.
pub trait Projects: Send + Sync {
    /// The open project now; None when none is open.
    fn now(&self) -> Option<ProjectNow>;

    /// Commits `tx` as one undo step on the open project when it still stands `at`. With `next` (what
    /// `apply_ops(project, &tx.ops)` made, as a review keeps it) that project is recorded as it is; without, the ops
    /// are applied once. The new revision reaches the engine and is saved like every edit.
    fn commit(
        &self,
        at: &At,
        tx: &Transaction,
        next: Option<Project>,
    ) -> Result<Committed, CommitError>;

    /// Takes back the top undo step when the open project still stands `at`.
    fn undo(&self, at: &At) -> Result<Committed, CommitError>;
}

/// Records `tx` on `document` as one undo step (`Document::commit_tx`): `next` when given (a reviewed project, applied
/// once already), else its ops applied once. An empty batch changes nothing and records nothing.
pub fn commit_reviewed(
    document: &mut Document,
    tx: &Transaction,
    next: Option<Project>,
) -> Result<SnapshotPatch, ModelError> {
    let change = match next {
        Some(next) => document.commit_tx(tx, next),
        None if tx.ops.is_empty() => Change::none(document.revision()),
        None => document.apply_change(tx)?,
    };
    Ok(document.snapshot_patch(change))
}

/// A project held in memory, with its history: for tools and tests that run chat without the app.
pub struct MemoryProjects {
    open: Mutex<Option<Held>>,
}

struct Held {
    document: Document,
    file: PathBuf,
    opening: u64,
}

impl MemoryProjects {
    /// `project` open, saved (in name only) at `file`.
    pub fn open(project: Project, file: PathBuf) -> Result<Self, ModelError> {
        Ok(Self {
            open: Mutex::new(Some(Held {
                document: Document::new(project)?,
                file,
                opening: 1,
            })),
        })
    }

    fn held(&self) -> MutexGuard<'_, Option<Held>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The open project (panics when none is).
    pub fn project(&self) -> Arc<Project> {
        self.held()
            .as_ref()
            .expect("a project is open")
            .document
            .shared()
    }

    pub fn revision(&self) -> u64 {
        self.held().as_ref().map_or(0, |h| h.document.revision())
    }

    /// Undo steps there are.
    pub fn undo_steps(&self) -> usize {
        self.held().as_ref().map_or(0, |h| h.document.undo_steps())
    }

    /// A hand edit, as the timeline makes one: one undo step.
    pub fn hand_edit(&self, tx: &Transaction) -> Result<SnapshotPatch, ModelError> {
        let mut held = self.held();
        let held = held.as_mut().expect("a project is open");
        commit_reviewed(&mut held.document, tx, None)
    }

    /// Undo (or redo) as the Edit menu does: None when there is no step to take.
    pub fn step(&self, redo: bool) -> Option<SnapshotPatch> {
        let mut held = self.held();
        let document = &mut held.as_mut()?.document;
        let change = if redo {
            document.redo_change()
        } else {
            document.undo_change()
        }?;
        Some(document.snapshot_patch(change))
    }

    /// Another project opens in its place (`None`: the project closes).
    pub fn switch(&self, project: Option<Project>) -> Result<(), ModelError> {
        let mut held = self.held();
        let opening = held.as_ref().map_or(0, |h| h.opening) + 1;
        let file = held.as_ref().map(|h| h.file.clone()).unwrap_or_default();
        *held = match project {
            Some(project) => Some(Held {
                document: Document::new(project)?,
                file,
                opening,
            }),
            None => None,
        };
        Ok(())
    }

    /// `f` on the open project when it still stands `at`.
    fn at<T>(
        &self,
        at: &At,
        f: impl FnOnce(&mut Document) -> Result<T, CommitError>,
    ) -> Result<(T, Committed), CommitError> {
        let mut held = self.held();
        let held = held.as_mut().ok_or(CommitError::NoProject)?;
        if at_of(held) != *at {
            return Err(CommitError::Moved);
        }
        let out = f(&mut held.document)?;
        let committed = Committed {
            patch: held
                .document
                .snapshot_patch(Change::none(held.document.revision())),
            at: at_of(held),
            project: held.document.shared(),
        };
        Ok((out, committed))
    }
}

fn at_of(held: &Held) -> At {
    At {
        project_id: held.document.project().id.clone(),
        opening: held.opening,
        revision: held.document.revision(),
    }
}

impl Projects for MemoryProjects {
    fn now(&self) -> Option<ProjectNow> {
        let held = self.held();
        let held = held.as_ref()?;
        Some(ProjectNow {
            at: at_of(held),
            project: held.document.shared(),
            file: held.file.clone(),
            read_only: false,
        })
    }

    fn commit(
        &self,
        at: &At,
        tx: &Transaction,
        next: Option<Project>,
    ) -> Result<Committed, CommitError> {
        let (patch, committed) = self.at(at, |document| {
            commit_reviewed(document, tx, next)
                .map_err(|e| CommitError::Refused(Said::Coded(e.coded())))
        })?;
        Ok(Committed { patch, ..committed })
    }

    fn undo(&self, at: &At) -> Result<Committed, CommitError> {
        let (patch, committed) = self.at(at, |document| {
            let revision = document.revision();
            let change = document
                .undo_change()
                .unwrap_or_else(|| Change::none(revision));
            Ok(document.snapshot_patch(change))
        })?;
        Ok(Committed { patch, ..committed })
    }
}
