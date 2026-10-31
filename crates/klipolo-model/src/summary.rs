//! The Home screen's view of a saved project.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::coded::CodedError;
use crate::error::ModelError;
use crate::ids::Id;
use crate::project::{AssetKind, FormatKind, Project};
use crate::query::video_duration;
use crate::time::Us;
use crate::timeline::ClipSource;

/// One row in the Home screen's project list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectSummary {
    pub id: Id,
    pub name: String,
    pub updated_at_ms: u64,
    pub duration_us: Us,
    pub formats: Vec<FormatKind>,
    pub clip_count: u32,
    /// A picture for the project, when it has one: its first clip's picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub thumbnail: Option<ThumbnailRef>,
    /// Why the project can't be opened, as a code the UI words (`problem`): listed rather than left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub problem: Option<CodedError>,
    /// The row is a guess (an old summary) or a placeholder (no summary, so no name yet) while the project file is read
    /// in the background; the real row follows as a `project-summary` event. The project opens either way.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// Where a project's picture comes from: a frame of one of its files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThumbnailRef {
    /// The file, as the project names it (`Asset::path`).
    pub path: String,
    /// The frame's time in that file.
    pub at_us: Us,
}

impl ProjectSummary {
    pub fn of(project: &Project, updated_at_ms: u64) -> Self {
        Self {
            id: project.id.clone(),
            name: project.name.clone(),
            updated_at_ms,
            duration_us: video_duration(&project.timeline),
            formats: project.settings.formats.clone(),
            clip_count: u32::try_from(project.timeline.main.len()).unwrap_or(u32::MAX),
            thumbnail: thumbnail(project),
            problem: None,
            pending: false,
        }
    }

    /// A project this build can't open: its name when it is known, otherwise its folder's (`id`), and why.
    pub fn unreadable(
        id: Id,
        name: Option<String>,
        updated_at_ms: u64,
        problem: CodedError,
    ) -> Self {
        Self {
            name: name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| id.clone()),
            id,
            updated_at_ms,
            duration_us: 0,
            formats: Vec::new(),
            clip_count: 0,
            thumbnail: None,
            problem: Some(problem),
            pending: false,
        }
    }
}

/// Why Home can't open a project (`ProjectSummary::problem`): the codes the UI words, each in [`MESSAGES`]. The
/// version codes are [`ModelError::coded`]'s. A file that can't be opened or read is `project.cantOpen`, whose next step
/// (check it's still there and that Klipolo may open it) fixes it; a file that reads but doesn't parse or hold a valid
/// project is one `project.damaged`, never the validation code behind it (`model.layer.placement` means nothing on
/// Home).
///
/// [`MESSAGES`]: crate::messages::MESSAGES
pub mod problem {
    use crate::messages::code;

    /// Its file can't be opened or read (no permission, a disk error, deleted meanwhile).
    pub const CANT_OPEN: &str = code!("project.cantOpen");
    /// Its file doesn't parse, or doesn't hold a valid project.
    pub const DAMAGED: &str = code!("project.damaged");
    /// It was made with a newer Klipolo: updating opens it.
    pub const NEWER: &str = code!("model.newerVersion");
    /// It was made with an older test build, which this one doesn't open.
    pub const OLDER: &str = code!("model.olderVersion");
}

/// The problem of a project file that doesn't load (`load_project`'s error).
pub fn load_problem(error: &ModelError) -> CodedError {
    CodedError::new(match error {
        ModelError::OlderVersion => problem::OLDER,
        ModelError::UnsupportedVersion(_) | ModelError::NewerFeatures(_) => problem::NEWER,
        ModelError::Parse(_) | ModelError::Invalid(_) | ModelError::NotFound { .. } => {
            problem::DAMAGED
        }
    })
}

/// The first clip's picture at the clip's first frame: a recording's main footage (its screen, else its camera) at its
/// own time there, or a video or image.
fn thumbnail(project: &Project) -> Option<ThumbnailRef> {
    let clip = project.timeline.main.first()?;
    let (asset_id, at_us) = match &clip.source {
        ClipSource::Asset { id } => (id, clip.source_in_us),
        ClipSource::Recording { id } => {
            let (run, at) = project
                .recording(id)?
                .primary_source()?
                .run_or_nearest(clip.source_in_us)?;
            (&run.asset_id, at)
        }
    };
    let asset = project
        .asset(asset_id)
        .filter(|a| a.kind != AssetKind::Audio)?;
    Some(ThumbnailRef {
        path: asset.path.clone(),
        at_us,
    })
}
