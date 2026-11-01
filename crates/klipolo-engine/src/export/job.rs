//! The export job: a folder next to the finished files, `<exports>/.jobs/<job id>/`, so a failed, crashed,
//! quit or paused export continues where it stopped. It holds:
//! - `job.json`, the manifest: the settings and their hash, each format's segments (about 30 s each) with their
//!   state, the sound and the job's state. Only one process writes it at a time (the app before and after a run, the
//!   worker while it runs), always atomically (`write_atomic`);
//! - `project.json`, the project as saved when the export started (written once);
//! - `captions.json`, the caption pages of every file's format, frozen with it (written once; part of the settings
//!   hash, so a job resumes only with the pages it started with). Where someone speaks, which a ducking sound layer
//!   goes down under, is frozen in the manifest the same way (`speech_ranges`);
//! - `contrib/`, the contributions it draws besides the built-ins and their verdicts (`export::contrib`);
//! - `captions.srt` or `captions.vtt`, the caption file a landscape export saves beside its video, made from those pages
//!   (`caption_file`; written once, part of the settings hash too);
//! - `<format>/seg-00042.<ext>`, the rendered segments; `audio.<ext>`, the sound; `<format>.<ext>`, a joined file
//!   until it is renamed into place.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use klipolo_model::captions::Captions;
use klipolo_model::captions::sidecar::CaptionFile;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::RangeUs;
use serde::{Deserialize, Serialize};

use super::caption_file::{Frozen, job_captions};
use super::jobs::{format_dir, frames_of, jobs_root, settings_hash, staging_name};
use super::quality::AudioCodec;
use super::runner::Batch;
use super::shared::{is_disk_full, write_atomic};
use crate::api::ExportQuality;
use crate::audio::plan::ducking_speech;

mod formats;

pub use formats::{JobOutput, Segment};

pub const JOB_VERSION: u32 = 2;
pub const AUDIO_BITRATE: u32 = 192_000;
/// Exports run at the project's frame rate, at most this.
pub const MAX_FPS: u32 = 60;
/// Segment length: a crash or pause costs at most this much rendering. A multiple of the 2 s GOP.
pub const SEGMENT_SECONDS: u32 = 30;
pub const JOB_FILE: &str = "job.json";
pub(crate) const PROJECT_FILE: &str = "project.json";
pub const CAPTIONS_FILE: &str = "captions.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportMode {
    Hardware,
    /// FFmpeg software decode, CPU NV12 readback and VideoToolbox's software encoder (`require_sw=1`), one format at
    /// a time. Never libx264.
    SoftwareSafe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PartStatus {
    Pending,
    /// Written, fsynced and verified (frame count and duration probed, size and checksum recorded).
    Done,
    /// Its file didn't verify when the job resumed: rendered again.
    Failed,
}

/// The job as a whole, as the app last left it (a worker's run is `Running`; a crash leaves it so).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Running,
    Paused,
    Failed,
}

/// What the analysis pass measured over the whole mix (integrated loudness) and the gain it chose: kept, so the
/// sound never differs between runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Levels {
    pub lufs: Option<f64>,
    pub gain_db: f64,
}

/// The whole sound track, rendered in one pass (cheap next to the pictures) and shared by every format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub codec: AudioCodec,
    pub file: String,
    pub status: PartStatus,
    pub bytes: u64,
    pub checksum: u32,
    /// The limiter ceiling it was rendered with, and the true peak measured on the encoded file.
    pub ceiling_dbtp: f32,
    pub true_peak_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportJob {
    pub version: u32,
    pub job_id: String,
    pub created_ms: u64,
    /// Project content, formats, sizes, quality and encoder settings: a job resumes only while the saved project
    /// still hashes the same.
    pub settings_hash: String,
    pub project_id: String,
    pub project_name: String,
    pub fps: u32,
    /// Frames of the main track at `fps` (a partial last frame counts).
    pub total_frames: u64,
    pub segment_frames: u64,
    pub quality: ExportQuality,
    pub mode: ExportMode,
    pub audio_bitrate: u32,
    /// The preview's PCM cache, so audio the editor already decoded isn't decoded again (None: a temp folder).
    pub pcm_dir: Option<PathBuf>,
    /// The editor's screen activity folder (`<media cache>/activity`): what the editor analysed is read, not
    /// analysed again, and what this export analyses, or finds it can't, the editor plans with too. None: the export
    /// plans without the screens' activity.
    #[serde(default)]
    pub activity_dir: Option<PathBuf>,
    pub state: JobState,
    /// Why it stopped: English, or a code (a frame it couldn't draw whole).
    pub message: Option<crate::api::Said>,
    /// It paused because the hardware path kept failing: it may finish in compatibility mode (`ResumeMode`).
    pub offer_compatible: bool,
    /// The user chose to finish it in compatibility mode: every later run (a plain Resume too) stays in it, and a
    /// pause keeps the offer. A new job (Start over) starts without it.
    pub chosen_compatible: bool,
    /// The person chose to export without the cursors that can't be read (`ErrorCode::CursorMissing`): those takes
    /// draw no cursor, and each says so (`exports.note.cursorDropped`).
    #[serde(default)]
    pub allow_missing_cursor: bool,
    /// The screen the export last asked about, because it can't be read now (`ErrorCode::ActivityUnreadable`): the
    /// person's "Export without it" is this screen's.
    #[serde(default)]
    pub activity_asked: Option<String>,
    /// The screens the person chose to export without, each on its own question: each is tried once, then planned with
    /// the minutes already made, never asked about again and never marked.
    #[serde(default)]
    pub activity_without: Vec<String>,
    pub levels: Option<Levels>,
    /// Where someone speaks on the timeline, which its ducking sound layers go down under (`ducking_speech` of the
    /// batch's project and speech), frozen when the job is made: a resumed job's sound is the one it started with, and
    /// the worker needs no transcripts. Part of the settings hash; empty (and left out) when nothing ducks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speech_ranges: Vec<RangeUs>,
    pub audio: AudioTrack,
    pub outputs: Vec<JobOutput>,
    /// The text language its CJK follows (`PlanContext::text_language`), frozen when the job is made.
    #[serde(default)]
    pub text_language: String,
    /// A text in a missing font exports in its stand-in (the export's "Export with Inter instead"); never CJK as boxes.
    #[serde(default)]
    pub allow_stand_ins: bool,
    /// The bundled fonts folder of the app that runs the job (`render::fonts::dirs`): recorded at creation
    /// and again by each run (an app moved or updated between a pause and a resume), so the worker configures exactly
    /// the folder the preview draws from (`worker::font_dirs`). None: a broken install, every text a stand-in.
    #[serde(default)]
    pub fonts_dir: Option<PathBuf>,
    /// What it drew reduced and says when it finishes (a motion graphic past its caps: `JobHandle::note`).
    #[serde(default)]
    pub notes: Vec<crate::api::Said>,
    /// The downloaded Google families the app had registered (`render::fonts::downloads`), the CJK
    /// fallbacks' Noto Sans among them: recorded beside `fonts_dir` at creation and by each run, and registered by the
    /// worker before frame 0 (`worker::fonts_before_frame_zero`).
    #[serde(default)]
    pub downloads: Vec<klipolo_fonts::index::Downloaded>,
    /// The caption file it saves beside each landscape video (`caption_file::frozen_name` in its folder), when the
    /// person asked for one and it has words to show.
    #[serde(default)]
    pub caption_file: Option<CaptionFile>,
    /// A caption file was asked for with a landscape file, but there were no words to put in it: said when the export
    /// finishes (`ExportEvent::CaptionFileEmpty`).
    #[serde(default)]
    pub captions_empty: bool,
    /// `project.json`.
    #[serde(skip, default = "blank_project")]
    pub project: Project,
    /// `captions.json`: the pages of every output's format (none when captions are off).
    #[serde(skip)]
    pub captions: Arc<Captions>,
    #[serde(skip)]
    pub dir: PathBuf,
}

fn blank_project() -> Project {
    Project::new("")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl ExportJob {
    /// Plans a job for `batch` in `<files' folder>/.jobs/<new id>/` (segments of `segment_s` seconds) and writes it.
    /// It is written in `<id>.<pid>.new/` (which the job list ignores; `tidy_staging` removes it if this process dies) and renamed into place once complete, so a full disk
    /// or a crash part-way never leaves a job that can't be read.
    pub fn create(batch: &Batch, segment_s: u32) -> Result<Self, String> {
        let fps = batch.fps.clamp(1, MAX_FPS);
        let exports = batch
            .files
            .first()
            .and_then(|f| Path::new(&f.path).parent())
            .ok_or("The export has no files.")?;
        let job_id = uuid::Uuid::new_v4().to_string();
        let dir = jobs_root(exports).join(&job_id);
        let staging = jobs_root(exports).join(staging_name(&job_id));
        let total_frames = frames_of(&batch.project, fps);
        let segment_frames = u64::from(segment_s.max(1)) * u64::from(fps);
        let outputs = formats::plan(batch, fps, total_frames, segment_frames);
        let audio_codec = outputs
            .first()
            .map_or(AudioCodec::Aac192, |o| o.settings.audio);
        // The pages of exactly these files' formats, frozen with the job (a format the project doesn't list included),
        // and the caption file made from them.
        let frozen = job_captions(&batch.project, &batch.speech, &outputs, batch.caption_file)
            .map_err(|e| {
                tracing::warn!(%e, "export: the captions couldn't be written");
                "The export didn't start: its files couldn't be written. Try again.".to_string()
            })?;
        let Frozen {
            captions,
            json: captions_json,
            file: caption_file,
        } = frozen;
        let speech_ranges = ducking_speech(&batch.project, &batch.speech).to_vec();
        let outputs_have_landscape = outputs.iter().any(|o| o.format == FormatKind::Landscape);
        let mut job = Self {
            version: JOB_VERSION,
            settings_hash: settings_hash(
                &batch.project,
                &captions_json,
                caption_file.as_ref(),
                &speech_ranges,
                fps,
                batch.quality,
                &outputs,
            ),
            job_id,
            created_ms: now_ms(),
            project_id: batch.project.id.clone(),
            project_name: batch.project.name.clone(),
            fps,
            total_frames,
            segment_frames,
            quality: batch.quality,
            mode: ExportMode::Hardware,
            audio_bitrate: AUDIO_BITRATE,
            pcm_dir: batch.pcm_dir.clone(),
            activity_dir: batch.activity_dir.clone(),
            state: JobState::Running,
            message: None,
            offer_compatible: false,
            chosen_compatible: false,
            allow_missing_cursor: batch.allow_missing_cursor,
            activity_asked: None,
            activity_without: Vec::new(),
            levels: None,
            speech_ranges,
            audio: AudioTrack {
                codec: audio_codec,
                file: match audio_codec {
                    AudioCodec::Aac192 => "audio.m4a".into(),
                    AudioCodec::Pcm24 => "audio.mov".into(),
                },
                status: PartStatus::Pending,
                bytes: 0,
                checksum: 0,
                ceiling_dbtp: super::audio::CEILING_DBTP,
                true_peak_db: None,
            },
            outputs,
            text_language: batch.text_language.to_string(),
            allow_stand_ins: batch.allow_stand_ins,
            fonts_dir: crate::render::fonts::dirs().bundled,
            notes: Vec::new(),
            downloads: crate::render::fonts::downloads(),
            caption_file: caption_file.as_ref().map(|(kind, _)| *kind),
            captions_empty: batch.caption_file.is_some()
                && caption_file.is_none()
                && outputs_have_landscape,
            project: batch.project.clone(),
            captions: Arc::new(captions),
            dir: staging.clone(),
        };
        let fail = |e: std::io::Error| {
            if is_disk_full(&e) {
                return super::runner::DISK_FULL.to_string();
            }
            tracing::warn!(%e, "export: the job folder couldn't be written");
            "The export didn't start: its files couldn't be written. Try again.".to_string()
        };
        let written = (|| {
            for o in &job.outputs {
                std::fs::create_dir_all(job.dir.join(format_dir(o.format)))?;
            }
            let project = serde_json::to_vec(&job.project).map_err(std::io::Error::other)?;
            write_atomic(&job.dir.join(PROJECT_FILE), &project)?;
            write_atomic(&job.dir.join(CAPTIONS_FILE), &captions_json)?;
            // The contributions it draws, from memory, with their verdicts.
            super::contrib::write_job_contribs(&job.dir, &batch.contribs)?;
            if let Some((kind, text)) = &caption_file {
                write_atomic(
                    &job.dir.join(super::caption_file::frozen_name(*kind)),
                    text.as_bytes(),
                )?;
            }
            job.save()?;
            std::fs::rename(&staging, &dir)
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(fail(e));
        }
        job.dir = dir;
        Ok(job)
    }

    /// Reads the job in `dir` (its manifest, project and caption pages).
    ///
    /// Fails closed: a job whose project has captions on reads only with pages for every one of its files' formats, so
    /// an export never finishes silently without them. Without `captions.json`, captions must be off.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let text = std::fs::read(dir.join(JOB_FILE))
            .map_err(|e| format!("This export's progress couldn't be read: {e}"))?;
        let mut job: Self = serde_json::from_slice(&text)
            .map_err(|e| format!("This export's progress is damaged: {e}"))?;
        if job.version != JOB_VERSION {
            return Err("This export came from another version of Klipolo.".into());
        }
        if job.fps == 0 || job.outputs.is_empty() || job.segment_frames == 0 {
            return Err("This export's progress is damaged: no frame rate or no files.".into());
        }
        let project = std::fs::read(dir.join(PROJECT_FILE))
            .map_err(|e| format!("This export's project couldn't be read: {e}"))?;
        job.project = serde_json::from_slice(&project)
            .map_err(|e| format!("This export's project is damaged: {e}"))?;
        job.captions = match std::fs::read(dir.join(CAPTIONS_FILE)) {
            Ok(bytes) => Arc::new(
                serde_json::from_slice(&bytes)
                    .map_err(|e| format!("This export's captions are damaged: {e}"))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Arc::default(),
            Err(e) => return Err(format!("This export's captions couldn't be read: {e}")),
        };
        if job.project.captions.enabled
            && let Some(o) = job
                .outputs
                .iter()
                .find(|o| !job.captions.pages.contains_key(&o.format))
        {
            return Err(format!(
                "This export's captions are missing for its {} file.",
                format_dir(o.format)
            ));
        }
        job.dir = dir.to_path_buf();
        Ok(job)
    }

    /// Writes the manifest (atomically).
    pub fn save(&self) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        write_atomic(&self.dir.join(JOB_FILE), &bytes)
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    /// The manifest the worker is started with.
    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join(JOB_FILE)
    }
}
