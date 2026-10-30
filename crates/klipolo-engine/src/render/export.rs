//! Export rendering on `GpuBackend::for_export`: every format's plan for one instant shares one decode, each
//! format is drawn by its own compositor on the shared device (its render target stays its size), converted on the
//! GPU and submitted. The backend's rule holds: at most two frames on the GPU, and a frame's decoded set (with the
//! IOSurfaces it holds) is reused only after its GPU work is done.
//!
//! A frame's completion comes from a Metal command buffer's completed handler (`FrameDone`), not from wgpu's
//! blocking poll: `Device::maintain` holds the device's fence lock while it waits, so the next frame's
//! `Queue::submit` would stall until the GPU finished the previous one (measured: 70 % of the export loop).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

use super::GpuBackend;
use super::compositor::Compositor;
use super::device::RenderDevice;
use super::export_tuning;
use super::pool::{DecodedSet, DecoderPool};
use super::yuv::{Yuv, YuvConverter, YuvOut, YuvPixels, YuvSink};
use crate::plan::FramePlan;

mod frame_problem;
mod notes;
use frame_problem::frame_problem;

/// Buffers per format in the IOSurface pool: two frames on the GPU, the encoder's queue (3) and VideoToolbox's own
/// look-ahead with B-frames (measured 11 at 1080p). The pool makes them as needed.
#[cfg(target_os = "macos")]
const SURFACES_PER_FORMAT: usize = 16;

/// ProRes frames (10-bit 4:2:2) VideoToolbox may hold at once, in bytes of our buffers. ProRes is
/// intra-only, so VideoToolbox keeps no frame back for reordering, but when its encoder is the slower side (4K on
/// Intel's software ProRes) it takes every frame the GPU draws and works on each from a copy of its own of the same
/// size: 4K Master held 11 of our 33 MB buffers and as much again inside VideoToolbox, about 1.14 GB in the worker.
/// A buffer it still uses has its IOSurface in use (also for a while after the frame is encoded); the GPU draws the
/// next frame only while fewer than this are (looked at again as VideoToolbox lets each frame go).
#[cfg(target_os = "macos")]
const PRORES_BUSY_BYTES: u64 = 128 << 20;
/// Never fewer: the encoder always has the next frame at hand.
#[cfg(target_os = "macos")]
const PRORES_MIN_BUSY: usize = 2;

/// `PRORES_BUSY_BYTES`, or in a trial build `KLIPOLO_TRIAL_PRORES_BUSY_MB` (0: no bound), for the on-device A/B.
#[cfg(target_os = "macos")]
fn prores_busy_bytes() -> u64 {
    #[cfg(feature = "trial")]
    if let Some(mb) = std::env::var("KLIPOLO_TRIAL_PRORES_BUSY_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        return if mb == 0 { u64::MAX } else { mb << 20 };
    }
    PRORES_BUSY_BYTES
}

/// How many of `yuv`'s buffers at `padded` VideoToolbox may hold at once: ProRes's by `PRORES_BUSY_BYTES`, the
/// others any (H.264 and HEVC hold frames back for B-frames; their encoder runs in VideoToolbox's own process).
#[cfg(target_os = "macos")]
fn busy_limit(yuv: Yuv, (width, height): (u32, u32)) -> usize {
    if yuv != Yuv::P210 {
        return SURFACES_PER_FORMAT;
    }
    // 16 bits per sample: the luma plane, and the chroma plane's Cb and Cr pairs over half the width.
    let frame = 2 * u64::from(width) * u64::from(height)
        + 4 * u64::from(width.div_ceil(2)) * u64::from(Yuv::P210.chroma_rows(height));
    usize::try_from(prores_busy_bytes() / frame.max(1))
        .unwrap_or(SURFACES_PER_FORMAT)
        .clamp(PRORES_MIN_BUSY, SURFACES_PER_FORMAT)
}

/// Completion of the GPU work submitted before it was made (see the module docs).
#[derive(Clone)]
pub struct FrameDone(Arc<(AtomicBool, Notify)>);

impl FrameDone {
    fn finished() -> Self {
        Self(Arc::new((AtomicBool::new(true), Notify::new())))
    }

    /// Fires when the GPU work submitted so far is done (`platform::gpu::on_submitted_done`: a Metal command
    /// buffer's completed handler). Where the platform can't say, it is finished at once.
    fn mark(dev: &RenderDevice) -> Self {
        let state = Arc::new((AtomicBool::new(false), Notify::new()));
        let fired = state.clone();
        let marked = crate::platform::gpu::on_submitted_done(&dev.queue, move || {
            fired.0.store(true, Ordering::Release);
            // Every waiter: drawing (a frame slot) and delivery may wait for the same frame.
            fired.1.notify_waiters();
        });
        if !marked {
            state.0.store(true, Ordering::Release);
        }
        Self(state)
    }

    /// Waits until the GPU finished that work. A GPU that hasn't after 10 s is reported (the export stops and runs
    /// again from its finished segments) rather than drawing over a frame still in use.
    pub async fn wait(&self) -> Result<(), String> {
        let (done, notify) = &*self.0;
        let notified = notify.notified();
        tokio::pin!(notified);
        // Registered before the check, so a completion between the two isn't missed.
        notified.as_mut().enable();
        if !done.load(Ordering::Acquire)
            && tokio::time::timeout(Duration::from_secs(10), notified)
                .await
                .is_err()
        {
            return Err("an export frame wasn't finished on the GPU after 10 s".into());
        }
        Ok(())
    }
}

/// Every format of an export pass, with the pass's ring of in-flight frames.
pub struct FormatTargets {
    targets: Vec<FormatTarget>,
    done: [FrameDone; 3],
    next: usize,
    /// A text in a font that isn't here may draw its stand-in (the job's `allow_stand_ins`); CJK without its Noto
    /// Sans never may.
    allow_stand_ins: bool,
    /// What the frames drew reduced and the export reports, each code once with its first time (`notes.rs`).
    notes: Vec<(String, klipolo_model::time::Us)>,
}

impl FormatTargets {
    pub fn new(targets: Vec<FormatTarget>) -> Self {
        Self {
            targets,
            done: [
                FrameDone::finished(),
                FrameDone::finished(),
                FrameDone::finished(),
            ],
            next: 0,
            allow_stand_ins: false,
            notes: Vec::new(),
        }
    }

    /// These targets drawing a missing font's stand-in instead of stopping (`ExportJob::allow_stand_ins`).
    pub fn allowing_stand_ins(mut self, allow: bool) -> Self {
        self.allow_stand_ins = allow;
        self
    }

    pub fn iter(&self) -> std::slice::Iter<'_, FormatTarget> {
        self.targets.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, FormatTarget> {
        self.targets.iter_mut()
    }
}

/// One output format: its compositor, size, conversion and decoded sets.
pub struct FormatTarget {
    compositor: Compositor,
    size: (u32, u32),
    out: YuvOut,
    sets: [DecodedSet; 3],
}

impl FormatTarget {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Its compositor (a pass puts the cursor shapes in its atlas before the first frame).
    pub fn compositor_mut(&mut self) -> &mut Compositor {
        &mut self.compositor
    }

    /// Test hook: this format's scene text build thread is gone.
    #[cfg(any(test, feature = "test-support"))]
    pub fn lose_scene_text_thread(&mut self) {
        self.compositor.scene_drawer().lose_text_thread();
    }

    pub fn layout(&self) -> super::yuv::PlaneLayout {
        self.out.converter.layout()
    }

    /// What this format's titles and captions cost and hold (`Compositor::text_stats`): the engine's tests.
    pub(crate) fn text_stats(&self) -> (super::text::TitleStats, super::captions::CaptionStats) {
        self.compositor.text_stats()
    }

    /// The distinct IOSurfaces this format's pool made (0 when reading back).
    pub fn surface_peak(&self) -> usize {
        match &self.out.sink {
            #[cfg(target_os = "macos")]
            YuvSink::Surface(pool) => pool.peak,
            YuvSink::Readback => 0,
        }
    }
}

/// Why an export frame couldn't be drawn.
#[derive(Debug)]
pub enum ExportFailure {
    /// A video couldn't be opened or decoded (the user-facing message).
    Decode(String),
    /// Something else the frame needs is missing (an image, a LUT, a plugin).
    Content(String),
    /// The frame couldn't be drawn whole (an effect or text budget, a protection that couldn't run): a code
    /// (`exports.frame.*`) with its time. A degraded frame is never written.
    Frame(crate::api::CodedError),
    Gpu(String),
    /// A video's disk didn't answer within `EXPORT_STAT_LIMIT` (asleep for good, a network volume gone): its path.
    NotAnswering(Arc<str>),
}

impl GpuBackend {
    /// `for_export`, decoding with FFmpeg in software instead of AVAssetReader/VideoToolbox (SoftwareSafe mode).
    pub async fn for_export_with(software_decode: bool) -> Result<Self, String> {
        let mut backend = Self::for_export().await?;
        if software_decode {
            backend.pool = DecoderPool::new(export_tuning(), true).patient();
        }
        Ok(backend)
    }

    /// A format drawn at `size` and converted to `yuv`: into IOSurfaces when `zero_copy`, else read back.
    pub fn format_target(
        &self,
        size: (u32, u32),
        yuv: Yuv,
        zero_copy: bool,
    ) -> Result<FormatTarget, String> {
        // The export renderer: `Contained`, its slow checked passes in tiles.
        let mut compositor = Compositor::new(
            self.dev.clone(),
            self.redraw.clone(),
            super::mode::RenderMode::Contained,
        );
        compositor.set_wait_for_assets(true);
        compositor.set_tiled(true);
        // H.264 and HEVC encode whole 16-pixel macroblocks: their buffers carry the padding (ProRes takes the frame
        // as it is; read-back frames go to FFmpeg, which pads them itself).
        let padded = if zero_copy && yuv != Yuv::P210 {
            (size.0.next_multiple_of(16), size.1.next_multiple_of(16))
        } else {
            size
        };
        let converter = YuvConverter::new(&self.dev.device, &self.dev.queue, yuv, size);
        let sink = if zero_copy {
            surface_sink(&self.dev.device, yuv, size, padded)?
        } else {
            YuvSink::Readback
        };
        Ok(FormatTarget {
            compositor,
            size,
            out: YuvOut { converter, sink },
            sets: Default::default(),
        })
    }

    /// Draws `plans[i]` into `targets[i]` (one instant, every format) and submits them; their pixels go to
    /// `pixels` in the same order. Returns the frame's GPU completion: hand IOSurfaces to an encoder only once it is
    /// done.
    pub(crate) async fn render_formats(
        &mut self,
        plans: &[FramePlan],
        all: &mut FormatTargets,
        pixels: &mut Vec<YuvPixels>,
    ) -> Result<FrameDone, ExportFailure> {
        self.ensure_device().await.map_err(ExportFailure::Gpu)?;
        let index = all.next;
        all.next = (index + 1) % all.done.len();
        // The frame two before this one must be off the GPU (then the one three before, whose sets this frame
        // reuses, is too).
        all.done[(index + 1) % all.done.len()]
            .wait()
            .await
            .map_err(ExportFailure::Gpu)?;
        // Let wgpu retire finished submissions (never waits).
        let _ = self.dev.device.poll(wgpu::PollType::Poll);
        let targets = &mut all.targets;
        pixels.clear();
        for k in 0..targets.len() {
            let (before, rest) = targets.split_at_mut(k);
            let target = &mut rest[0];
            let set = &mut target.sets[index];
            if !share(before, plans, index, &plans[k], set) {
                self.pool.decode(&plans[k], set).await;
            }
            if let Some(path) = set.missing.first() {
                return Err(ExportFailure::Decode(super::pool::missing(path)));
            }
            if let Some(path) = set.not_answering.first() {
                return Err(ExportFailure::NotAnswering(path.clone()));
            }
            if let Some(message) = set.warnings.first() {
                return Err(ExportFailure::Decode(message.clone()));
            }
            let (frame, warnings) = target
                .compositor
                .render_yuv(&plans[k], set, target.size, &mut target.out)
                .await
                .map_err(ExportFailure::Gpu)?;
            // A frame that couldn't be drawn whole is never written (hidden or missing effects never ship): the
            // export stops, saying when, why and what to do. A missing font's stand-in is drawn when the job allows it;
            // a motion graphic past its caps is drawn as the preview draws it, and reported (`notes.rs`).
            let stand_ins = all.allow_stand_ins;
            // An effect being checked is left out of the job, as the preview leaves it out.
            let at = plans[k].at_us;
            let refused = warnings.into_iter().find(|w| {
                let stand_in = stand_ins && w == crate::api::FRAME_FONT_MISSING;
                let checking = crate::api::is_agent_effect_checking(w);
                !(stand_in || checking || notes::reported(&mut all.notes, w, at))
            });
            if let Some(message) = refused {
                // A font that isn't here is named, from this very frame.
                let problem = if message == crate::api::FRAME_FONT_MISSING {
                    frame_problem::font_problem(&plans[k], &crate::render::fonts::index())
                } else {
                    frame_problem(&message, plans[k].at_us)
                };
                return Err(ExportFailure::Frame(problem));
            }
            pixels.push(frame);
        }
        let done = FrameDone::mark(&self.dev);
        all.done[index] = done.clone();
        Ok(done)
    }
}

#[cfg(target_os = "macos")]
fn surface_sink(
    device: &wgpu::Device,
    yuv: Yuv,
    size: (u32, u32),
    padded: (u32, u32),
) -> Result<YuvSink, String> {
    super::yuv::SurfacePool::new(
        device,
        yuv,
        size,
        padded,
        SURFACES_PER_FORMAT,
        busy_limit(yuv, padded),
    )
    .map(YuvSink::Surface)
}

#[cfg(not(target_os = "macos"))]
fn surface_sink(_: &wgpu::Device, _: Yuv, _: (u32, u32), _: (u32, u32)) -> Result<YuvSink, String> {
    Ok(YuvSink::Readback)
}

/// Fills `set` for `plan` from the frames earlier formats (`earlier`, drawing `plans`) decoded into their set
/// `index` at this instant. False when a video layer shows something they didn't (then `plan` is decoded; the
/// decoders' caches still serve repeated frames).
fn share(
    earlier: &[FormatTarget],
    plans: &[FramePlan],
    index: usize,
    plan: &FramePlan,
    set: &mut DecodedSet,
) -> bool {
    set.frames.clear();
    set.warnings.clear();
    set.missing.clear();
    set.not_answering.clear();
    for v in plan.videos() {
        let found = earlier.iter().zip(plans).find_map(|(t, p)| {
            let s = &t.sets[index];
            p.videos()
                .zip(&s.frames)
                .find(|(o, _)| {
                    o.source.asset_id == v.source.asset_id && o.source.time_us == v.source.time_us
                })
                .and_then(|(_, f)| f.clone())
        });
        match found {
            Some(frame) => set.frames.push(Some(frame)),
            None => return false,
        }
    }
    true
}
