//! Export: plan the files with a size and time estimate, start a batch on the one export slot, stop it,
//! and say what it wrote. The worker runs out of process (this app's own binary, `--export-worker`); the engine runs
//! it, segment by segment, and renames each finished file into place, never over an existing file.
//!
//! - **One file per format**, `<project name> 16x9.mp4`, in the folder the dialog shows (`Movies / Klipolo` unless the
//!   person picked another), `<name> 16x9 2.mp4` when that name is taken (`naming`).
//! - **What it draws is what the preview draws**: the open project as saved, the captions frozen from the finished
//!   transcripts the preview has (`captions`), and the fonts checked first (`fonts`).
//! - **An export that stops resumes**: a run that fails or pauses (a full disk, a disk gone, a busy encoder), Quit, a
//!   crash and the Mac shutting down all keep its job and every finished part (`cleanup`), and the dialog lists it to
//!   resume or discard (`jobs`). Only Cancel and Discard delete one. Quit while one runs asks first, and pauses it (the
//!   app's `commands::export`).
//!
//! No Tauri types here: the commands in `commands/export.rs` are thin layers over it, and `tests/export_mac.rs` drives
//! it with the real worker.

mod captions;
mod cleanup;
mod estimate;
mod fonts;
pub mod jobs;
mod naming;
mod run;
mod start;
#[cfg(test)]
mod start_tests;
mod state;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use klipolo_engine::api::{ExportQuality, ExportRequest, PlannedFile};
use klipolo_engine::export::contrib::JobContribs;
use klipolo_engine::export::jobs::jobs_root;
use klipolo_engine::export::runner::Batch;
use klipolo_model::coded::{CodedError, Said};
use klipolo_model::project::Project;
use klipolo_model::query::video_duration;
use klipolo_model::speech::Speech;
use klipolo_platform::durable::{self, DiskReason};
use serde::Serialize;
use ts_rs::TS;

pub use captions::export_speech;
pub use cleanup::{pause_on_quit, sweep_folder, tidy_jobs};
pub use estimate::{Estimate, apple_silicon, estimate, screen_share};
pub use fonts::{missing_fonts, refuse_missing};
pub use naming::{format_word, plan_files};
pub use run::{coded, resume, run, said};
pub use start::{StartSteps, Started, create_job, resume_on_slot, start_on_slot};
pub use state::{ExportState, Releases};

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
    /// Master's ProRes is encoded on the CPU here (no media engine: an Intel Mac, Windows, Linux), so it is slower.
    pub slow_master: bool,
    /// Some file of the project has its words heard in full: a landscape file can also save its captions as a file.
    pub can_save_captions: bool,
    /// A Maximum plan on a computer with no 10-bit HEVC encoder: its files are H.264 at Maximum's higher bitrate.
    pub maximum_as_h264: bool,
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
/// engine), and whether a landscape file can also save its captions as a file (`speech`: what was heard in the project's
/// files), and whether Maximum makes H.264 here (`hevc_main10`: this computer has a 10-bit HEVC encoder).
pub fn plan(
    project: &Project,
    speech: &Speech,
    req: &ExportRequest,
    dir: &Path,
    apple_silicon: bool,
    hevc_main10: bool,
) -> ExportPlan {
    let files = plan_files(project, &req.formats, req.quality, dir);
    let cost = estimate(
        &files,
        req.quality,
        project.settings.fps,
        video_duration(&project.timeline),
        apple_silicon,
        screen_share(project),
    );
    ExportPlan {
        files,
        bytes: cost.bytes,
        seconds: cost.seconds,
        folder: folder_label(dir),
        slow_master: !apple_silicon,
        can_save_captions: any_finished(speech, project),
        maximum_as_h264: req.quality == ExportQuality::Maximum && !hevc_main10,
    }
}

/// Some file of `project` has its words heard in full in `speech`: a caption file has words to save.
fn any_finished(speech: &Speech, project: &Project) -> bool {
    speech
        .transcripts
        .iter()
        .any(|(asset, t)| t.complete && !t.words.is_empty() && project.asset(asset).is_some())
}

/// The batch an export of `project` runs: its files planned in `dir`, the captions frozen from `speech` (its finished
/// transcripts only) and in `text_language`, decoded sound read from (and kept in) `pcm_dir`, and the caption file asked
/// for (saved beside each landscape video, from the same words). The job keeps its own frozen copy of the project: edits
/// made while it runs aren't in it.
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
        // SRT or WebVTT beside each landscape video, when the person asked for one.
        caption_file: req.caption_file,
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

/// Makes the export folder `dir` and its jobs folder, where an export's job is written, and checks a file can be
/// written there: a folder on a disk that can't be written to, or one Klipolo isn't allowed to write to, is said by its
/// name before the export starts (`folder_error`), never as an export that didn't start.
pub fn make_folder(dir: &Path) -> Result<(), Said> {
    make_folder_with(
        dir,
        |path| std::fs::create_dir_all(path),
        |path| {
            std::fs::write(path, b"")?;
            std::fs::remove_file(path)
        },
    )
}

/// `make_folder` through `create` (a folder and those above it) and `probe` (a file written there and removed). The
/// probe's name is this process's own, so no other Klipolo's file is touched.
fn make_folder_with(
    dir: &Path,
    create: impl Fn(&Path) -> io::Result<()>,
    probe: impl Fn(&Path) -> io::Result<()>,
) -> Result<(), Said> {
    let jobs = jobs_root(dir);
    create(dir)
        .and_then(|()| create(&jobs))
        .and_then(|()| probe(&jobs.join(format!(".probe-{}", std::process::id()))))
        .map_err(|e| folder_error(&e, dir))
}

/// Why the export folder `dir` couldn't be made or written to, as a code naming it.
pub fn folder_error(e: &io::Error, dir: &Path) -> Said {
    tracing::warn!(error = %e, "the export folder couldn't be made or written to");
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
    use klipolo_model::captions::sidecar::CaptionFile;
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
        let (project, speech) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        let dir = tempfile::tempdir().unwrap();
        let two = request(
            &[FormatKind::Portrait, FormatKind::Landscape],
            ExportQuality::High,
        );
        let planned = plan(&project, &speech, &two, dir.path(), false, true);
        let names: Vec<&str> = planned.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Pages 16x9.mp4", "Pages 9x16.mp4"], "16:9 first");
        assert_eq!(
            (planned.files[1].width, planned.files[1].height),
            (1080, 1920)
        );
        assert!(planned.bytes > 0 && planned.seconds > 0.0);
        let one = plan(
            &project,
            &speech,
            &request(&[FormatKind::Landscape], ExportQuality::High),
            dir.path(),
            false,
            true,
        );
        assert!(planned.bytes > one.bytes, "two files take more than one");
        let smaller = plan(
            &project,
            &speech,
            &request(&[FormatKind::Landscape], ExportQuality::Standard),
            dir.path(),
            false,
            true,
        );
        assert!(smaller.bytes < one.bytes, "Smaller file is smaller");
        assert_eq!(planned.folder, folder_label(dir.path()));
        // Nothing is written by a plan.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn master_is_slower_on_a_computer_without_a_media_engine() {
        let (project, speech) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        let dir = tempfile::tempdir().unwrap();
        let master = request(&[FormatKind::Landscape], ExportQuality::Master);
        let intel = plan(&project, &speech, &master, dir.path(), false, true);
        assert!(intel.slow_master, "the CPU encodes ProRes here");
        assert!(intel.files[0].name.ends_with(".mov"), "Master writes .mov");
        assert!(
            !plan(&project, &speech, &master, dir.path(), true, true).slow_master,
            "a media engine encodes it"
        );
    }

    /// A caption file is offered once some file of the project has its words heard in full: not while it is still
    /// being transcribed, not when nothing was said, and not for a file another project uses.
    #[test]
    fn a_caption_file_is_offered_once_a_file_has_finished_words() {
        let (project, speech) =
            klipolo_engine::test_support::captions_project("caption.bold", &[("Hello", 1.0, 1.4)]);
        let dir = tempfile::tempdir().unwrap();
        let req = request(&[FormatKind::Landscape], ExportQuality::High);
        let offered = |speech: &Speech| {
            plan(&project, speech, &req, dir.path(), false, true).can_save_captions
        };
        assert!(offered(&speech));
        assert!(!offered(&Speech::default()), "nothing heard yet");
        let mut half = speech.clone();
        let mut heard = (*half.transcripts["v"]).clone();
        heard.complete = false;
        half.transcripts.insert("v".into(), Arc::new(heard));
        assert!(!offered(&half), "still being transcribed");
        let (_, silent) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        assert!(!offered(&silent), "no words to save");
        let mut elsewhere = Speech::default();
        elsewhere
            .transcripts
            .insert("other".into(), speech.transcripts["v"].clone());
        assert!(!offered(&elsewhere), "another project's file");
    }

    /// Maximum makes H.264 where there is no 10-bit HEVC encoder, and the plan says so; no other quality does.
    #[test]
    fn maximum_says_when_it_makes_h264_for_want_of_a_10_bit_hevc_encoder() {
        let (project, speech) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        let dir = tempfile::tempdir().unwrap();
        let maximum = request(&[FormatKind::Landscape], ExportQuality::Maximum);
        let planned = |req: &ExportRequest, hevc_main10: bool| {
            plan(&project, &speech, req, dir.path(), false, hevc_main10).maximum_as_h264
        };
        assert!(planned(&maximum, false), "no HEVC 10-bit encoder here");
        assert!(!planned(&maximum, true));
        let high = request(&[FormatKind::Landscape], ExportQuality::High);
        assert!(!planned(&high, false), "High is H.264 anyway");
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
        // No caption file unless asked, and never a stand-in font unless asked.
        assert!(batch.caption_file.is_none());
        assert!(!batch.allow_stand_ins);
    }

    #[test]
    fn the_batch_carries_the_caption_file_asked_for() {
        let (project, speech) =
            klipolo_engine::test_support::captions_project("caption.bold", &[("Hello", 1.0, 1.4)]);
        let dir = tempfile::tempdir().unwrap();
        for kind in [CaptionFile::Srt, CaptionFile::Vtt] {
            let req = ExportRequest {
                caption_file: Some(kind),
                ..request(&[FormatKind::Landscape], ExportQuality::High)
            };
            let batch = freeze(
                Arc::new(project.clone()),
                &speech,
                "en".into(),
                &req,
                dir.path(),
                None,
            );
            assert_eq!(batch.caption_file, Some(kind));
        }
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

    #[test]
    fn a_folder_klipolo_cant_write_in_is_named_before_the_export_starts() {
        let dir = Path::new("/Volumes/Card/Exports");
        let named = |code: &str| {
            Err(Said::Coded(
                CodedError::new(code).with("folder", "Card / Exports"),
            ))
        };
        // The folder is there, but Klipolo isn't allowed to make its jobs folder in it.
        let no_jobs = |path: &Path| {
            if path == dir {
                Ok(())
            } else {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        };
        assert_eq!(
            make_folder_with(dir, no_jobs, |_| Ok(())),
            named("exports.folder.denied")
        );
        // Both folders are there from before, but nothing can be written on their disk.
        let read_only = |_: &Path| Err(io::Error::from(io::ErrorKind::ReadOnlyFilesystem));
        assert_eq!(
            make_folder_with(dir, |_| Ok(()), read_only),
            named("exports.folder.readOnly")
        );
        // Where it can write: the folder, then its jobs folder, then a file written in that.
        let done = std::cell::RefCell::new(Vec::new());
        let step = |path: &Path| {
            done.borrow_mut().push(path.to_path_buf());
            Ok(())
        };
        let made = make_folder_with(dir, step, step);
        assert_eq!(made, Ok(()));
        let done = done.into_inner();
        assert_eq!(done[..2], [dir.to_path_buf(), dir.join(".jobs")]);
        assert_eq!(done[2].parent(), Some(dir.join(".jobs").as_path()));
    }

    #[test]
    fn making_the_folder_leaves_its_jobs_folder_and_nothing_in_it() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("Klipolo");
        assert_eq!(make_folder(&dir), Ok(()));
        let jobs = dir.join(".jobs");
        assert!(jobs.is_dir());
        assert_eq!(
            std::fs::read_dir(&jobs).unwrap().count(),
            0,
            "the probe is gone"
        );
    }
}
