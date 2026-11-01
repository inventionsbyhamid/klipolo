//! Runtime sampling of agent-made passes in the app's preview, a backstop to the measurement. A measurement can miss
//! what a setting does at a value it never took; the preview times the passes it draws and slows down one that runs far
//! over its class.
//!
//! - **What is timed.** Only in `RenderMode::Interactive`, only on a device with `TIMESTAMP_QUERY`, only `Checked`
//!   filters and generators with a verified cost (built-ins never), every [`SAMPLE_EVERY`]th frame, at most
//!   [`MAX_SAMPLED`] passes a frame: each such pass's render pass writes a timestamp at its beginning and its end. The
//!   query set, its resolve buffer and the buffer read back are made once; the frame resolves them into the read buffer,
//!   the next frame asks for it to be mapped (the device's poll thread completes it), and a later frame reads it: the
//!   drawing thread never waits for the GPU.
//! - **What counts as over.** More than [`OVER_FACTOR`] × its class's budget (`Cost::budget_ms`), scaled up for a pass
//!   bigger than the preview size it was measured at ([`MEASURED_PIXELS`]), for [`OVER_FOR_S`] on end (61 frames at
//!   30 fps).
//! - **What it does** ([`Degrade`], kept until the project closes): a generator goes to half size, then to a still (its
//!   picture drawn once, from the static cache); a filter goes to half size, then off (the layer drawn without
//!   it: a still filter would freeze the video under it). Each step is said once (`EngineEvent::EffectDegraded`) and
//!   the frame notes `frame.agentEffectSlow` once per effect. An export never samples or slows anything: it tiles
//!   (`tiles`).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use klipolo_model::contrib::ContribId;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::registry::{ShaderEffect, ShaderKind, ShaderTrust};
use crate::render::device::RenderDevice;

/// Every this many frames, the checked passes are timed.
pub const SAMPLE_EVERY: u64 = 30;
/// The most passes timed in one frame.
pub const MAX_SAMPLED: usize = 8;
/// Over this many times its class's budget...
pub const OVER_FACTOR: f64 = 2.0;
/// ...for this long, an effect is slowed down one step.
pub const OVER_FOR_S: f64 = 2.0;
/// The pixels a class's budget is measured at: the preview size within 0.92 MP.
pub const MEASURED_PIXELS: f64 = 1280.0 * 720.0;

/// How an effect is drawn after running over its budget in the preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Degrade {
    /// At half size, as a `downsample` runs a filter or `resolution: half` a generator.
    Half,
    /// Generators only: one picture, drawn once and kept.
    Still,
    /// Filters only: the layer drawn without it.
    Off,
}

/// The next step for an effect of `kind` drawn as `now` (None: drawn in full): half size, then a still (a generator) or
/// off (a filter); nothing past that.
fn next(kind: ShaderKind, now: Option<Degrade>) -> Option<Degrade> {
    match (kind, now) {
        (ShaderKind::Transition, _) => None,
        (_, None) => Some(Degrade::Half),
        (ShaderKind::Generator, Some(Degrade::Half)) => Some(Degrade::Still),
        (ShaderKind::Filter, Some(Degrade::Half)) => Some(Degrade::Off),
        (_, Some(_)) => None,
    }
}

/// The line a pass of `budget_ms`'s class may not stay over, at `pixels` drawn.
pub fn over_line(budget_ms: f64, pixels: f64) -> f64 {
    OVER_FACTOR * budget_ms * (pixels / MEASURED_PIXELS).max(1.0)
}

#[derive(Default)]
struct Track {
    /// When its timings went over the line and stayed there (seconds on the sampler's clock).
    over_since: Option<f64>,
    level: Option<Degrade>,
}

/// The decisions, from the timings alone (no GPU): per contribution, how long it has been over, and how it draws.
#[derive(Default)]
pub struct Degrader {
    tracks: HashMap<ContribId, Track>,
}

impl Degrader {
    /// A timing of `id`'s pass (`kind`): `ms` against `line` at `at_s`. Some step when it has stayed over for
    /// [`OVER_FOR_S`] (its window then starts again, so the next step needs as long).
    pub fn observe(
        &mut self,
        id: &ContribId,
        kind: ShaderKind,
        at_s: f64,
        ms: f64,
        line: f64,
    ) -> Option<Degrade> {
        let track = self.tracks.entry(id.clone()).or_default();
        if ms <= line {
            track.over_since = None;
            return None;
        }
        let since = *track.over_since.get_or_insert(at_s);
        if at_s - since < OVER_FOR_S - 1e-9 {
            return None;
        }
        let step = next(kind, track.level)?;
        track.level = Some(step);
        track.over_since = Some(at_s);
        Some(step)
    }

    /// How `id` draws now.
    pub fn level(&self, id: &ContribId) -> Option<Degrade> {
        self.tracks.get(id).and_then(|t| t.level)
    }

    /// The project closed: everything draws in full again.
    pub fn clear(&mut self) {
        self.tracks.clear();
    }
}

/// Whether `shader` is ever timed: an agent-made filter or generator with a verified cost (built-ins never are).
pub fn sampled(shader: &ShaderEffect) -> bool {
    shader.trust == ShaderTrust::Checked
        && shader.cost.is_some()
        && matches!(shader.kind, ShaderKind::Filter | ShaderKind::Generator)
}

/// A pass timed in a frame: what it was, the line it may not stay over, and when.
struct Timed {
    id: ContribId,
    kind: ShaderKind,
    line: f64,
    budget_ms: f64,
    at_s: f64,
}

/// The read buffer's state.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Read {
    Idle,
    /// Written by the frame just encoded: mapped when the next frame starts.
    Encoded,
    /// Mapping (0: pending, 1: mapped, 2: failed).
    Mapping,
}

/// The query set and its buffers, made once.
struct Gpu {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    period_ns: f64,
}

/// A step the sampler took, for the stage's events and notes.
pub struct Step {
    pub id: ContribId,
    pub to: Degrade,
    pub ms: f64,
    pub budget_ms: f64,
}

/// The sampler of one effect stage (see the module's doc).
pub struct PassSampler {
    on: bool,
    gpu: Option<Gpu>,
    timestamps: bool,
    every: u64,
    clock: Clock,
    read: Read,
    mapped: Arc<AtomicU8>,
    /// The passes timed in the frame being encoded, and in the frame whose read is under way.
    this_frame: Vec<Timed>,
    reading: Vec<Timed>,
    sampling_now: bool,
    now_s: f64,
    degrader: Degrader,
    /// Time spent in the sampler itself (the figure for the check run by hand on a Mac after a Mac build).
    cost: Duration,
}

enum Clock {
    Wall(Instant),
    /// Tests: this many seconds a frame.
    Stepped {
        step_s: f64,
        frames: u64,
    },
}

impl PassSampler {
    /// Sampling for a stage in `interactive` mode on `dev` (none otherwise, or without timestamp queries).
    pub fn new(dev: &RenderDevice, interactive: bool) -> Self {
        Self {
            on: interactive,
            gpu: None,
            timestamps: interactive && dev.has_timestamps(),
            every: SAMPLE_EVERY,
            clock: Clock::Wall(Instant::now()),
            read: Read::Idle,
            mapped: Arc::new(AtomicU8::new(0)),
            this_frame: Vec::with_capacity(MAX_SAMPLED),
            reading: Vec::with_capacity(MAX_SAMPLED),
            sampling_now: false,
            now_s: 0.0,
            degrader: Degrader::default(),
            cost: Duration::ZERO,
        }
    }

    /// Test hooks: sample every `every` frames, on a clock that moves `step_s` a frame.
    pub fn set_clock(&mut self, every: u64, step_s: f64) {
        self.every = every.max(1);
        self.clock = Clock::Stepped { step_s, frames: 0 };
    }

    /// Sampling on or off (the comparison for the check run by hand on a Mac after a Mac build).
    pub fn set_on(&mut self, on: bool) {
        self.on = on;
    }

    /// The time the sampler itself took so far.
    pub fn cost(&self) -> Duration {
        self.cost
    }

    /// How `id` draws now.
    pub fn level(&self, id: &ContribId) -> Option<Degrade> {
        self.degrader.level(id)
    }

    /// The project closed.
    pub fn clear(&mut self) {
        self.degrader.clear();
    }

    /// A frame starts (`frame` its number): the last read's timings are read if they arrived (never waiting), the
    /// frame before's asked for, and whether this frame times its passes decided. The steps taken.
    pub fn begin_frame(&mut self, dev: &RenderDevice, frame: u64) -> Vec<Step> {
        let started = Instant::now();
        self.now_s = match &mut self.clock {
            Clock::Wall(t0) => t0.elapsed().as_secs_f64(),
            Clock::Stepped { step_s, frames } => {
                *frames += 1;
                *frames as f64 * *step_s
            }
        };
        let mut steps = Vec::new();
        if let Some(gpu) = &self.gpu {
            match self.read {
                Read::Encoded => {
                    // The frame that resolved into it has been submitted: map it now.
                    self.mapped.store(0, Ordering::SeqCst);
                    let mapped = self.mapped.clone();
                    gpu.read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                        mapped.store(if r.is_ok() { 1 } else { 2 }, Ordering::SeqCst);
                    });
                    dev.poll_soon();
                    self.read = Read::Mapping;
                }
                Read::Mapping => match self.mapped.load(Ordering::SeqCst) {
                    0 => {}
                    1 => {
                        let ticks: Vec<u64> = {
                            let bytes = gpu.read.slice(..).get_mapped_range();
                            bytes
                                .chunks_exact(8)
                                .map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")))
                                .collect()
                        };
                        gpu.read.unmap();
                        let period = gpu.period_ns;
                        for (i, t) in self.reading.drain(..).enumerate() {
                            let ms =
                                ticks[2 * i + 1].saturating_sub(ticks[2 * i]) as f64 * period / 1e6;
                            if !(ms.is_finite() && ms > 0.0 && ms < 10_000.0) {
                                continue;
                            }
                            if let Some(to) =
                                self.degrader.observe(&t.id, t.kind, t.at_s, ms, t.line)
                            {
                                steps.push(Step {
                                    id: t.id,
                                    to,
                                    ms,
                                    budget_ms: t.budget_ms,
                                });
                            }
                        }
                        self.read = Read::Idle;
                    }
                    _ => {
                        gpu.read.unmap();
                        self.reading.clear();
                        self.read = Read::Idle;
                    }
                },
                Read::Idle => {}
            }
        }
        self.this_frame.clear();
        self.sampling_now = self.on
            && self.timestamps
            && self.read == Read::Idle
            && frame.is_multiple_of(self.every);
        self.cost += started.elapsed();
        steps
    }

    /// The query index pair `shader`'s pass at `pixels` writes its timestamps to, when this frame times it.
    pub fn sample(
        &mut self,
        dev: &RenderDevice,
        shader: &ShaderEffect,
        pixels: f64,
    ) -> Option<u32> {
        if !self.sampling_now || self.this_frame.len() >= MAX_SAMPLED || !sampled(shader) {
            return None;
        }
        let started = Instant::now();
        let budget_ms = shader.cost.as_ref().map_or(0.0, |c| c.budget_ms);
        if self.gpu.is_none() {
            self.gpu = Some(Gpu::new(dev));
        }
        let index = 2 * self.this_frame.len() as u32;
        self.this_frame.push(Timed {
            id: shader.id.clone(),
            kind: shader.kind,
            line: over_line(budget_ms, pixels),
            budget_ms,
            at_s: self.now_s,
        });
        self.cost += started.elapsed();
        Some(index)
    }

    /// The query set the timed passes write to (made by `sample`).
    pub fn query_set(&self) -> Option<&wgpu::QuerySet> {
        self.gpu.as_ref().map(|g| &g.set)
    }

    /// After the frame's passes: the timestamps resolved into the read buffer, in the frame's own command buffer.
    pub fn end_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let started = Instant::now();
        if let (Some(gpu), false) = (&self.gpu, self.this_frame.is_empty()) {
            let n = 2 * self.this_frame.len() as u32;
            encoder.resolve_query_set(&gpu.set, 0..n, &gpu.resolve, 0);
            encoder.copy_buffer_to_buffer(&gpu.resolve, 0, &gpu.read, 0, u64::from(n) * 8);
            std::mem::swap(&mut self.this_frame, &mut self.reading);
            self.read = Read::Encoded;
        }
        self.this_frame.clear();
        self.sampling_now = false;
        self.cost += started.elapsed();
    }
}

impl Gpu {
    fn new(dev: &RenderDevice) -> Self {
        let device = &dev.device;
        let bytes = 2 * MAX_SAMPLED as u64 * 8;
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage,
                mapped_at_creation: false,
            })
        };
        super::made();
        Self {
            set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Klipolo pass sampler"),
                ty: wgpu::QueryType::Timestamp,
                count: 2 * MAX_SAMPLED as u32,
            }),
            resolve: buffer(
                "Klipolo pass sampler resolve",
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            ),
            read: buffer(
                "Klipolo pass sampler read",
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            period_ns: f64::from(dev.queue.get_timestamp_period()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ContribId {
        ContribId::from(s)
    }

    /// `frames` timings at 30 fps from `from` (a frame number), each `ms` against `line`: the steps taken.
    fn feed(
        d: &mut Degrader,
        who: &ContribId,
        kind: ShaderKind,
        from: u32,
        frames: u32,
        ms: f64,
    ) -> Vec<Degrade> {
        let line = over_line(0.5, MEASURED_PIXELS);
        (from..from + frames)
            .filter_map(|n| d.observe(who, kind, f64::from(n) / 30.0, ms, line))
            .collect()
    }

    #[test]
    fn a_generator_over_twice_its_class_goes_to_half_size_then_to_a_still() {
        let mut d = Degrader::default();
        let g = id("lib.k3f9q2ma/generator.waves.v1");
        // 0.5 ms Light: the line is 1 ms. 60 frames over (two seconds less a frame): nothing yet.
        assert!(feed(&mut d, &g, ShaderKind::Generator, 0, 60, 1.5).is_empty());
        // The 61st.
        assert_eq!(
            feed(&mut d, &g, ShaderKind::Generator, 60, 1, 1.5),
            [Degrade::Half]
        );
        assert_eq!(d.level(&g), Some(Degrade::Half));
        // Still over for another 61: a still.
        assert_eq!(
            feed(&mut d, &g, ShaderKind::Generator, 61, 61, 1.5),
            [Degrade::Still]
        );
        // Nothing past a still.
        assert!(feed(&mut d, &g, ShaderKind::Generator, 122, 200, 9.0).is_empty());
        assert_eq!(d.level(&g), Some(Degrade::Still));
    }

    #[test]
    fn a_filter_goes_to_half_size_then_off_never_to_a_still() {
        let mut d = Degrader::default();
        let f = id("lib.k3f9q2ma/effect.glow.v1");
        let steps = feed(&mut d, &f, ShaderKind::Filter, 0, 400, 3.0);
        assert_eq!(steps, [Degrade::Half, Degrade::Off]);
        assert_eq!(d.level(&f), Some(Degrade::Off));
    }

    #[test]
    fn under_the_line_nothing_degrades_and_a_dip_starts_the_wait_again() {
        let mut d = Degrader::default();
        let g = id("lib.k3f9q2ma/generator.calm.v1");
        assert!(
            feed(&mut d, &g, ShaderKind::Generator, 0, 600, 1.0).is_empty(),
            "at the line"
        );
        assert_eq!(d.level(&g), None);
        // Over for 50 frames, a frame under, over for 50 more: never two seconds on end.
        assert!(feed(&mut d, &g, ShaderKind::Generator, 600, 50, 2.0).is_empty());
        assert!(feed(&mut d, &g, ShaderKind::Generator, 650, 1, 0.2).is_empty());
        assert!(feed(&mut d, &g, ShaderKind::Generator, 651, 50, 2.0).is_empty());
        // A bigger pass than the class was measured at has a higher line.
        assert_eq!(over_line(0.5, 4.0 * MEASURED_PIXELS), 4.0);
        assert_eq!(over_line(0.5, MEASURED_PIXELS / 4.0), 1.0);
    }

    #[test]
    fn built_ins_are_never_sampled() {
        let blur = crate::registry::Registry::builtin();
        assert!(blur.shaders().all(|s| !sampled(s)), "no built-in is timed");
        let def = klipolo_model::contrib::ContribDef::new(
            "lib.k3f9q2ma/generator.flat.v1",
            klipolo_model::contrib::ContribKind::Generator,
            "Flat",
        );
        let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";
        let checked = ShaderEffect::assemble_untrusted(&def, ShaderKind::Generator, body).unwrap();
        assert!(
            !sampled(&checked),
            "without a verified cost, nothing to compare with"
        );
        let verdict =
            crate::registry::pack::Verdict::of(crate::make::budget::Cost::with_export_ms(1.0))
                .unwrap();
        assert!(sampled(&checked.verified(&verdict)));
    }
}
