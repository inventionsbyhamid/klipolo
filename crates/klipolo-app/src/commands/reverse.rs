//! Reverse's commands: write a clip's footage backward (`klipolo_app::media::reverse`) with its progress on a channel,
//! and stop it. The page applies the copy itself (`ReverseClip`, one undo step with the new file), so a clip edited or
//! removed while its copy was being made is judged by the model then. One copy at a time per clip.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_app::chat::Projects;
use klipolo_app::media::reverse::{self, ReverseFailure};
use klipolo_model::coded::{CodedError, Said};
use klipolo_model::project::Asset;
use klipolo_model::time::Us;
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;
use ts_rs::TS;

use crate::media::probe_asset;
use crate::project::ProjectSessionState;

/// The copies being made, by clip id: each one's stop.
#[derive(Default)]
pub(crate) struct ReverseJobs(Mutex<HashMap<String, Arc<AtomicBool>>>);

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
        ReverseFailure::Failed { name } => {
            CodedError::new("reverse.failed").with("name", name).into()
        }
    }
}

/// Writes clip `clip_id` of the open project backward beside it and answers the copy; `progress` hears 0 to 1, at
/// most every whole percent. Refused while that clip's copy is being made.
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
        .map_err(|_| Said::code("reverse.stopped"))?
        .map_err(said)?;
    let asset = probe_asset(&made.path).map_err(|error| {
        tracing::warn!(%error, "a reversed copy couldn't be read back");
        let _ = std::fs::remove_file(&made.path);
        Said::code("reverse.stopped")
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
