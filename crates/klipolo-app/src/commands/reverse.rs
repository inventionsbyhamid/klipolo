//! Reverse's commands: write a clip's footage backward (`klipolo_app::media::reverse`) with its progress on a channel,
//! stop it, and throw away a copy the page didn't use. The page applies the copy itself (`ReverseClip`, one undo step
//! with the new file) once it has checked the project and the clip are still there. One copy at a time per clip;
//! another project opening, or this one closing, stops them all (`ReverseJobs::stop_all`), and a copy finished for a
//! project no longer open is deleted, never handed over.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_app::chat::Projects;
use klipolo_app::media::reverse::{self, ReverseFailure};
use klipolo_model::coded::{CodedError, Said};
use klipolo_model::project::Asset;
use klipolo_model::time::Us;
use klipolo_model::timeline::ClipSource;
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;
use ts_rs::TS;

use crate::media::probe_asset;
use crate::project::ProjectSessionState;

/// The copies being made, by clip id: each one's stop. Shared with the project switch's hook.
#[derive(Clone, Default)]
pub(crate) struct ReverseJobs(Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>);

impl ReverseJobs {
    /// A stop for a new copy of `clip_id`; None when one is being made.
    fn start(&self, clip_id: &str) -> Option<Arc<AtomicBool>> {
        let mut jobs = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if jobs.contains_key(clip_id) {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        jobs.insert(clip_id.to_owned(), stop.clone());
        Some(stop)
    }

    fn end(&self, clip_id: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(clip_id);
    }

    fn stop(&self, clip_id: &str) {
        if let Some(stop) = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(clip_id)
        {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// Stops every copy being made: the open project is changing.
    pub(crate) fn stop_all(&self) {
        for stop in self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

/// A clip's footage written backward: the new file as an asset (not yet in the project), and the stretch of the clip's
/// file it holds (`ReverseCopy`).
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReversedCopy {
    pub asset: Asset,
    pub from_us: Us,
    pub to_us: Us,
}

fn said(failure: ReverseFailure) -> Said {
    match failure {
        ReverseFailure::NotImported => Said::code("model.reverse.recording"),
        ReverseFailure::Missing { name } => {
            CodedError::new("reverse.missing").with("name", name).into()
        }
        ReverseFailure::NoEncoder => Said::code("reverse.noEncoder"),
        ReverseFailure::Stopped => Said::code("reverse.stopped"),
        ReverseFailure::Failed { name } => failed(name),
    }
}

fn failed(name: String) -> Said {
    CodedError::new("reverse.failed").with("name", name).into()
}

/// Writes clip `clip_id` of the open project backward beside it and answers the copy; `progress` hears 0 to 1, at
/// most every whole percent. Refused while that clip's copy is being made. Stopped when the project changes; a copy
/// that ends after it did is deleted and answers `reverse.stopped`.
#[tauri::command]
pub(crate) async fn clip_reverse(
    state: State<'_, ProjectSessionState>,
    jobs: State<'_, ReverseJobs>,
    clip_id: String,
    progress: Channel<f64>,
) -> Result<ReversedCopy, Said> {
    let now = state.now().ok_or_else(|| Said::code("project.none"))?;
    if now.read_only {
        return Err(Said::code("model.readOnly"));
    }
    let (project_id, file) = (now.project.id.clone(), now.file.clone());
    // The clip's file's name, for a failure's sentence.
    let name = (now.project.timeline.main.iter())
        .find(|c| c.id == clip_id)
        .and_then(|c| match &c.source {
            ClipSource::Asset { id } => now.project.asset(id).map(|a| a.name.clone()),
            ClipSource::Recording { .. } => None,
        })
        .unwrap_or_default();
    let stop = jobs
        .start(&clip_id)
        .ok_or_else(|| Said::code("reverse.busy"))?;
    let id = clip_id.clone();
    let made = tauri::async_runtime::spawn_blocking(move || {
        let mut said = -1i64;
        let mut tell = |done: f64| {
            let percent = (done * 100.0).floor() as i64;
            if percent > said {
                said = percent;
                let _ = progress.send(done);
            }
        };
        reverse::make(&now.project, &now.file, &id, &stop, &mut tell)
    })
    .await;
    jobs.end(&clip_id);
    let made = made
        .map_err(|error| {
            tracing::warn!(%error, "reversing a clip stopped unexpectedly");
            failed(name.clone())
        })?
        .map_err(said)?;
    // Made for a project that is no longer open (a switch stops the copy, but it may have just ended): deleted.
    let open = state.now();
    if !open
        .as_ref()
        .is_some_and(|n| n.project.id == project_id && n.file == file)
    {
        reverse::discard_unused(open.as_ref().map(|n| &*n.project), &made.path);
        return Err(Said::code("reverse.stopped"));
    }
    let asset = probe_asset(&made.path).map_err(|error| {
        tracing::warn!(%error, "a reversed copy couldn't be read back");
        let _ = std::fs::remove_file(&made.path);
        failed(name)
    })?;
    Ok(ReversedCopy {
        asset,
        from_us: made.from_us,
        to_us: made.to_us,
    })
}

/// Stops the copy of clip `clip_id` being made (its `clip_reverse` answers `reverse.stopped`); nothing when none is.
#[tauri::command]
pub(crate) fn clip_reverse_stop(jobs: State<'_, ReverseJobs>, clip_id: String) {
    jobs.stop(&clip_id);
}

/// Deletes reversed copy `path` the page didn't use (`reverse::discard_unused`: only a reversed copy, never one the open
/// project plays).
#[tauri::command]
pub(crate) fn clip_reverse_discard(state: State<'_, ProjectSessionState>, path: String) {
    let now = state.now();
    reverse::discard_unused(now.as_ref().map(|n| &*n.project), Path::new(&path));
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::ReverseJobs;

    /// One copy a clip at a time; another project opening stops every copy being made, and the jobs then start anew.
    #[test]
    fn a_project_change_stops_every_copy_being_made() {
        let jobs = ReverseJobs::default();
        let a = jobs.start("a").unwrap();
        let b = jobs.start("b").unwrap();
        assert!(jobs.start("a").is_none(), "one copy a clip");
        jobs.stop_all();
        assert!(a.load(Ordering::Relaxed) && b.load(Ordering::Relaxed));
        jobs.end("a");
        assert!(jobs.start("a").is_some());
    }
}
