//! The GPU backend as the render worker draws with it: on the worker's own device, every frame waiting
//! for its images, its video frames from the app's stills (`FrozenStills`) and never from the decoder pool, so it opens
//! no file of the person's and runs no FFmpeg or VideoToolbox; optionally transparent; and timed for the meter.

use std::sync::Arc;

use super::GpuBackend;
use super::compositor::FrameTime;
use super::device::RenderDevice;
use super::shared_fonts::SharedFonts;
use crate::media::frozen::FrozenStills;
use crate::present::Output;
use crate::session::{RenderRequest, Rendered};

impl GpuBackend {
    /// The worker's backend on `dev`: `transparent` frames have no background. `Contained`: its checked shaders
    /// compile before the frame, with no handshake; a crash or hang is the worker's own (its deadline).
    pub(crate) fn for_worker(dev: Arc<RenderDevice>, transparent: bool) -> Self {
        let mut backend =
            Self::with_device(dev, super::mode::RenderMode::Contained).wait_for_assets();
        if let Some(compositor) = &mut backend.compositor {
            compositor.set_transparent(transparent);
        }
        backend
    }

    /// Frames without a background (cleared to transparent), or with it.
    pub(crate) fn set_transparent(&mut self, on: bool) {
        if let Some(compositor) = &mut self.compositor {
            compositor.set_transparent(on);
        }
    }

    /// The registry changed: other drafts' pipelines and kept pictures go.
    pub(crate) fn keep_shaders_of(&mut self, registry: &crate::registry::Registry) {
        if let Some(compositor) = &mut self.compositor {
            compositor.keep_shaders_of(registry);
        }
    }

    /// The effect pipelines kept (tests).
    pub(crate) fn pipelines_kept(&self) -> usize {
        self.compositor.as_ref().map_or(0, |c| c.pipelines_kept())
    }

    /// Texts are shaped with `fonts` from now on.
    pub(crate) fn set_fonts(&mut self, fonts: SharedFonts) {
        if let Some(compositor) = &mut self.compositor {
            compositor.set_fonts(fonts);
        }
    }

    pub(crate) fn device(&self) -> &Arc<RenderDevice> {
        &self.dev
    }

    /// One frame read back, its videos from `stills` (none: no video draws).
    pub(crate) async fn render_frozen(
        &mut self,
        req: RenderRequest,
        stills: Option<&FrozenStills>,
    ) -> Result<Rendered, String> {
        self.fill(&req, stills);
        let set = &self.decoded[0];
        let compositor = self.compositor.as_mut().ok_or(super::device::GPU_FAILED)?;
        compositor
            .render(&req.plan, set, (req.width, req.height), Output::Rgba, None)
            .await
    }

    /// One frame timed (`Compositor::render_timed`), its videos from `stills`; `fail_read` fails its timestamps'
    /// readback (tests).
    pub(crate) async fn render_timed(
        &mut self,
        req: RenderRequest,
        stills: Option<&FrozenStills>,
        fail_read: bool,
    ) -> Result<FrameTime, String> {
        self.fill(&req, stills);
        let set = &self.decoded[0];
        let compositor = self.compositor.as_mut().ok_or(super::device::GPU_FAILED)?;
        compositor
            .render_timed(&req.plan, set, (req.width, req.height), fail_read)
            .await
    }

    /// `frames` copies of one frame in one submission (`Compositor::render_batch`), its videos from `stills`.
    pub(crate) async fn render_batch(
        &mut self,
        req: RenderRequest,
        stills: Option<&FrozenStills>,
        frames: u32,
    ) -> Result<FrameTime, String> {
        self.fill(&req, stills);
        let set = &self.decoded[0];
        let compositor = self.compositor.as_mut().ok_or(super::device::GPU_FAILED)?;
        compositor
            .render_batch(&req.plan, set, (req.width, req.height), frames)
            .await
    }

    fn fill(&mut self, req: &RenderRequest, stills: Option<&FrozenStills>) {
        let set = &mut self.decoded[0];
        match stills {
            Some(stills) => stills.fill(&req.plan, set),
            None => FrozenStills::new().fill(&req.plan, set),
        }
    }
}
