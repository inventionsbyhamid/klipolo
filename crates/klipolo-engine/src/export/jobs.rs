//! A job's progress and the jobs on disk: what `export_jobs` lists, and whether a job may still resume (its
//! settings hash against the project as saved now).

use std::path::{Path, PathBuf};

use klipolo_model::captions::sidecar::CaptionFile;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::query::video_duration;
use klipolo_model::speech::Speech;
use klipolo_model::time::RangeUs;

use super::caption_file::{frozen_name, job_captions};
use super::job::{AUDIO_BITRATE, ExportJob, JobOutput, JobState, PartStatus};
use crate::api::{ExportCompatibility, ExportJobInfo, ExportJobState, ExportQuality, PlannedFile};
use crate::audio::plan::ducking_speech;

/// `<exports>/.jobs`.
pub fn jobs_root(exports_dir: &Path) -> PathBuf {
    exports_dir.join(JOBS_DIR)
}

/// A job id is a UUID: anything else is refused before it becomes a path.
pub fn valid_job_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// 64-bit FNV-1a (stable across builds, unlike std's hasher).
fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The hash a job resumes under: the project's content (its name doesn't change the pictures), its caption pages
/// (`captions.json`'s bytes) and caption file (its kind and text), where someone speaks when a sound layer
/// ducks under it (`ExportJob::speech_ranges`; nothing when none does, so a job without ducking hashes as before), the
/// frame rate, quality and every output's format, size and encoder settings.
pub fn settings_hash(
    project: &Project,
    captions_json: &[u8],
    caption_file: Option<&(CaptionFile, String)>,
    speech: &[RangeUs],
    fps: u32,
    quality: ExportQuality,
    outputs: &[JobOutput],
) -> String {
    // Only what changes the rendered video: not the name, nor which formats the project lists (each output keeps
    // its own format, size and settings below).
    let mut content = project.clone();
    content.name.clear();
    content.settings.formats.clear();
    let shapes: Vec<_> = outputs
        .iter()
        .map(|o| (o.format, o.width, o.height, o.settings))
        .collect();
    let mut bytes =
        serde_json::to_vec(&(content, fps, quality, shapes, AUDIO_BITRATE)).unwrap_or_default();
    bytes.extend_from_slice(captions_json);
    if let Some((kind, text)) = caption_file {
        bytes.extend_from_slice(kind.extension().as_bytes());
        bytes.extend_from_slice(text.as_bytes());
    }
    if !speech.is_empty() {
        bytes.extend_from_slice(b"speech");
        for r in speech {
            bytes.extend_from_slice(&r.start.to_le_bytes());
            bytes.extend_from_slice(&r.end.to_le_bytes());
        }
    }
    format!("{:016x}", fnv64(&bytes))
}

/// A format's folder in the job folder.
pub(super) fn format_dir(format: FormatKind) -> &'static str {
    match format {
        FormatKind::Landscape => "landscape",
        FormatKind::Portrait => "portrait",
        FormatKind::Square => "square",
    }
}

/// The folder a job is written in before it is renamed into place (`ExportJob::create`): `<id>.<pid>.new`. The job
/// list ignores it (not a job id).
pub(super) fn staging_name(job_id: &str) -> String {
    format!("{job_id}.{}.new", std::process::id())
}

/// The name a discarded job's folder takes before its files are deleted (`runner::discard_job_atomically`):
/// `<id>.<pid>.gone`, in the same `.jobs` folder. The job list ignores it, and `tidy_staging` sweeps it
/// once the process that renamed it is gone.
pub fn discarded_name(job_id: &str) -> String {
    format!("{job_id}.{}.gone", std::process::id())
}

/// Removes the staging folders of job creations a crash or a force quit cut off (app start), and the folders of
/// discards one cut off (`discarded_name`): only Klipolo's own, `<job id>.<pid>.new` or `.gone`, and only once that
/// process is gone. A live creation or discard is never touched, nor a folder of any other name.
pub fn tidy_staging(jobs_root: &Path) {
    let Ok(entries) = std::fs::read_dir(jobs_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name
            .strip_suffix(".new")
            .or_else(|| name.strip_suffix(".gone"))
        else {
            continue;
        };
        let Some((job, pid)) = stem
            .rsplit_once('.')
            .and_then(|(job, pid)| Some((job, pid.parse::<u32>().ok()?)))
        else {
            continue;
        };
        if valid_job_id(job)
            && entry.path().is_dir()
            && klipolo_platform::process::is_leftover(Some(pid), None)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The bytes the files in `dir` take, and those in its folders (a job's format folders hold its parts).
pub fn folder_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => folder_bytes(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Job folders live here, inside the exports folder (same volume: finished files are renamed into place).
pub const JOBS_DIR: &str = ".jobs";

/// Frames of `project`'s video at `fps` (a partial last frame counts): until the main track or its last picture layer
/// ends, whichever is later. Its sound is cut there too (`export::sound`).
pub fn frames_of(project: &Project, fps: u32) -> u64 {
    let d = i128::from(video_duration(&project.timeline).max(0));
    ((d * i128::from(fps) + 999_999) / 1_000_000) as u64
}

/// What the user reads when an export's files can't be read (the reason goes to the log).
pub const UNREADABLE: &str = "This export's files can't be read. Discard it and export again.";

impl JobOutput {
    /// Most of its finished frames came from the software encoder (SoftwareSafe): it keeps finishing with that
    /// encoder, so they still join. A format with none finished is false (hardware first).
    pub fn built_in_software(&self) -> bool {
        let done = |software: bool| -> u64 {
            self.segments
                .iter()
                .filter(|s| s.status == PartStatus::Done && s.software == software)
                .map(|s| s.frames)
                .sum()
        };
        done(true) > done(false)
    }

    /// Finished hardware segments that compatibility mode would redo: only where segments are still to render (a
    /// format that is complete joins as it is).
    pub fn parts_to_redo(&self) -> u32 {
        if self.pending().next().is_none() {
            return 0;
        }
        self.segments
            .iter()
            .filter(|s| s.status == PartStatus::Done && !s.software)
            .count() as u32
    }
}

impl ExportJob {
    /// Every format's frames (the unit of `Progress`).
    pub fn format_frames(&self) -> u64 {
        self.total_frames * self.outputs.len() as u64
    }

    /// Frames already in finished segments (or joined files), every format.
    pub fn frames_done(&self) -> u64 {
        self.outputs
            .iter()
            .map(|o| {
                if o.joined {
                    self.total_frames
                } else {
                    o.segments
                        .iter()
                        .filter(|s| s.status == PartStatus::Done)
                        .map(|s| s.frames)
                        .sum()
                }
            })
            .sum()
    }

    pub fn segments_total(&self) -> u32 {
        self.outputs.iter().map(|o| o.segments.len() as u32).sum()
    }

    pub fn segments_done(&self) -> u32 {
        self.outputs
            .iter()
            .map(|o| {
                if o.joined {
                    o.segments.len() as u32
                } else {
                    o.segments
                        .iter()
                        .filter(|s| s.status == PartStatus::Done)
                        .count() as u32
                }
            })
            .sum()
    }

    pub fn files(&self) -> Vec<PlannedFile> {
        self.outputs
            .iter()
            .map(|o| PlannedFile {
                format: o.format,
                width: o.width,
                height: o.height,
                name: o
                    .final_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: o.final_path.display().to_string(),
            })
            .collect()
    }

    /// Whether `saved` (the project as saved now) and `speech` (what was said in *its* files now) still make what this
    /// job renders, its caption pages and where its sound ducks included. For a project that isn't open, `speech` is
    /// that project's transcripts from the speech store, not the open project's: with captions on, or a sound layer
    /// ducking, any other speech makes the job look stale.
    pub fn is_current(&self, saved: &Project, speech: &Speech) -> bool {
        job_captions(saved, speech, &self.outputs, self.caption_file).is_ok_and(|f| {
            let hash = settings_hash(
                saved,
                &f.json,
                f.file.as_ref(),
                &ducking_speech(saved, speech),
                self.fps,
                self.quality,
                &self.outputs,
            );
            hash == self.settings_hash
        })
    }

    /// Whether `saved` (a project as saved now, one that isn't open) still makes what this job renders, judged with the
    /// words the job froze: what was said in a project's files is heard again only while it is open, so a closed
    /// project's job is changed exactly when the project's saved content is. A job whose frozen pages or caption file
    /// can't be read is never current.
    pub fn is_current_as_saved(&self, saved: &Project) -> bool {
        let Ok(captions) = std::fs::read(self.dir.join(super::job::CAPTIONS_FILE)) else {
            return false;
        };
        let file = match self.caption_file {
            None => None,
            Some(kind) => match std::fs::read_to_string(self.dir.join(frozen_name(kind))) {
                Ok(text) => Some((kind, text)),
                Err(_) => return false,
            },
        };
        let hash = settings_hash(
            saved,
            &captions,
            file.as_ref(),
            &self.speech_ranges,
            self.fps,
            self.quality,
            &self.outputs,
        );
        hash == self.settings_hash
    }

    /// For `export_jobs`: `running` when a worker of this app is on it, `stale` from `is_current`.
    pub fn info(&self, running: bool, stale: bool) -> ExportJobInfo {
        let state = match (self.state, running) {
            (_, true) => ExportJobState::Running,
            (JobState::Running, false) => ExportJobState::Interrupted,
            (JobState::Paused, false) => ExportJobState::Paused,
            (JobState::Failed, false) => ExportJobState::Failed,
        };
        ExportJobInfo {
            job_id: self.job_id.clone(),
            project_id: self.project_id.clone(),
            project_name: self.project_name.clone(),
            quality: self.quality,
            files: self.files(),
            state,
            message: self.message.clone(),
            frame: self.frames_done(),
            total_frames: self.format_frames(),
            segments_done: self.segments_done(),
            segments_total: self.segments_total(),
            stale,
            created_ms: self.created_ms,
            bytes: folder_bytes(&self.dir),
            compatibility: (self.state == JobState::Paused && self.offer_compatible).then(|| {
                ExportCompatibility {
                    parts_to_redo: self.outputs.iter().map(JobOutput::parts_to_redo).sum(),
                    chosen: self.chosen_compatible,
                }
            }),
        }
    }
}

/// Every job folder in `root`, newest first: readable ones, and the id and reason of the rest (they can only be
/// discarded).
pub fn list_jobs(root: &Path) -> Vec<Result<ExportJob, (String, String)>> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut jobs: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let id = e.file_name().into_string().ok()?;
            valid_job_id(&id).then(|| ExportJob::open(&e.path()).map_err(|m| (id, m)))
        })
        .collect();
    jobs.sort_by_key(|j| std::cmp::Reverse(j.as_ref().map_or(0, |j| j.created_ms)));
    jobs
}
