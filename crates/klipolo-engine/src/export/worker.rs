//! The child side of export: `Klipolo --export-worker <job folder>/job.json`. It owns its own runtime, GPU
//! device and VideoToolbox sessions, so a crash or a stuck encoder can't take the app down. It runs one job:
//! 1. check what earlier runs finished, then the media; set up the sound (decode and measure), the encoder probe and
//!    the GPU device, side by side (`prepare.rs`);
//! 2. render the sound track on its own thread, and the missing segments in decode passes (`pass.rs`);
//! 3. join each format's segments with the sound (`finalize.rs`).
//!
//! It stops cleanly on `pause` (stdin) and when its parent is gone (stdin or stdout closed), keeping every finished
//! segment; if VideoToolbox won't let go, a deadline ends the process anyway (`plumbing.rs`).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::job::ExportJob;
use super::passes::render_all;
pub(super) use super::plumbing::parent_gone;
use super::plumbing::{LOCK_WAIT, arm_deadline, listen, lock_job};
use super::prepare::{Prepared, prepare};
use super::protocol::{ErrorCode, Lines, WorkerLine};
use super::quality::probe_caps;
use super::shared::{Control, JobHandle, WorkerError};

/// While only the sound is left, the parent hears from the worker this often (its watchdog allows 120 s).
const SOUND_BEAT: Duration = Duration::from_secs(2);

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    Done,
    Paused,
    /// The parent is gone: nothing more to say.
    Gone,
}

pub fn main(args: &[String]) -> i32 {
    let _ = ffmpeg::init();
    match args.first().map(String::as_str) {
        Some("--version") => {
            let info = crate::ffmpeg_info();
            let caps = probe_caps();
            println!(
                "{}",
                serde_json::json!({ "ffmpeg": info.version, "license": info.license, "encoders": caps })
            );
            0
        }
        Some(path) => {
            arm_deadline();
            crate::qos::set_ceiling(crate::qos::Qos::UserInitiated);
            crate::qos::set_current(crate::qos::Qos::UserInitiated);
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("klipolo-export")
                .on_thread_start(|| crate::qos::set_current(crate::qos::Qos::UserInitiated))
                .build();
            match runtime {
                Ok(rt) => rt.block_on(run(Path::new(path))),
                Err(_) => 2,
            }
        }
        None => {
            eprintln!("usage: --export-worker <job.json>");
            2
        }
    }
}

async fn run(job_path: &Path) -> i32 {
    // The trial smoke's stalled reader or slow software decoder (never in a shipped build).
    #[cfg(feature = "trial")]
    crate::decode_wait::hooks::from_env();
    let mut out = Lines::stdout();
    let fail = |out: &mut Lines, message: String| {
        let _ = out.send(&WorkerLine::Error {
            code: ErrorCode::InvalidJob,
            message,
            retryable: false,
        });
        2
    };
    let dir = job_path.parent().unwrap_or(Path::new("."));
    // The lock first, then the manifest: a resume that waited for an old worker reads what that worker finished
    // meanwhile (a segment it closed, a format it joined), so finished work is never done again.
    let _lock = match lock_job(dir, LOCK_WAIT).await {
        Ok(lock) => lock,
        Err(message) => return fail(&mut out, message),
    };
    let job = match ExportJob::open(dir) {
        Ok(job) => job,
        Err(reason) => {
            eprintln!("export: the job can't be read: {reason}");
            return fail(&mut out, super::jobs::UNREADABLE.into());
        }
    };
    fonts_before_frame_zero(&job);
    let control = Arc::new(Control::default());
    listen(control.clone());
    let _awake = crate::activity::begin("Exporting video");
    match export(job, &mut out, control.clone()).await {
        Ok(Ended::Done) => 0,
        Ok(Ended::Paused) => {
            let _ = out.send(&WorkerLine::Paused);
            0
        }
        Ok(Ended::Gone) => 3,
        Err(_) if control.stopped() => 3,
        Err(e) => {
            let _ = out.send(&WorkerLine::Error {
                code: e.code,
                message: e.message,
                retryable: e.retryable,
            });
            2
        }
    }
}

/// The worker's fonts, before any font system: the one `configure` of this process with the fonts
/// folder the app recorded in the job, then the job's downloaded families (`render::fonts::register`), whose
/// Noto Sans are the CJK fallbacks, then the app's scan of this computer's fonts as the job recorded it
/// (`render::fonts::use_recorded_scan`). The compositor made after it builds its font system before frame 0
/// (`SharedFonts::new(true)`), from that record (no system scan), or by scanning when the job has none, so frame 0 has
/// the installed families, their forbidden-fallback list and every face the job registered.
pub fn fonts_before_frame_zero(job: &ExportJob) {
    if let Err(e) = crate::render::fonts::configure(font_dirs(job)) {
        eprintln!("export: {e}");
    }
    let downloads = job_downloads(job);
    if !downloads.is_empty() {
        crate::render::fonts::register(downloads);
    }
    crate::render::fonts::use_recorded_scan(&job.dir);
}

/// The downloaded families a job draws with, as the app recorded them in it: only their font files, each a face
/// file the worker's fontdb reads (memory-safe) when a text uses it.
fn job_downloads(job: &ExportJob) -> Vec<klipolo_fonts::index::Downloaded> {
    job.downloads.clone()
}

/// The fonts a job's worker draws with: the folder the app recorded in it, so the export draws the very
/// faces the preview does. A pure function of the job, so a test can check it.
pub fn font_dirs(job: &ExportJob) -> crate::render::fonts::FontDirs {
    crate::render::fonts::FontDirs {
        bundled: job.fonts_dir.clone(),
    }
}

/// A line to the parent; a closed stdout means the parent is gone (the run stops cleanly, no error).
pub(super) fn send<W: std::io::Write>(out: &mut Lines<W>, line: &WorkerLine, control: &Control) {
    if !control.stopped() && out.send(line).is_err() {
        parent_gone(control);
    }
}

/// Runs the job in the job folder. In-process tests call this with their own `Lines` and `Control`.
pub async fn export<W: std::io::Write>(
    job: ExportJob,
    out: &mut Lines<W>,
    control: Arc<Control>,
) -> Result<Ended, WorkerError> {
    let started = Instant::now();
    let handle = JobHandle::new(job);
    let Some(Prepared {
        ctx,
        settings,
        software,
        mut backend,
        mut sound,
    }) = prepare(&handle, out, &control).await?
    else {
        // Paused or stopped while it analysed the screens: every whole minute made is kept.
        return Ok(if control.stopped() {
            Ended::Gone
        } else {
            Ended::Paused
        });
    };
    eprintln!("export set-up {:.2} s", started.elapsed().as_secs_f64());

    let mut stopped = false;
    if let Some(backend) = backend.as_mut() {
        let rendered =
            render_all(&handle, &settings, &software, &ctx, backend, out, &control).await;
        stopped = match rendered {
            Ok(stopped) => stopped,
            Err(e) => {
                // Stops the sound thread (not `stop`: that means the parent is gone, and the error would be lost).
                control.pause();
                if let Some(sound) = sound.take() {
                    let _ = super::sound::wait(sound, &mut |_, _| {}, SOUND_BEAT).await;
                }
                return Err(e);
            }
        };
    }
    drop(backend);
    if let Some(sound) = sound.take() {
        if stopped {
            control.pause();
        }
        // Only the sound is left: say so every few seconds while it advances (the parent stops a silent worker).
        let mut beat = |done_s: f64, total_s: f64| {
            send(out, &WorkerLine::Sound { done_s, total_s }, &control);
        };
        let finished = super::sound::wait(sound, &mut beat, SOUND_BEAT).await?;
        stopped |= !finished;
    }
    if stopped || !control.running() {
        return Ok(if control.stopped() {
            Ended::Gone
        } else {
            Ended::Paused
        });
    }

    let outputs = handle.read(|j| j.outputs.len());
    for i in 0..outputs {
        if handle.read(|j| j.outputs[i].joined) {
            continue;
        }
        send(out, &WorkerLine::Finishing, &control);
        let (format, (temp_path, bytes)) = (
            handle.read(|j| j.outputs[i].format),
            super::finalize::join(&handle, i, &mut || {
                send(out, &WorkerLine::Finishing, &control)
            })?,
        );
        send(
            out,
            &WorkerLine::FormatDone {
                format,
                temp_path,
                bytes,
            },
            &control,
        );
    }
    let elapsed_s = started.elapsed().as_secs_f64();
    eprintln!("export done {elapsed_s:.2} s");
    send(out, &WorkerLine::Done { elapsed_s }, &control);
    Ok(Ended::Done)
}
