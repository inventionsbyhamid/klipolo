//! The decode passes of a run: outputs with segments to render, grouped so each pass fits the encoder
//! sessions (`pass.rs` runs one), and the progress lines they send.

use std::collections::VecDeque;
use std::time::Instant;

use super::check::check_media;
use super::job::JobOutput;
use super::pass::{Pass, run_pass};
use super::protocol::{ErrorCode, Lines, WorkerLine};
use super::quality::{EncodeSettings, ONE_PASS_SESSIONS, session_weight};
use super::shared::{Control, JobHandle, WorkerError, internal};
use super::worker::send;
use crate::plan::PlanContext;
use crate::render::GpuBackend;

/// Format-frames done (earlier runs' included) and the progress lines (at most 10 a second).
pub(super) struct Progress {
    done: u64,
    total: u64,
    rendered: u64,
    started: Instant,
    sent: Instant,
    job: JobHandle,
    segments_total: u32,
}

impl Progress {
    pub(super) fn tick<W: std::io::Write>(
        &mut self,
        out: &mut Lines<W>,
        n: u64,
        control: &Control,
    ) {
        // Finished segments a new encoder's first verified one set back (`JobHandle::segment_done`) come off now, so
        // the bar never climbs to the end and drops back at the next round.
        self.done = self.done.saturating_sub(self.job.take_set_back()) + n;
        self.rendered += n;
        if self.sent.elapsed().as_millis() >= 100 || self.done >= self.total {
            self.sent = Instant::now();
            let fps = self.rendered as f64 / self.started.elapsed().as_secs_f64().max(1e-3);
            let line = WorkerLine::Progress {
                frame: self.done.min(self.total),
                total_frames: self.total,
                fps,
                segments_done: self.job.segments_done(),
                segments_total: self.segments_total,
            };
            send(out, &line, control);
        }
    }
}

/// Outputs with segments to render, grouped into decode passes: hardware outputs as many as fit `ONE_PASS_SESSIONS`,
/// software ones (`software[i]`) one at a time.
fn passes(
    outputs: &[JobOutput],
    settings: &[EncodeSettings],
    software: &[bool],
) -> VecDeque<Vec<usize>> {
    let mut passes: VecDeque<Vec<usize>> = VecDeque::new();
    let (mut weight, mut open) = (0, false);
    for (i, o) in outputs.iter().enumerate() {
        if o.pending().next().is_none() {
            continue;
        }
        if software[i] {
            passes.push_back(vec![i]);
            open = false;
            continue;
        }
        let w = session_weight(&settings[i], (o.width, o.height));
        match passes.back_mut() {
            Some(pass) if open && weight + w <= ONE_PASS_SESSIONS => pass.push(i),
            _ => {
                passes.push_back(vec![i]);
                (weight, open) = (0, true);
            }
        }
        weight += w;
    }
    passes
}

/// Renders every missing segment, pass after pass, until none is left. A format's first verified segment from a new
/// encoder may mark its older segments for rendering again (`JobHandle::segment_done`): the next round renders
/// them. True when a pause (or a gone parent) stopped it.
pub(super) async fn render_all<W: std::io::Write>(
    handle: &JobHandle,
    settings: &[EncodeSettings],
    software: &[bool],
    ctx: &PlanContext,
    backend: &mut GpuBackend,
    out: &mut Lines<W>,
    control: &Control,
) -> Result<bool, WorkerError> {
    let (total, segments_total, outputs) =
        handle.read(|j| (j.format_frames(), j.segments_total(), j.outputs.len()));
    let mut progress = Progress {
        done: 0,
        total,
        rendered: 0,
        started: Instant::now(),
        sent: Instant::now(),
        job: handle.clone(),
        segments_total,
    };
    // Each format changes encoder at most once per run, so a round more than formats means something is wrong.
    for _ in 0..=outputs + 1 {
        let mut queue = handle.read(|j| passes(&j.outputs, settings, software));
        if queue.is_empty() {
            return Ok(false);
        }
        // Every set-back of the round before is in the manifest now (its encoders have finished).
        handle.take_set_back();
        progress.done = handle.read(|j| j.frames_done());
        while let Some(pass) = queue.pop_front() {
            match run_pass(
                handle,
                &pass,
                settings,
                software,
                ctx,
                backend,
                out,
                &mut progress,
                control,
            )
            .await
            {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(Pass::Sequential) => {
                    let line = WorkerLine::Fallback {
                        reason: "sequential".into(),
                    };
                    send(out, &line, control);
                    for &i in pass.iter().rev() {
                        queue.push_front(vec![i]);
                    }
                }
                // A file that went missing mid-export fails the job, naming it (not retried); other decode errors
                // are retried.
                Err(Pass::Failed(e)) if e.code == ErrorCode::Decode => {
                    check_media(ctx).await?;
                    return Err(e);
                }
                Err(Pass::Failed(e)) => return Err(e),
            }
            if !control.running() {
                return Ok(true);
            }
        }
    }
    Err(internal("the encoder kept changing in one run"))
}
