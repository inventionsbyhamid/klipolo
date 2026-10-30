//! Draws a FramePlan back to front into an RGBA texture at any output size, then reads it back as RGBA or NV12, or
//! leaves it in a native presenter's slot (nothing read back). The layer stack
//! follows Cap's render loop (rendering/src/lib.rs:6935–7100, pattern only), in one render pass per adjustment
//! layer plus one (one pass when there are none):
//! - the background;
//! - main and camera video through the vendored composite pipeline, with their clip's look and LUT;
//! - black for clip fades;
//! - overlays in track order: images, overlay video, text and generators through the vendored image quad, which
//!   rotates and blends them (normal, multiply, screen or add: one pipeline each, `blend.rs`);
//! - layers with effects are drawn alone into the effect stage first and placed by the image quad
//!   (`staged.rs`); an adjustment layer ends the pass, re-draws what is under it through its effects and starts the
//!   next pass with the result; a layer blended by overlay does the same with one read-below pass (`blend.rs`);
//! - a transition draws each side's main layers alone into a canvas-sized target, joins them with its shader and
//!   places the result like an image, under the overlays (`transition.rs`);
//!   places the result like an image, under the overlays (`transition.rs`);
//! - the cursor over its footage, in the same pass, from its own slot (`render::cursor`);
//! - the captions last, over everything, by the caption drawer (`render::captions`: instanced quads over a page's
//!   word atlas).

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Notify;

use crate::plan::FramePlan;
use crate::render::captions::CaptionDrawer;
use crate::render::cursor::{CursorPass, DrawRange};
use crate::render::device::RenderDevice;
use crate::render::effects::EffectStage;

use crate::frames::buffer::FramePool;
use crate::present::Output;
use crate::present::native::Slots;
use crate::render::device::GPU_FAILED;
use crate::render::lut::LutTextures;
use crate::render::mode::RenderMode;
use crate::render::pool::DecodedSet;
use crate::render::readback::ReadbackRing;
use crate::render::scene::SceneDrawer;
use crate::render::shared_fonts::SharedFonts;
use crate::render::text::TextDrawer;
use crate::render::text_timing::{TextTime, TextTiming};
use crate::render::video::VideoDrawers;
use crate::session::Rendered;
use crate::vendor::cap::rendering::composite_frame::CompositeVideoFramePipeline;
use crate::vendor::cap::rendering::frame_pipeline::{RenderSession, RgbaToNv12Converter};

mod backdrop;
pub(crate) mod blend;
mod draw;
mod export;
mod finish;
mod frame;
mod guards;
mod layers;
mod padded;
mod passes;
mod scene;
mod staged;
mod timed;
mod transition;
use crate::vendor::cap::rendering::layers::background::BackgroundLayer;
use crate::vendor::cap::rendering::layers::image::ImageLayer;
pub use timed::FrameTime;

pub struct Compositor {
    dev: Arc<RenderDevice>,
    background: BackgroundLayer,
    video: VideoDrawers,
    /// Black fades: background layers in colour-with-alpha mode, one per `Layer::Black` in a frame (each keeps its
    /// own uniform buffer, so two fades in one pass don't overwrite each other).
    blacks: Vec<BackgroundLayer>,
    /// Image files (decoded, cached by file version) and the quad that draws images, overlay video and text.
    quads: ImageLayer,
    /// Made on the first text layer: the system font scan behind it runs on a thread from `new`.
    texts: Option<TextDrawer>,
    /// The one font system the text and caption drawers shape with, made with the first text or caption:
    /// with `wait_for_assets` (export) it waits for the system font scan. Refreshed once a frame.
    fonts: Option<SharedFonts>,
    /// Made by the first caption or the warm-up: the pages' atlases, built on a thread of their own.
    captions: Option<CaptionDrawer>,
    /// Fired when something loading in the background is ready (the caption drawer's pipelines among them).
    redraw: Arc<Notify>,
    luts: LutTextures,
    /// Offscreen passes for layers with effects, generators and adjustment layers.
    effects: EffectStage,
    /// Motion graphics, each drawn into a target of `effects`' pool.
    scenes: SceneDrawer,
    /// This frame's draw list, kept between frames so drawing allocates nothing once warm.
    draws: Vec<Draw>,
    /// A transition's sides, drawn one after the other; kept between frames.
    side_draws: Vec<Draw>,
    session: Option<RenderSession>,
    /// Staging buffers the frames are read back through, and the buffers their pixels land in (both reused).
    ring: ReadbackRing,
    frames: FramePool,
    /// RGBA → NV12 (BT.709) on the GPU for playback frames sent over the socket; made on first use.
    nv12: Option<RgbaToNv12Converter>,
    /// Export waits for images and LUTs to load; the preview draws without them and redraws when they're ready.
    wait_for_assets: bool,
    /// The render worker's fonts were given (`set_fonts`): its scene text never waits for the font scan.
    fonts_given: bool,
    /// The last output size (the lookahead sizes images for it).
    last_size: (u32, u32),
    /// Effects on a texture stored turned run on it turned upright; only a test turns it off, to measure
    /// that pass.
    upright: bool,
    /// This frame's overlay-blended layers (`Draw::Overlay`), kept between frames.
    overlays: Vec<Overlaid>,
    /// Scene layers waiting for the frame under them (`Draw::Backdrop`), kept between frames, and how many this frame.
    backdrops: Vec<backdrop::Backdropped>,
    backdrops_used: usize,
    /// Render passes this frame encoded outside the effect stage (layer passes, a main footage drawn alone).
    passes: u32,
    /// Every render pass the last drawn frame encoded (`passes_last_frame`).
    passes_last: u32,
    /// The render worker's: no background, cleared to transparent; its frames' timer (made on the first
    /// timed frame, when the device has timestamps) and whether the frame being drawn uses it.
    transparent: bool,
    timer: Option<frame::FrameTimer>,
    timing: bool,
    /// Set only while the frame's own passes are encoded: their last writes the second timestamp.
    timing_end: Option<wgpu::QuerySet>,
    /// How it treats agent-made shaders: the app's preview, or contained (export, worker, tests).
    mode: RenderMode,
    /// The perf harness's timing of the text drawers (`time_text`); None in the app. (Not the render
    /// worker's frame `timing` above.)
    text_timing: Option<TextTiming>,
    /// The cursor pass: made by the first cursor layer, a preload or `warm_all`; its atlas goes with `forget`.
    cursor: Option<CursorPass>,
}

/// A layer blended by overlay, waiting for what is under it: its texture (after its effects) and where it lands.
struct Overlaid {
    texture: wgpu::Texture,
    quad: crate::vendor::cap::rendering::layers::image::ImageQuad,
}

enum Draw {
    Video(usize, wgpu::BindGroup),
    Black(usize),
    Quad(usize),
    /// The adjustment layer at this index of `plan.layers`: the pass ends here.
    Adjust(usize),
    /// The overlay-blended layer at this index of `overlays`: the pass ends here too.
    Overlay(usize),
    /// The scene layer with a backdrop kept as this index of `backdrops`: the pass ends here too.
    Backdrop(usize),
    /// The captions, in the caption drawer's slot.
    Caption(usize),
    /// The perf harness's text timing (`time_text`): a title's or the captions' draws start here (true) or end.
    Mark(bool),
    /// A title built ahead, touched once on the frame it landed: its quad draw at this index, at
    /// zero opacity, scissored to the target's first pixel.
    Touch(usize),
    /// A cursor layer's slot (`CursorPass::draw`).
    Cursor(DrawRange),
}

impl Compositor {
    /// A compositor on `dev`, drawing again on `redraw`, treating agent-made shaders as `mode` says (there is no
    /// default: the app's preview passes `Interactive`, everything else `Contained`).
    pub fn new(dev: Arc<RenderDevice>, redraw: Arc<Notify>, mode: RenderMode) -> Self {
        let (device, queue) = (&dev.device, &dev.queue);
        let composite = Arc::new(CompositeVideoFramePipeline::new(device));
        TextDrawer::scan_fonts_in_background(redraw.clone());
        let mut effects = EffectStage::new(&dev, mode);
        effects.set_wake(redraw.clone());
        Self {
            luts: LutTextures::new(device, queue, &composite, redraw.clone()),
            video: VideoDrawers::new(device, composite),
            background: BackgroundLayer::new(device),
            blacks: Vec::new(),
            quads: ImageLayer::new(device, redraw.clone()),
            effects,
            scenes: SceneDrawer::with_redraw(redraw.clone()),
            texts: None,
            fonts: None,
            captions: None,
            redraw,
            draws: Vec::new(),
            side_draws: Vec::new(),
            session: None,
            ring: ReadbackRing::default(),
            frames: FramePool::new(),
            nv12: None,
            wait_for_assets: false,
            fonts_given: false,
            last_size: (1280, 720),
            upright: true,
            overlays: Vec::new(),
            backdrops: Vec::new(),
            backdrops_used: 0,
            passes: 0,
            passes_last: 0,
            transparent: false,
            timer: None,
            timing: false,
            timing_end: None,
            mode,
            text_timing: None,
            cursor: None,
            dev,
        }
    }

    /// Times the text drawers' own work from the next frame on (the perf harness), or stops.
    pub(crate) fn time_text(&mut self, on: bool) {
        self.text_timing = on.then(|| TextTiming::new(&self.dev.device, &self.dev.queue));
        let stamps = self.text_timing.as_ref().and_then(TextTiming::stamps);
        if let Some(texts) = &mut self.texts {
            texts.time_gpu(stamps);
        }
    }

    /// What the last frame's text drawers cost (`time_text`; zeros when off). Waits for the GPU to read its timestamps.
    pub(crate) fn text_time(&self) -> TextTime {
        self.text_timing
            .as_ref()
            .map(|t| t.last(&self.dev.device))
            .unwrap_or_default()
    }

    /// Wait for images and LUTs to load instead of drawing without them (export, tests).
    pub fn set_wait_for_assets(&mut self, wait: bool) {
        if wait != self.wait_for_assets {
            // Fonts made by `warm_all` keep the flag they were made with: an export's must wait for the scan, the
            // caption build thread's too.
            self.texts = None;
            self.fonts = None;
            self.captions = None;
        }
        self.wait_for_assets = wait;
    }

    /// Built-in passes whose settings allow it run at half size (on by default; see `GpuBackend::set_downsample`).
    pub(crate) fn set_downsample(&mut self, on: bool) {
        self.effects.set_downsample(on);
    }

    /// Effects on turned textures run upright (on by default; see `GpuBackend::set_upright`).
    pub(crate) fn set_upright(&mut self, on: bool) {
        self.upright = on;
    }

    /// How many render passes the last drawn frame encoded: its layer passes (one, plus one after each adjustment or
    /// overlay-blended layer), a transition's sides, and every effect-stage pass (effects, an adjustment's grab, an
    /// overlay's read-below pass). Glyph rasterising, video conversion and mipmaps aren't counted: they run when a
    /// texture changes, not per layer. For the motion cost test.
    #[doc(hidden)]
    pub fn passes_last_frame(&self) -> u32 {
        self.passes_last
    }

    /// What the titles and captions cost and hold (tests and the perf harness); zeros before the first.
    pub(crate) fn text_stats(
        &self,
    ) -> (
        crate::render::text::TitleStats,
        crate::render::captions::CaptionStats,
    ) {
        (
            self.texts
                .as_ref()
                .map(TextDrawer::stats)
                .unwrap_or_default(),
            self.captions
                .as_ref()
                .map(CaptionDrawer::stats)
                .unwrap_or_default(),
        )
    }

    /// An RGBA frame read back to memory (export, tests).
    pub async fn render_rgba(
        &mut self,
        plan: &FramePlan,
        frames: &DecodedSet,
        size: (u32, u32),
    ) -> Result<Rendered, String> {
        self.render(plan, frames, size, Output::Rgba, None).await
    }

    /// Draws `plan` at `size` and finishes it as `output`: RGBA or NV12 pixels read back (`Rendered::pixels`
    /// resolves when the GPU is done), or drawn into a slot of `native` and nothing read back.
    pub(crate) async fn render(
        &mut self,
        plan: &FramePlan,
        frames: &DecodedSet,
        size: (u32, u32),
        output: Output,
        native: Option<&Arc<Slots>>,
    ) -> Result<Rendered, String> {
        let dev = self.dev.clone();
        // 1. A render target of `size`: a native slot (the presenter copies it to the screen), or the session texture
        //    that is read back.
        let slot = match (output, native) {
            (Output::Native, Some(slots)) => Some(slots.lease(&dev.device, size)),
            (Output::Native, None) => return Err(GPU_FAILED.into()),
            _ => None,
        };
        let target = match &slot {
            Some(slot) => slot.view().clone(),
            None => self.session_view(size),
        };
        let (encoder, warnings, missing) = self.draw(plan, frames, size, &target).await;

        // 5. Submit, finished as `output` (`finish.rs`).
        let (stride, format, pixels) = self.finish(&dev, slot, output, size, encoder);
        // A validation error anywhere in this frame (already logged by the device's handler) fails the frame.
        let faulted = dev.take_fault().is_some();
        self.frame_submitted(faulted);
        if faulted {
            return Err(GPU_FAILED.into());
        }
        Ok(Rendered {
            pixels,
            width: size.0,
            height: size.1,
            stride,
            format,
            warnings,
            missing,
            not_answering: frames.not_answering.clone(),
        })
    }

    /// After a frame's submit, on every frame path (`render`, `render_yuv`): the captions' staging buffers it copied from
    /// are mapped again. A frame that `faulted` (wgpu drops a submit that fails
    /// validation whole, the captions' copy with it) or was given up before its submit leaves the caption drawer
    /// uploading everything again. Counted in the drawers' CPU for the perf harness.
    fn frame_submitted(&mut self, faulted: bool) {
        let Some(captions) = &mut self.captions else {
            return;
        };
        let started = self.text_timing.as_ref().map(|_| Instant::now());
        if faulted {
            captions.forget_uploads();
        }
        captions.after_submit();
        if let (Some(timing), Some(started)) = (&mut self.text_timing, started) {
            timing.add_after_frame(started);
        }
    }

    /// The session texture at `size` (made or resized), as a render target.
    fn session_view(&mut self, size: (u32, u32)) -> wgpu::TextureView {
        let device = &self.dev.device;
        match &mut self.session {
            Some(session) => session.update_texture_size(device, size.0, size.1),
            None => self.session = Some(RenderSession::new(device, size.0, size.1)),
        }
        self.session
            .as_ref()
            .expect("made above")
            .current_texture_view()
            .clone()
    }
}
