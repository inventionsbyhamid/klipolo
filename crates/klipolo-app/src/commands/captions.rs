//! The Captions panel's commands: thin layers over `klipolo_app::captions`, on the engine's feed (so the panel shows
//! the pages the video shows). New speech on the feed goes to the page as a `speech-changed` event: it asks its pages
//! again. In debug builds, `dev_fixture_speech` gives the open project's clips a sample transcript, for checking the
//! panel by hand before analysis has run.

use std::sync::Arc;

use klipolo_app::captions::{CaptionPagesView, CaptionsCache, PagesQuery, WordView, answer, find};
use klipolo_model::project::{AssetKind, FormatKind, Project};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::{Us, secs_to_us};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::project::ProjectSessionState;

/// The event a new speech on the feed goes out on.
pub(crate) const SPEECH_CHANGED_EVENT: &str = "speech-changed";

/// Tells the page each time the feed's speech changes (not when only the revision does): its caption pages were made
/// again.
pub(crate) fn follow_speech(app: &AppHandle, projects: &ProjectSessionState) {
    let mut feed = projects.subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last: Option<Arc<Speech>> = None;
        while feed.changed().await.is_ok() {
            let now = feed.borrow_and_update().as_ref().map(|f| f.speech.clone());
            let changed = match (&last, &now) {
                (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                (None, Some(b)) => !b.transcripts.is_empty(),
                _ => false,
            };
            if now.is_some() {
                last = now;
            }
            if changed && let Err(error) = app.emit(SPEECH_CHANGED_EVENT, ()) {
                tracing::warn!(%error, "speech event");
            }
        }
    });
}

/// A window of the open project's pages in `format` (at most `MAX_PAGES`), or None when `project_id` isn't open. The
/// build, when one is needed, runs on a blocking thread.
#[tauri::command]
pub(crate) async fn captions_pages(
    app: AppHandle,
    project_id: String,
    format: FormatKind,
    query: PagesQuery,
) -> Option<CaptionPagesView> {
    let feed = app
        .try_state::<ProjectSessionState>()?
        .subscribe()
        .borrow()
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cache = app.try_state::<CaptionsCache>()?;
        answer(&cache, feed.as_ref(), &project_id, format, query)
    })
    .await
    .ok()
    .flatten()
}

/// Every place the open project's captions in `format` show `text`, once per file word (what "Fix all" fixes), or None
/// when `project_id` isn't open.
#[tauri::command]
pub(crate) async fn captions_find(
    app: AppHandle,
    project_id: String,
    format: FormatKind,
    text: String,
) -> Option<Vec<WordView>> {
    let feed = app
        .try_state::<ProjectSessionState>()?
        .subscribe()
        .borrow()
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cache = app.try_state::<CaptionsCache>()?;
        find(&cache, feed.as_ref(), &project_id, format, &text)
    })
    .await
    .ok()
    .flatten()
}

/// What the sample transcript says, over and over: plain talk with a few fillers.
const SAMPLE: &str = "So this is the studio where it all happens. Um, every piece starts as a lump of clay. \
    You wedge it first, then you center it on the wheel. Teo asked me once why I keep the cracked ones. \
    Uh, they remind me to slow down. And if you rush the drying, it tells you in the kiln.";

/// A sample transcript for every video with sound in `project`, its words spread over the file (debug builds).
fn sample_speech(project: &Project) -> Speech {
    let words: Vec<&str> = SAMPLE.split_whitespace().collect();
    let mut speech = Speech::default();
    for asset in project
        .assets
        .iter()
        .filter(|a| a.kind == AssetKind::Video && a.has_audio)
    {
        let end = asset.duration_us.min(secs_to_us(2.0 * 3600.0));
        let mut out = Vec::new();
        let mut at: Us = secs_to_us(0.5);
        for text in words.iter().cycle() {
            let len = secs_to_us(0.26);
            if at + len > end {
                break;
            }
            let bare = text
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            let kind = if bare == "um" || bare == "uh" {
                WordKind::Filler
            } else {
                WordKind::Word
            };
            out.push(Word {
                text: (*text).to_owned(),
                start_us: at,
                end_us: at + len,
                confidence: 0.9,
                kind,
            });
            at += secs_to_us(if text.ends_with('.') { 0.9 } else { 0.32 });
        }
        speech.transcripts.insert(
            asset.id.clone(),
            Arc::new(Transcript {
                key: format!("sample:{}", asset.id),
                language: "en".into(),
                model: "sample".into(),
                covered_us: asset.duration_us,
                complete: true,
                words: out,
            }),
        );
    }
    speech
}

/// Debug builds: gives the open project's clips a sample transcript, as analysis would, and says how many clips got
/// one. Refused in release builds.
#[tauri::command]
pub(crate) fn dev_fixture_speech(
    projects: State<'_, ProjectSessionState>,
) -> Result<usize, String> {
    if !cfg!(debug_assertions) {
        return Err("Sample words are for development builds only.".into());
    }
    let feed = projects.subscribe().borrow().clone();
    let feed = feed.ok_or_else(|| "Open a project first.".to_owned())?;
    let speech = sample_speech(&feed.project);
    let clips = speech.transcripts.len();
    projects.set_speech(Arc::new(speech));
    Ok(clips)
}
