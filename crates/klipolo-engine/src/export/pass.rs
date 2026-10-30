//! One decode pass of an export. It opens the pass's encoders, all at once: one VideoToolbox session per format
//! for the whole run. Then it renders the segments still missing, with two stages side by side:
//! - pictures: plan, decode once, draw every format on the GPU and submit (`GpuBackend::render_formats`);
//! - delivery: once a frame's GPU work is done, each format that still needs that segment gets its IOSurface (bounded
//!   queues: the slowest stage sets the pace, memory stays flat).
//!
//! A pause finishes the current segment when less than half of it is left and drops it otherwise. When the parent is
//! gone, the pass stops at once. Either way every finished segment is kept.

use std::collections::BTreeMap;
use std::time::Instant;

use tokio::sync::mpsc;

use super::codecs::input_format;
use super::passes::Progress;
use super::prepare::preload_formats;
use super::protocol::{ErrorCode, Lines, WorkerLine};
use super::quality::EncodeSettings;
use super::shared::{Control, JobHandle, WorkerError, internal};
use super::worker::send;
use crate::plan::{FramePlan, PlanContext, frame_plan, frame_start_us};
use crate::render::GpuBackend;
use crate::render::export::{ExportFailure, FormatTarget, FormatTargets};
use crate::render::yuv::{PlaneLayout, Yuv};

mod delivery;
mod encoders;
use delivery::{Drawn, Heartbeat, Timing, deliver};
use encoders::open_encoders;

pub enum Pass {
    /// Not every session of this pass opened: export its outputs one at a time.
    Sequential,
    Failed(WorkerError),
}

impl From<WorkerError> for Pass {
    fn from(e: WorkerError) -> Self {
        Pass::Failed(e)
    }
}

fn failure(f: ExportFailure) -> WorkerError {
    match f {
        ExportFailure::Decode(message) => WorkerError {
            code: ErrorCode::Decode,
            message,
            retryable: true,
        },
        ExportFailure::Content(message) => WorkerError {
            code: ErrorCode::MissingMedia,
            message,
            retryable: false,
        },
        ExportFailure::Frame(coded) => WorkerError {
            code: ErrorCode::Frame,
            message: serde_json::to_string(&coded).unwrap_or_default(),
            retryable: false,
        },
        ExportFailure::Gpu(message) => gpu(message),
        // Not retried: the job pauses, naming the file, until its disk is back.
        ExportFailure::NotAnswering(path) => WorkerError {
            code: ErrorCode::NotAnswering,
            message: crate::render::pool::not_answering(&path),
            retryable: false,
        },
    }
}

fn gpu(message: String) -> WorkerError {
    WorkerError {
        code: ErrorCode::Gpu,
        message,
        retryable: true,
    }
}

/// One segment the pass renders, and which of its formats still need it (bit k: format k).
struct Span {
    start: u64,
    frames: u64,
    wanted: u32,
}

/// Renders the pass's missing segments. True when it stopped early (pause, or the parent is gone).
#[allow(clippy::too_many_arguments)]
pub async fn run_pass<W: std::io::Write>(
    job: &JobHandle,
    pass: &[usize],
    settings: &[EncodeSettings],
    software: &[bool],
    ctx: &PlanContext,
    backend: &mut GpuBackend,
    out: &mut Lines<W>,
    progress: &mut Progress,
    control: &Control,
) -> Result<bool, Pass> {
    let mut encoders = open_encoders(job, pass, settings, software).await?;
    let (fps, formats) = job.read(|j| {
        let formats: Vec<_> = pass
            .iter()
            .map(|&i| (j.outputs[i].format, j.outputs[i].width, j.outputs[i].height))
            .collect();
        (j.fps, formats)
    });
    let mut spans: BTreeMap<u32, Span> = BTreeMap::new();
    job.read(|j| {
        for (k, &i) in pass.iter().enumerate() {
            for s in j.outputs[i].pending() {
                let span = spans.entry(s.index).or_insert_with(|| Span {
                    start: s.start_frame,
                    frames: s.frames,
                    wanted: 0,
                });
                span.wanted |= 1 << k;
            }
        }
    });
    let mut list: Vec<FormatTarget> = Vec::with_capacity(pass.len());
    for &i in pass {
        let o = formats[list.len()];
        let yuv = Yuv::from_pixel(input_format(settings[i].codec)).unwrap_or(Yuv::P010);
        list.push(
            backend
                .format_target((o.1, o.2), yuv, !software[i])
                .map_err(gpu)?,
        );
    }
    let mut targets = FormatTargets::new(list).allowing_stand_ins(job.read(|j| j.allow_stand_ins));
    preload_formats(ctx, &mut targets);
    let layouts: Vec<PlaneLayout> = targets.iter().map(FormatTarget::layout).collect();
    let (mut timing, started) = (Timing::default(), Instant::now());

    let (tx, mut rx) = mpsc::channel::<Drawn>(1);
    let beat = backend.decode_beat();
    let (mut draw, mut drawn_frames, mut stopped) = (0f64, 0u64, false);
    let produce = async {
        let mut plans: Vec<FramePlan> = Vec::with_capacity(pass.len());
        // Where a pause or a gone parent stops the pass (a frame index).
        let mut stop_at: Option<u64> = None;
        'spans: for span in spans.values() {
            for n in span.start..span.start + span.frames {
                if stop_at.is_none() && !control.running() {
                    let end = span.start + span.frames;
                    let finish = !control.stopped() && (end - n) * 2 < span.frames;
                    stop_at = Some(if finish { end } else { n });
                }
                if stop_at.is_some_and(|at| n >= at) {
                    stopped = true;
                    break 'spans;
                }
                let at = Instant::now();
                let t = frame_start_us(n as i64, fps);
                plans.clear();
                plans.extend(formats.iter().map(|f| frame_plan(ctx, t, f.0)));
                let mut pixels = Vec::with_capacity(pass.len());
                let done = backend
                    .render_formats(&plans, &mut targets, &mut pixels)
                    .await
                    .map_err(failure)?;
                // A motion graphic drawn reduced: the finished export says so.
                for (code, at) in targets.take_notes() {
                    job.note(&code, at)?;
                }
                draw += at.elapsed().as_secs_f64();
                drawn_frames += 1;
                let drawn = Drawn {
                    index: n,
                    done,
                    pixels,
                    wanted: span.wanted,
                };
                if tx.send(drawn).await.is_err() {
                    // Delivery stopped; its error is the pass's.
                    break 'spans;
                }
            }
            if stop_at.is_some_and(|at| at <= span.start + span.frames) {
                stopped = true;
                break;
            }
        }
        drop(tx);
        Ok::<(), WorkerError>(())
    };
    let consume = async {
        let mut heartbeat = Heartbeat::new(beat);
        loop {
            let drawn = tokio::select! {
                drawn = rx.recv() => drawn,
                waiting = heartbeat.due() => {
                    if waiting {
                        send(out, &WorkerLine::WaitingForDecoder, control);
                    }
                    continue;
                }
            };
            let Some(drawn) = drawn else { break };
            match deliver(drawn, &layouts, &mut encoders, &mut timing).await {
                Ok(delivered) => progress.tick(out, delivered, control),
                Err(e) => {
                    // `rx` outlives this stage: close it, or the drawing stage waits forever to send its next frame.
                    rx.close();
                    return Err(e);
                }
            }
        }
        Ok::<(), WorkerError>(())
    };
    let (produced, consumed) = tokio::join!(produce, consume);
    consumed?;
    produced?;

    let (frames, wall) = (drawn_frames.max(1) as f64, started.elapsed().as_secs_f64());
    let ms = |s: f64| s * 1000.0 / frames;
    let peaks: Vec<usize> = targets.iter().map(FormatTarget::surface_peak).collect();
    eprintln!(
        "export pass {pass:?}: {drawn_frames} frames, {:.1} fps; ms/frame draw {:.1}, GPU wait {:.1}, encoder queue {:.1}; IOSurfaces {peaks:?}",
        frames / wall.max(1e-3),
        ms(draw),
        ms(timing.gpu),
        ms(timing.queue)
    );
    let finishing: Vec<_> = encoders
        .into_iter()
        .map(|e| tokio::task::spawn_blocking(|| e.finish()))
        .collect();
    for finished in finishing {
        finished.await.map_err(internal)??;
    }
    Ok(stopped)
}
