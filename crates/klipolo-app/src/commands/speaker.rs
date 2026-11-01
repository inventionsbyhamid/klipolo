//! Follow the speaker from the clip card: thin commands over `klipolo_app::speaker`. `speaker_estimate` prices a clip
//! in a format before anything runs (the card shows it); `speaker_follow` runs it, its progress going out on
//! `speaker://progress`, and lands the clip's path as one undo step on the project as it is once the run is done (a
//! hand edit meanwhile is kept). `speaker_keep` keeps one person, both, or whoever speaks in frame, made again from
//! what the follow tracked and matched, at no charge, as one undo step. No command or event carries a key or a token.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use klipolo_analysis::subject::PERSON;
use klipolo_app::chat::Projects;
use klipolo_app::speaker::{
    NO_ONE, PROGRESS_EVENT, Speaker, SpeakerEstimate, SpeakerFollowed, SpeakerProgress, Voices,
};
use klipolo_model::coded::Said;
use klipolo_model::fill_focus::Keep;
use klipolo_model::history::SnapshotPatch;
use klipolo_model::project::FormatKind;
use tauri::{AppHandle, Emitter, Manager, State};

use super::settings::CredentialsState;
use crate::project::{ProjectSessionState, build_and_apply};

/// The app's speaker following.
pub(crate) type SpeakerHandle = Arc<Speaker>;

/// The undo step a follow lands as.
const UNDO: &str = "clip.focus.speaker.undo";
/// The undo step a change of who is kept lands as.
const KEEP_UNDO: &str = "clip.focus.keep.undo";

/// The app's speaker following: SAM through the saved credentials, tracked footage kept in the analysis cache under
/// `cache_root`, the small copies made in the app's cache folder, and whoever speaks kept in frame from `voices` (the
/// clips' analysis).
pub(crate) fn load(
    app: &AppHandle,
    credentials: CredentialsState,
    cache_root: &Path,
    voices: Arc<dyn Voices>,
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
    Arc::new(
        Speaker::for_app(credentials, cache_root.to_path_buf(), work_root, progress)
            .with_voices(voices),
    )
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

/// Follows the speaker in clip `clip_id` in `format`: what changed, as `project_apply` gives it, with why the crop
/// follows the main person when who speaks couldn't be told; or why it couldn't run (one sentence).
#[tauri::command]
pub(crate) async fn speaker_follow(
    speaker: State<'_, SpeakerHandle>,
    projects: State<'_, ProjectSessionState>,
    clip_id: String,
    format: FormatKind,
) -> Result<SpeakerFollowed, Said> {
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
    .map(|change| SpeakerFollowed {
        change,
        note: followed.note.clone(),
    })
    .map_err(|e| e.said())
}

/// Keeps `keep` in frame in clip `clip_id` in `format`, made from what its follow tracked and matched: what changed,
/// as `project_apply` gives it, or why it couldn't (one sentence). Never pays: one person or both need only the
/// tracking, and footage that isn't tracked in the cache any more is refused (`speaker::NOT_TRACKED`).
#[tauri::command]
pub(crate) async fn speaker_keep(
    speaker: State<'_, SpeakerHandle>,
    projects: State<'_, ProjectSessionState>,
    clip_id: String,
    format: FormatKind,
    keep: Keep,
) -> Result<SnapshotPatch, Said> {
    if !keep.is_valid() {
        return Err(Said::code("model.clip.fillFocus"));
    }
    let now = Projects::now(&*projects).ok_or_else(|| Said::code("project.none"))?;
    let keeps = BTreeMap::from([(clip_id.clone(), keep)]);
    // From what the follow tracked and matched only: it never pays.
    let followed = speaker
        .keep(&now.project, std::slice::from_ref(&clip_id), &[format], &keeps)
        .await
        .map_err(Said::Text)?;
    build_and_apply(&projects, Some(&now.project.id), None, KEEP_UNDO, |p| {
        let ops = followed.ops(p);
        if ops.is_empty() {
            return Err(Said::code("model.notFound.clip"));
        }
        Ok(ops)
    })
    .map_err(|e| e.said())
}
