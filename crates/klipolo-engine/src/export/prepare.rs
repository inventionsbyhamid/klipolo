//! Everything an export run does before its first frame: what earlier runs finished is checked; the media is
//! checked; a run that draws frames makes the screens' activity its plans read (`activity`: it may pause
//! there); the cursor takes are read and smoothed with it, their follow paths and the camera plans made, all off the
//! runtime's threads, so export plans as the preview does; a cursor it can't draw stops a run that draws frames
//! before anything is written unless the person allowed it (`cursors`); then, side by side, the sound (decoded, and
//! measured on the first run), the encoder probe and the GPU device; each format's encoder settings; and the sound
//! thread starts. `worker::export` then renders the pictures with what this returns; each decode pass puts the cursor
//! shapes in its formats' atlases before its first frame (`preload_formats`).

use std::sync::Arc;

use klipolo_model::time::Us;

use super::audio::ExportAudio;
use super::check::{check_files, check_parts, media_files};
use super::job::{ExportJob, ExportMode, PartStatus};
use super::plumbing::alive;
use super::protocol::{ErrorCode, Lines, WorkerLine};
use super::quality::{EncodeSettings, VideoCodec, encode_settings, probe_caps_for, software_safe};
use super::runner::NO_H264;
use super::shared::{Control, JobHandle, WorkerError, internal};
use super::sound::Sound;
use super::worker::send;
use crate::media::activity::ActivityStore;
use crate::media::cursor::wanted;
use crate::plan::{PlanContext, frame_start_us};
use crate::platform::activity::platform_opener;
use crate::render::GpuBackend;

mod activity;
mod cursors;

pub use activity::{
    Analysed, Analysis, RETRY_WAITS, Retry, analyse, analyse_screens, screen_files, unreadable,
};
pub use cursors::{
    Loaded, Missing, cursor_shapes, load_cursors, load_cursors_into, load_cursors_with,
    preload_formats, prepare_cursors,
};

/// What the run renders with.
pub(super) struct Prepared {
    pub(super) ctx: PlanContext,
    /// Each format's encoder settings, and whether it uses the software encoder.
    pub(super) settings: Vec<EncodeSettings>,
    pub(super) software: Vec<bool>,
    /// The GPU device, when some format has segments left to render.
    pub(super) backend: Option<GpuBackend>,
    /// The sound thread, when the sound isn't finished and some format isn't joined.
    pub(super) sound: Option<Sound>,
}

/// Timeline ranges, `[from, to)` each.
type Ranges = Vec<(Us, Us)>;

/// The timeline ranges of the job's parts, every format's: those still to render, and those finished (a joined
/// format's all are). What the cursor question covers. O(parts).
fn parts(j: &ExportJob) -> (Ranges, Ranges) {
    let at = |frame: u64| frame_start_us(frame as i64, j.fps);
    let (mut left, mut done) = (Vec::new(), Vec::new());
    for o in &j.outputs {
        for s in &o.segments {
            let range = (at(s.start_frame), at(s.start_frame + s.frames));
            match o.joined || s.status == PartStatus::Done {
                true => done.push(range),
                false => left.push(range),
            }
        }
    }
    (left, done)
}

/// Sets the run up (see the module notes), telling the parent it is alive meanwhile. None when it was paused or
/// stopped while it analysed the screens.
pub(super) async fn prepare<W: std::io::Write>(
    handle: &JobHandle,
    out: &mut Lines<W>,
    control: &Arc<Control>,
) -> Result<Option<Prepared>, WorkerError> {
    let compatible = handle.read(|j| j.mode == ExportMode::SoftwareSafe);
    let checked = {
        let handle = handle.clone();
        tokio::task::spawn_blocking(move || check_parts(&handle))
    };
    let (project, allow_missing, activity_dir, without_activity) = handle.read(|j| {
        let project = Arc::new(j.project.clone());
        let without = j.activity_without.clone();
        (
            project,
            j.allow_missing_cursor,
            j.activity_dir.clone(),
            without,
        )
    });
    // What it draws besides the built-ins: exactly what the preview's snapshot ran when the job was made, assembled
    // 11), assembled off the async threads.
    let registry = {
        let dir = handle.read(|j| j.dir.clone());
        tokio::task::spawn_blocking(move || super::contrib::job_registry(&dir))
            .await
            .map_err(internal)?
    };
    alive(checked, out, control).await??;
    // Each format with the encoder its finished segments came from (so they still join), hardware for a new one;
    // compatibility mode puts every format on the software encoder.
    let software: Vec<bool> = handle.read(|j| {
        j.outputs
            .iter()
            .map(|o| compatible || o.built_in_software())
            .collect()
    });
    let (render, sound, line, (left, done)) = handle.read(|j| {
        let render = j.outputs.iter().any(|o| o.pending().next().is_some());
        let sound = j.audio.status != PartStatus::Done && j.outputs.iter().any(|o| !o.joined);
        let line = WorkerLine::Started {
            job_id: j.job_id.clone(),
            total_frames: j.format_frames(),
            frames_done: j.frames_done(),
            formats: j.outputs.iter().map(|o| o.format).collect(),
            mode: j.mode,
        };
        (render, sound, line, parts(j))
    });
    send(out, &line, control);
    if render || sound {
        // Off the thread, telling the parent it is alive: a disk waking up may take a while to answer.
        let files = tokio::spawn(check_files(media_files(&project, &registry)));
        alive(files, out, control).await??;
    }
    // The screens' activity its plans read, made before the first frame by a run that draws frames, in the
    // editor's folder; then the takes with it (their facts and follow paths) and the camera plans, off the thread, so
    // export zooms and crops as the preview does.
    let wants = wanted(&project);
    let activity = match render {
        true => {
            let store = activity_dir.map(|root| ActivityStore::detached(root, platform_opener()));
            let retry = Retry {
                waits: &RETRY_WAITS,
                without: without_activity,
            };
            let files = screen_files(&project, &wants);
            // The job keeps the screen it asks about, so the person's answer is that screen's.
            let asked = |path: &str| {
                let kept = handle.update(|j| j.activity_asked = Some(path.to_owned()));
                if let Err(error) = kept {
                    tracing::warn!(?error, "export: the screen asked about couldn't be noted");
                }
            };
            match analyse_screens(store, files, out, control, retry, &asked).await? {
                Some(analysed) => Some(analysed.source()),
                None => return Ok(None),
            }
        }
        false => None,
    };
    let (loaded, ctx) = {
        let (project, registry) = (project.clone(), registry.clone());
        // The cues frozen with the job: a resumed job draws exactly the captions it started with.
        let (captions, text_language): (_, Arc<str>) =
            handle.read(|j| (j.captions.clone(), j.text_language.as_str().into()));
        let planned = tokio::task::spawn_blocking(move || {
            let loaded = load_cursors_with(&project, &wants, activity);
            let ctx = PlanContext::with_cursors(project, registry, loaded.cursors.clone())
                .with_captions(captions)
                .with_text_language(text_language);
            (loaded, ctx)
        });
        alive(planned, out, control).await?
    };
    // A separate take whose cursor isn't held stops a run that draws it here, after its footage is checked and before
    // anything is written, unless the person allowed it; then each is said. Only the parts left
    // count: a run that only joins or renders the sound draws nothing, so it neither asks nor says it
    // never draw that take.
    if render {
        if let Some(question) = loaded.question_in(&left, &done).filter(|_| !allow_missing) {
            return Err(question);
        }
        for message in loaded.notes_in(&left) {
            send(out, &WorkerLine::Note { message }, control);
        }
    }

    // Set-up side by side: the sound (decode, and measure on the first run), the encoder probe and the GPU device.
    let audio = sound.then(|| {
        let (project, registry) = ((*project).clone(), ctx.registry.clone());
        let (quality, pcm_dir, levels, speech) = handle.read(|j| {
            let speech: Arc<[_]> = j.speech_ranges.as_slice().into();
            (j.quality, j.pcm_dir.clone(), j.levels.clone(), speech)
        });
        tokio::task::spawn_blocking(move || {
            ExportAudio::prepare(
                &project,
                &registry,
                quality,
                pcm_dir.as_deref(),
                levels,
                speech,
            )
        })
    });
    let qualities: Vec<_> = handle.read(|j| j.outputs.iter().map(|o| o.quality).collect());
    let caps = render.then(|| tokio::task::spawn_blocking(move || probe_caps_for(&qualities)));
    // FFmpeg decoding as soon as one format with work left is in compatibility mode.
    let soft_decode = handle.read(|j| {
        j.outputs
            .iter()
            .zip(&software)
            .any(|(o, &soft)| soft && o.pending().next().is_some())
    });
    let backend = match render {
        true => Some(
            GpuBackend::for_export_with(soft_decode)
                .await
                .map_err(|message| WorkerError {
                    code: ErrorCode::Gpu,
                    message,
                    retryable: true,
                })?,
        ),
        false => None,
    };
    let caps = match caps {
        Some(caps) => caps.await.map_err(internal)?,
        None => Default::default(),
    };
    let settings: Vec<EncodeSettings> = handle.read(|j| {
        j.outputs
            .iter()
            .zip(&software)
            .map(|(o, &soft)| {
                let s = encode_settings(o.quality, (o.width, o.height), j.fps, &caps);
                if soft { software_safe(s) } else { s }
            })
            .collect()
    });
    // No H.264 encoder here (Windows and Linux until W and L): say so rather than retrying.
    let needs_h264 = handle.read(|j| {
        j.outputs
            .iter()
            .zip(&settings)
            .any(|(o, s)| s.codec == VideoCodec::H264High && o.pending().next().is_some())
    });
    if render && needs_h264 && !caps.h264 {
        return Err(WorkerError {
            code: ErrorCode::EncoderUnavailable,
            message: NO_H264.into(),
            retryable: false,
        });
    }
    let mut sound = None;
    if let Some(audio) = audio {
        let (audio, levels) = alive(audio, out, control).await??;
        eprintln!("export audio: {levels:?}");
        handle.update(|j| j.levels = Some(levels))?;
        sound = Some(super::sound::spawn(audio, handle.clone(), control.clone())?);
    }
    Ok(Some(Prepared {
        ctx,
        settings,
        software,
        backend,
        sound,
    }))
}
