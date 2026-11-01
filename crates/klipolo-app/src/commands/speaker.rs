//! Follow the speaker from the clip card: thin commands over `klipolo_app::speaker`. `speaker_estimate` prices a clip
//! in a format before anything runs (the card shows it); `speaker_follow` runs it, its progress going out on
//! `speaker://progress`, and lands the clip's path as one undo step on the project as it is once the run is done (a
//! hand edit meanwhile is kept). No command or event carries a key or a token.

use std::path::Path;
use std::sync::Arc;

use klipolo_analysis::subject::PERSON;
use klipolo_app::chat::Projects;
use klipolo_app::speaker::{NO_ONE, PROGRESS_EVENT, Speaker, SpeakerEstimate, SpeakerProgress};
use klipolo_model::coded::Said;
use klipolo_model::history::SnapshotPatch;
use klipolo_model::project::FormatKind;
use tauri::{AppHandle, Emitter, Manager, State};

use super::settings::CredentialsState;
use crate::project::{ProjectSessionState, build_and_apply};

/// The app's speaker following.
pub(crate) type SpeakerHandle = Arc<Speaker>;

/// The undo step a follow lands as.
const UNDO: &str = "clip.focus.speaker.undo";

/// The app's speaker following: SAM through the saved credentials, tracked footage kept in the analysis cache under
/// `cache_root`, the small copies made in the app's cache folder.
pub(crate) fn load(
    app: &AppHandle,
    credentials: CredentialsState,
    cache_root: &Path,
) -> SpeakerHandle {
    let work_root = app.path().app_cache_dir().map_or_else(
        |_| std::env::temp_dir().join("klipolo").join("speaker-work"),
        |dir| dir.join("speaker-work"),
    );
    let progress = {
        let app = app.clone();
        Arc::new(move |p: SpeakerProgress| {
            if let Err(error) = app.emit(PROGRESS_EVENT, &p) {
                tracing::warn!(%error, "speaker progress event");
            }
        })
    };
    Arc::new(Speaker::for_app(
        credentials,
        cache_root.to_path_buf(),
        work_root,
        progress,
    ))
}

/// What following the speaker costs for clip `clip_id` in `format`, before it runs: free for footage tracked before,
/// and why it can't run (no key or code, more than an access code's credit left).
#[tauri::command]
pub(crate) async fn speaker_estimate(
    speaker: State<'_, SpeakerHandle>,
    projects: State<'_, ProjectSessionState>,
    clip_id: String,
    format: FormatKind,
) -> Result<SpeakerEstimate, Said> {
    let now = Projects::now(&*projects).ok_or_else(|| Said::code("project.none"))?;
    Ok(speaker
        .estimate(&now.project, &[clip_id], &[format], PERSON)
        .await)
}

/// Follows the speaker in clip `clip_id` in `format`: what changed, as `project_apply` gives it, or why it couldn't
/// (one sentence).
#[tauri::command]
pub(crate) async fn speaker_follow(
    speaker: State<'_, SpeakerHandle>,
    projects: State<'_, ProjectSessionState>,
    clip_id: String,
    format: FormatKind,
) -> Result<SnapshotPatch, Said> {
    let now = Projects::now(&*projects).ok_or_else(|| Said::code("project.none"))?;
    let followed = speaker
        .follow(
            &now.project,
            std::slice::from_ref(&clip_id),
            &[format],
            PERSON,
        )
        .await
        .map_err(Said::Text)?;
    build_and_apply(&projects, Some(&now.project.id), None, UNDO, |p| {
        let ops = followed.ops(p);
        if ops.is_empty() {
            return Err(if followed.not_found.is_empty() {
                Said::code("model.notFound.clip")
            } else {
                Said::from(NO_ONE)
            });
        }
        Ok(ops)
    })
    .map_err(|e| e.said())
}
