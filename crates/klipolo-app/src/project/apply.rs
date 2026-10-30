//! The one apply path every edit takes: applying a batch as one undo step, the publish-and-save tail, and refusing a
//! batch built on a base the project moved on from.

use super::*;

/// Applies the ops once and records the result as one undo step (`Document::commit_tx`: its label keeps its params, so
/// "Undo Rename to “Intro”" can be worded), returning what changed. Saving happens in the background once `with_open`
/// publishes the new revision.
pub(crate) fn apply_and_commit(
    open: &mut OpenProject,
    tx: &Transaction,
) -> Result<SnapshotPatch, Said> {
    if open.read_only {
        return Err(read_only_refusal());
    }
    let change = if tx.ops.is_empty() {
        Change::none(open.document.revision())
    } else {
        let next =
            apply_ops(open.document.project(), &tx.ops).map_err(|e| Said::Coded(e.coded()))?;
        open.document.set_limits(history_limits());
        open.document.commit_tx(tx, next)
    };
    Ok(open.document.snapshot_patch(change))
}

/// Undo and redo together keep 50 steps while the system is short of memory.
fn history_limits() -> HistoryLimits {
    limits_at(pressure::level())
}

fn limits_at(level: Pressure) -> HistoryLimits {
    match level {
        Pressure::Normal => HistoryLimits::default(),
        Pressure::Warning | Pressure::Critical => HistoryLimits::under_pressure(),
    }
}

/// Undo (or redo): what changed, or a patch that changes nothing when there is no step to take.
pub(crate) fn step(open: &mut OpenProject, redo: bool) -> Result<SnapshotPatch, Said> {
    if open.read_only {
        return Err(read_only_refusal());
    }
    open.document.set_limits(history_limits());
    let revision = open.document.revision();
    let change = if redo {
        open.document.redo_change()
    } else {
        open.document.undo_change()
    };
    Ok(open
        .document
        .snapshot_patch(change.unwrap_or_else(|| Change::none(revision))))
}

/// Runs `f` on the open project (None when none is open); a new revision goes to the engine and to the saver.
fn on_open<T>(state: &ProjectSessionState, f: impl FnOnce(&mut OpenProject) -> T) -> Option<T> {
    let mut guard = lock(state);
    let open = guard.as_mut()?;
    let result = f(open);
    if let Some(project) = state.publish(Some(&*open), false) {
        state.saver.submit(open.file.clone(), project);
    }
    Some(result)
}

/// [`on_open`], failing with `project.none` when no project is open.
pub(crate) fn with_open<T>(
    state: &ProjectSessionState,
    f: impl FnOnce(&mut OpenProject) -> Result<T, Said>,
) -> Result<T, Said> {
    on_open(state, f).unwrap_or_else(|| Err(ApplyError::NoProject.said()))
}

/// Why `apply_transaction` didn't commit.
#[derive(Debug, PartialEq)]
pub(crate) enum ApplyError {
    NoProject,
    /// Another project is open now.
    OtherProject,
    /// The project moved on from the base the batch was built on.
    Moved,
    /// The batch doesn't apply, or the project is read-only: what the person is told.
    Refused(Said),
}

impl ApplyError {
    /// What the person is told: the UI resyncs on `project.moved` and the person tries again.
    pub(crate) fn said(&self) -> Said {
        match self {
            Self::NoProject => Said::code("project.none"),
            Self::OtherProject | Self::Moved => Said::code("project.moved"),
            Self::Refused(s) => s.clone(),
        }
    }
}

/// The open project is `project_id`, at `revision` when given.
fn check(open: &OpenProject, project_id: &str, revision: Option<u64>) -> Result<(), ApplyError> {
    if open.document.project().id != project_id {
        return Err(ApplyError::OtherProject);
    }
    if revision.is_some_and(|r| r != open.document.revision()) {
        return Err(ApplyError::Moved);
    }
    Ok(())
}

/// Commits `tx` to the open project `project_id` by the path every edit takes (one undo step, the engine feed, the
/// save). With `base` (the revision the caller's copy had), a project that moved on refuses it: refused, never rebased,
/// since ops built on an older copy can be wrong after other edits. None for callers with no copy.
pub(crate) fn apply_transaction(
    state: &ProjectSessionState,
    project_id: &str,
    tx: &Transaction,
    base: Option<u64>,
) -> Result<SnapshotPatch, ApplyError> {
    on_open(state, |open| {
        check(open, project_id, base)?;
        apply_and_commit(open, tx).map_err(ApplyError::Refused)
    })
    .unwrap_or(Err(ApplyError::NoProject))
}
