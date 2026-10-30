//! What the app holds for the preview: the open project as the engine's feed sees it, and the one engine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_engine::registry::Registry;
use klipolo_engine::render::probe::{MediaKind, probe_media};
use klipolo_engine::render::warm::Warmup;
use klipolo_engine::session::{EngineSlot, FeedRx, ProjectFeed};
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::query::video_duration;
use klipolo_model::time::{Us, secs_to_us};
use klipolo_model::timeline::{Clip, ClipSource};
use serde::Serialize;
use tokio::sync::watch;

/// The open project, published to the engine's watch feed (the newest value always wins).
pub struct ProjectState {
    feed: watch::Sender<Option<ProjectFeed>>,
    /// Bumped by every open, so the engine starts a new session even for the same project.
    opened: AtomicU64,
}

impl Default for ProjectState {
    fn default() -> Self {
        Self {
            feed: watch::channel(None).0,
            opened: AtomicU64::new(0),
        }
    }
}

impl ProjectState {
    /// The engine's side of the feed.
    pub fn feed(&self) -> FeedRx {
        self.feed.subscribe()
    }

    /// Opens `project` at revision 0: the engine shows it from its next frame.
    pub fn open(&self, project: Project) {
        let opened = self.opened.fetch_add(1, Ordering::SeqCst) + 1;
        self.feed.send_replace(Some(ProjectFeed {
            project_id: project.id.clone(),
            revision: 0,
            opened,
            project: Arc::new(project),
            speech: Arc::default(),
            text_language: Arc::from("en"),
            registry: Registry::builtin(),
        }));
    }

    /// What the UI shows about the open project, if any.
    pub fn summary(&self) -> Option<ProjectSummary> {
        self.feed.borrow().as_ref().map(|feed| ProjectSummary {
            name: feed.project.name.clone(),
            duration_us: video_duration(&feed.project.timeline),
        })
    }
}

/// The open project's name and length, for the player bar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub name: String,
    pub duration_us: Us,
}

/// The open project's name and length, or null when none is open.
#[tauri::command]
pub fn project_summary(projects: tauri::State<'_, ProjectState>) -> Option<ProjectSummary> {
    projects.summary()
}

/// The video to preview at launch, until import exists: `KLIPOLO_OPEN`, else the first argument that isn't a flag.
pub fn launch_video() -> Option<PathBuf> {
    std::env::var_os("KLIPOLO_OPEN")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::args_os()
                .skip(1)
                .find(|a| !a.to_string_lossy().starts_with('-'))
                .map(PathBuf::from)
        })
}

/// A project holding one video, whole, on the main track, in all three formats (16:9 first).
pub fn single_video_project(path: &Path) -> Result<Project, String> {
    let shown = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let info = probe_media(path).map_err(|e| format!("{shown} couldn't be opened. {e}"))?;
    if info.kind != MediaKind::Video {
        return Err(format!("{shown} isn't a video."));
    }
    let duration_us = secs_to_us(info.duration);
    let asset = Asset {
        id: "video".into(),
        kind: AssetKind::Video,
        path: path.to_string_lossy().into_owned(),
        name: shown.clone(),
        duration_us,
        width: info.width,
        height: info.height,
        fps: info.fps,
        rotation: Some(info.rotation),
        has_audio: info.has_audio,
        audio_lead_us: Some(info.audio_lead_us),
    };
    let mut project = Project::new(shown);
    project.settings.formats = vec![
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ];
    project.settings.primary_format = FormatKind::Landscape;
    project.timeline.main = vec![Clip::new(
        ClipSource::Asset {
            id: asset.id.clone(),
        },
        0,
        duration_us,
    )];
    project.assets = vec![asset];
    Ok(project)
}

/// The preview engine: started by the first attach and kept for the app's lifetime (started again if it ended).
pub struct EngineState {
    pub(crate) slot: EngineSlot,
    /// The GPU warm-up started at launch, until the engine's first start takes its backend.
    warm: Mutex<Option<Warmup>>,
    /// The app's cache folder (decoded sound, thumbnails); None when it couldn't be found.
    pub(crate) cache_dir: Option<PathBuf>,
}

impl EngineState {
    pub fn new(warm: Option<Warmup>, cache_dir: Option<PathBuf>) -> Self {
        Self {
            slot: EngineSlot::new(),
            warm: Mutex::new(warm),
            cache_dir,
        }
    }

    /// The warm-up to take the backend from (the engine's first start); afterwards there is none.
    pub(crate) fn take_warm_up(&self) -> Option<Warmup> {
        self.warm
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}
