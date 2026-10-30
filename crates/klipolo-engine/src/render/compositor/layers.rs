//! What the compositor keeps for its layers between frames: the pipelines a first frame (or any frame) needs, made
//! ahead of it (`warm_first_frame`, `warm_all`), the images, LUTs and titles a coming frame needs, loaded or built
//! ahead of it (`warm`, the playback lookahead, cursor shapes included), and what a hidden window or a closed project
//! lets go (`forget`).

use std::path::Path;
use std::sync::Arc;

use super::Compositor;
use crate::plan::{FramePlan, Layer};
use crate::render::captions::CaptionDrawer;
use crate::render::cursor::CursorPass;
use crate::render::device::RenderDevice;
use crate::render::shared_fonts::SharedFonts;
use crate::render::text::TextDrawer;
use crate::render::uniforms::quad_for;
use crate::vendor::cap::rendering::frame_pipeline::RgbaToNv12Converter;

impl Compositor {
    /// Makes now what a typical project's first frame draws with beyond what `new` makes (the NV12 video composite,
    /// background and image quad): text, glyphs and boxes (titles are common at a project's start), and the
    /// caption drawer, whose build thread makes its pipelines and builds every page.
    pub fn warm_first_frame(&mut self) {
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        let wait = self.wait_for_assets;
        self.fonts.get_or_insert_with(|| SharedFonts::new(wait));
        self.texts
            .get_or_insert_with(|| TextDrawer::shared(device, queue).for_export(wait))
            .warm(device);
        let (dev, redraw) = (&self.dev, &self.redraw);
        self.captions
            .get_or_insert_with(|| CaptionDrawer::new(dev, Some(redraw.clone()), wait));
    }

    /// Compiles now every pipeline a frame can need, so no frame waits for the shader compiler: the first frame's
    /// (`warm_first_frame`), the socket's NV12 conversion, video without an IOSurface and its YUV converters, overlay
    /// video's scaler, and the built-in effects, generators and transitions (every registry shader; a transition's
    /// two-input bind group uses the stage's input layout, so it needs no pipeline of its own); the native
    /// presenter's copy in `GpuBackend::warm_all`. This is the one list. The GPU warm-up runs it on a
    /// device of its own (`render::warm`), so each later comes from Metal's shader cache; the compositor otherwise
    /// makes each the first time a frame needs it.
    pub fn warm_all(&mut self) {
        self.warm_first_frame();
        let device = &self.dev.device;
        self.nv12
            .get_or_insert_with(|| RgbaToNv12Converter::new(device));
        self.video.warm_all(device);
        self.effects.warm();
        self.scenes.warm(device);
        self.scenes.warm_passes(device, &mut self.effects);
        self.quads.warm_pipelines(device);
        self.cursor_pass();
    }

    /// The cursor pass, made now if it isn't yet (its pipeline and ring: about 2 KB until a shape arrives).
    pub(super) fn cursor_pass(&mut self) -> &mut CursorPass {
        let (device, redraw) = (&self.dev.device, &self.redraw);
        self.cursor
            .get_or_insert_with(|| CursorPass::new(device, redraw.clone()))
    }

    /// Export and headless, before frame 1: `f` with the cursor pass (made now if it isn't yet) and the device, to put
    /// the shapes in range in its atlas (`export::prepare::prepare_cursors`).
    pub fn with_cursor_pass<R>(
        &mut self,
        f: impl FnOnce(&mut CursorPass, &RenderDevice) -> R,
    ) -> R {
        let dev = self.dev.clone();
        f(self.cursor_pass(), &dev)
    }

    /// Test hook: the cursor pass was made (none where no clip draws a cursor).
    #[doc(hidden)]
    pub fn cursor_pass_made(&self) -> bool {
        self.cursor.is_some()
    }

    /// `f` for each cursor shape refused since the last call (its image didn't decode), so its take plans the Arrow
    /// look from then on. No allocation.
    pub fn take_refused_cursor_shapes(&mut self, f: impl FnMut(&Arc<str>, u16)) {
        if let Some(cursor) = &mut self.cursor {
            cursor.take_refused(f);
        }
    }

    /// Starts loading the images and LUTs `plan` needs, so they are ready when it is drawn (playback lookahead), and
    /// in the preview has its titles built on the title build thread (`TextDrawer::ahead`).
    pub fn warm(&mut self, plan: &FramePlan) {
        for layer in &plan.layers {
            self.warm_one(layer, plan.canvas);
        }
    }

    fn warm_one(&mut self, layer: &Layer, canvas: (u32, u32)) {
        if let Layer::Transition(t) = layer {
            for side in t.outgoing.iter().chain(&t.incoming) {
                self.warm_one(side, canvas);
            }
            return;
        }
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        match layer {
            // An export draws every frame in turn and never warms; its titles are built as their first frame draws.
            Layer::Text(text) if !self.wait_for_assets => {
                let texts = self
                    .texts
                    .get_or_insert_with(|| TextDrawer::shared(device, queue).for_export(false));
                texts.ahead(&self.dev, text, canvas, self.last_size);
            }
            Layer::Image(image) => {
                let quad = quad_for(image.dest, 0.0, 1.0, 0.0, self.last_size);
                // As `prepare_one` decodes it: for the largest box its motion reaches.
                let side = (quad.size.x.max(quad.size.y) * image.reach).ceil() as u32;
                self.quads
                    .warm(device, queue, Path::new(&*image.path), side);
            }
            Layer::Video(v) => {
                if let Some(lut) = v.look.as_ref().and_then(|l| l.lut.as_ref()) {
                    self.luts.warm(device, queue, lut);
                }
            }
            Layer::Scene(l) => self.warm_scene(l, canvas),
            // Its shape decodes off the render thread, ready when the frame draws.
            Layer::Cursor(c) => self.cursor_pass().request(c),
            Layer::Text(_)
            | Layer::Caption(_)
            | Layer::Generator(_)
            | Layer::Adjustment(_)
            | Layer::Black { .. }
            | Layer::Transition(_) => {}
        }
    }

    /// Drops per-project GPU textures (video frames, images, text, LUTs, the render targets, the cursor atlas);
    /// pipelines stay.
    pub fn forget(&mut self) {
        self.video.forget();
        self.background.forget();
        self.quads.forget();
        // The glyph atlas, the caption atlases and the font system go too; the next text or caption makes them again.
        self.texts = None;
        self.fonts = None;
        self.captions = None;
        self.luts.forget();
        self.effects.forget();
        self.scenes.forget();
        self.session = None;
        self.nv12 = None;
        if let Some(cursor) = &mut self.cursor {
            cursor.forget(&self.dev.device);
        }
    }
}

impl crate::render::GpuBackend {
    /// Headless, before frame 1: `f` with the compositor's cursor pass and the device (`Compositor::with_cursor_pass`).
    /// None while the GPU warm-up is still making the compositor: its strict frames then load each shape as they draw
    /// it (`CursorPass::ensure`).
    pub fn with_cursor_pass<R>(
        &mut self,
        f: impl FnOnce(&mut CursorPass, &RenderDevice) -> R,
    ) -> Option<R> {
        Some(self.compositor.as_mut()?.with_cursor_pass(f))
    }

    /// Test hook: some cursor shape the preview asked for hasn't arrived yet (the export cursor pixel test waits for
    #[doc(hidden)]
    pub fn cursor_shapes_waiting(&self) -> bool {
        let pass = self.compositor.as_ref().and_then(|c| c.cursor.as_ref());
        pass.is_some_and(CursorPass::shapes_waiting)
    }

    /// Test hook: the compositor made its cursor pass (none where no clip draws a cursor).
    #[doc(hidden)]
    pub fn cursor_pass_made(&self) -> bool {
        self.compositor
            .as_ref()
            .is_some_and(Compositor::cursor_pass_made)
    }

    /// After a frame: the shapes its cursor pass refused are marked on the takes of `set_cursors`' cache, so the next
    /// plan draws the Arrow look for them, as export does. Nothing to do on almost every frame.
    pub(crate) fn refuse_cursor_shapes(&mut self) {
        let cursors = self.cursors.as_deref();
        if let Some(compositor) = &mut self.compositor {
            compositor.take_refused_cursor_shapes(|take, shape| {
                if let Some(cursors) = cursors {
                    cursors.refuse_shape(take, shape);
                }
            });
        }
    }
}
