//! Export: plan the files with a size and time estimate, start a batch on the one export slot, stop it,
//! and say what it wrote. The worker runs out of process (this app's own binary, `--export-worker`); the engine runs
//! it, segment by segment, and renames each finished file into place, never over an existing file.
//!
//! - **One file per format**, `<project name> 16x9.mp4`, in the folder the dialog shows (`Movies / Klipolo` unless the
//!   person picked another), `<name> 16x9 2.mp4` when that name is taken (`naming`).
//! - **What it draws is what the preview draws**: the open project as saved, the captions frozen from the finished
//!   transcripts the preview has (`captions`), and the fonts checked first (`fonts`).
//! - **Nothing half made is left behind**: Cancel deletes the job and its parts, and so does a run that fails, as
//!   Klipolo doesn't resume one (`run`). Quit while one runs asks first and stops it (the app's `commands::export`).
//!
//! No Tauri types here: the commands in `commands/export.rs` are thin layers over it, and `tests/export_mac.rs` drives
//! it with the real worker.

mod captions;
mod estimate;
mod fonts;
mod naming;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use klipolo_engine::api::{ExportRequest, PlannedFile};
use klipolo_engine::export::contrib::JobContribs;
use klipolo_engine::export::runner::Batch;
use klipolo_model::coded::{CodedError, Said};
use klipolo_model::project::Project;
use klipolo_model::query::video_duration;
use klipolo_model::speech::Speech;
use klipolo_platform::durable::{self, DiskReason};
use serde::Serialize;
use ts_rs::TS;

pub use captions::export_speech;
pub use estimate::{Estimate, apple_silicon, estimate};
pub use fonts::{missing_fonts, refuse_missing};
pub use naming::{format_word, plan_files};

/// What an export will write, and what it costs (an honest estimate before it starts).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportPlan {
    pub files: Vec<PlannedFile>,
    /// The most the files take together, in bytes.
    pub bytes: u64,
    /// About how long the export takes on this computer (the time left shown while it runs is measured).
    pub seconds: f64,
    /// Where the files go, as the dialog says it: "Movies / Klipolo".
    pub folder: String,
}

/// A file the last export put in place, as the finished dialog lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportedFile {
    /// Its file name, as written (a taken name gets " 2").
    pub name: String,
    /// Its size on the disk, in bytes.
    pub bytes: u64,
}

/// `Klipolo` in the videos folder (`~/Movies/Klipolo` on a Mac), beside the projects: where exports go unless the
/// person picks another folder. Made when an export starts, not when the dialog plans one.
pub fn default_folder() -> Result<PathBuf, Said> {
    klipolo_platform::paths::videos_dir()
        .map(|videos| videos.join("Klipolo"))
        .ok_or_else(|| Said::code("exports.noVideosFolder"))
}

/// A folder as the dialog names it: its last two parts, "Movies / Klipolo".
pub fn folder_label(dir: &Path) -> String {
    let parts: Vec<String> = dir
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let shown = &parts[parts.len().saturating_sub(2)..];
    if shown.is_empty() {
        dir.display().to_string()
    } else {
        shown.join(" / ")
    }
}

/// What exporting `req`'s formats of `project` into `dir` writes and costs (`apple_silicon`: this Mac has a media
/// engine).
pub fn plan(project: &Project, req: &ExportRequest, dir: &Path, apple_silicon: bool) -> ExportPlan {
    let files = plan_files(project, &req.formats, req.quality, dir);
    let cost = estimate(
        &files,
        req.quality,
        project.settings.fps,
        video_duration(&project.timeline),
        apple_silicon,
    );
    ExportPlan {
        files,
        bytes: cost.bytes,
        seconds: cost.seconds,
        folder: folder_label(dir),
    }
}

/// The batch an export of `project` runs: its files planned in `dir`, the captions frozen from `speech` (its finished
/// transcripts only) and in `text_language`, decoded sound read from (and kept in) `pcm_dir`. The job keeps its own
/// frozen copy of the project: edits made while it runs aren't in it.
pub fn freeze(
    project: Arc<Project>,
    speech: &Speech,
    text_language: Arc<str>,
    req: &ExportRequest,
    dir: &Path,
    pcm_dir: Option<PathBuf>,
) -> Batch {
    Batch {
        files: plan_files(&project, &req.formats, req.quality, dir),
        fps: project.settings.fps,
        quality: req.quality,
        speech: export_speech(speech, &project),
        text_language,
        // The job keeps its own frozen copy; the open project moves on.
        project: Arc::unwrap_or_clone(project),
        pcm_dir,
        // What it draws besides the built-ins: none in Klipolo yet.
        contribs: JobContribs::default(),
        // No caption file beside the videos: the captions are burned in.
        caption_file: None,
        allow_stand_ins: req.allow_stand_ins,
        // No recordings yet: no screens' activity, no cursors.
        activity_dir: None,
        allow_missing_cursor: false,
    }
}

/// Refuses an export of a project with nothing to show (`exports.empty`): a project of layers alone exports as long as
/// its layers.
pub fn refuse_empty(project: &Project) -> Result<(), Said> {
    if video_duration(&project.timeline) == 0 {
        return Err(Said::code("exports.empty"));
    }
    Ok(())
}

/// Why the export folder `dir` couldn't be made, as a code naming it.
pub fn folder_error(e: &io::Error, dir: &Path) -> Said {
    tracing::warn!(error = %e, "the export folder couldn't be made");
    let named = |code: &str| Said::Coded(CodedError::new(code).with("folder", folder_label(dir)));
    match durable::classify(e) {
        DiskReason::Full => Said::code("exports.folder.diskFull"),
        DiskReason::ReadOnly => named("exports.folder.readOnly"),
        DiskReason::Denied => named("exports.folder.denied"),
        DiskReason::Locked | DiskReason::Other => named("exports.folder.failed"),
    }
}

#[cfg(test)]
mod tests {
    use klipolo_engine::api::ExportQuality;
    use klipolo_model::project::FormatKind;

    use super::*;

    fn request(formats: &[FormatKind], quality: ExportQuality) -> ExportRequest {
        ExportRequest {
            formats: formats.to_vec(),
            quality,
            caption_file: None,
            allow_stand_ins: false,
            replaces: None,
            allow_missing_cursor: false,
        }
    }

    #[test]
    fn a_folder_is_named_by_its_last_two_parts() {
        assert_eq!(
            folder_label(Path::new("/Users/rosa/Movies/Klipolo")),
            "Movies / Klipolo"
        );
        assert_eq!(folder_label(Path::new("/Volumes/Card")), "Volumes / Card");
        assert_eq!(folder_label(Path::new("/Exports")), "Exports");
    }

    #[test]
    fn the_plan_lists_each_format_with_its_size_cost_and_folder() {
        let (project, _) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        let dir = tempfile::tempdir().unwrap();
        let two = request(
            &[FormatKind::Portrait, FormatKind::Landscape],
            ExportQuality::High,
        );
        let planned = plan(&project, &two, dir.path(), false);
        let names: Vec<&str> = planned.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Pages 16x9.mp4", "Pages 9x16.mp4"], "16:9 first");
        assert_eq!(
            (planned.files[1].width, planned.files[1].height),
            (1080, 1920)
        );
        assert!(planned.bytes > 0 && planned.seconds > 0.0);
        let one = plan(
            &project,
            &request(&[FormatKind::Landscape], ExportQuality::High),
            dir.path(),
            false,
        );
        assert!(planned.bytes > one.bytes, "two files take more than one");
        let smaller = plan(
            &project,
            &request(&[FormatKind::Landscape], ExportQuality::Standard),
            dir.path(),
            false,
        );
        assert!(smaller.bytes < one.bytes, "Smaller file is smaller");
        assert_eq!(planned.folder, folder_label(dir.path()));
        // Nothing is written by a plan.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_batch_freezes_the_project_its_finished_words_and_files() {
        let (project, speech) =
            klipolo_engine::test_support::captions_project("caption.bold", &[("Hello", 1.0, 1.4)]);
        let dir = tempfile::tempdir().unwrap();
        let req = request(&[FormatKind::Portrait], ExportQuality::Standard);
        let batch = freeze(
            Arc::new(project.clone()),
            &speech,
            "en".into(),
            &req,
            dir.path(),
            None,
        );
        assert_eq!(batch.project, project);
        assert_eq!(batch.fps, project.settings.fps);
        assert_eq!(batch.quality, ExportQuality::Standard);
        assert_eq!(batch.files.len(), 1);
        assert_eq!(batch.files[0].name, "Pages 9x16.mp4");
        assert!(
            batch.speech.transcripts.contains_key("v"),
            "the captions' words"
        );
        assert_eq!(&*batch.text_language, "en");
        // Klipolo saves no caption file and never draws a stand-in font unless asked.
        assert!(batch.caption_file.is_none());
        assert!(!batch.allow_stand_ins);
    }

    #[test]
    fn an_empty_project_is_refused_and_a_full_disk_is_said() {
        let mut project = Project::new("Empty");
        assert_eq!(refuse_empty(&project), Err(Said::code("exports.empty")));
        let (with_clip, _) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        project.timeline = with_clip.timeline;
        assert_eq!(refuse_empty(&project), Ok(()));
        let full = io::Error::from_raw_os_error(libc::ENOSPC);
        assert_eq!(
            folder_error(&full, Path::new("/Users/rosa/Movies/Klipolo")),
            Said::code("exports.folder.diskFull")
        );
        let denied = io::Error::from_raw_os_error(libc::EACCES);
        assert_eq!(
            folder_error(&denied, Path::new("/Users/rosa/Movies/Klipolo")),
            Said::Coded(
                CodedError::new("exports.folder.denied").with("folder", "Movies / Klipolo")
            )
        );
    }
}
