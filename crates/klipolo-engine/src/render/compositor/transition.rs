//! A transition: each side drawn alone into a canvas-sized target through `prepare_one` and `encode_passes`,
//! then joined by the transition's pass and placed like an image under the overlays. A shader that can't be compiled
//! draws the nearer side whole (a cut at the window's middle) and is reported once.
//!
//! Nothing here recurses (an `async fn` calling itself would need a boxed future on every transition frame): a side
//! holds one clip's layers, which `prepare_one` draws. The sides' draw list and the targets are kept between frames,
//! and the effect stage keeps the pair's bind group, so a warm transition frame allocates nothing.

use super::frame::{FrameCtx, PassStart};
use super::{Compositor, Draw};
use crate::plan::framing::FULL;
use crate::plan::{Layer, TransitionLayer};
use crate::render::effects::Input;
use crate::render::pool::DecodedSet;
use crate::render::uniforms::quad_for;

impl Compositor {
    /// Prepares `t` into `draws` as one full-canvas quad. Its video layers take their frames from `frames` in
    /// `FramePlan::videos()` order: the outgoing side first.
    pub(super) async fn transition(
        &mut self,
        t: &TransitionLayer,
        frames: &DecodedSet,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
    ) {
        // Moved out so `prepare_one` can take `&mut self`, and put back after.
        let mut side = std::mem::take(&mut self.side_draws);
        let a = self.side(&t.outgoing, frames, f, &mut side).await;
        let b = self.side(&t.incoming, frames, f, &mut side).await;
        side.clear();
        self.side_draws = side;
        let dev = self.dev.clone();
        let (device, queue) = (&dev.device, &dev.queue);
        let joined = self
            .effects
            .apply(
                device,
                queue,
                f.encoder,
                Input::Pair(a, b),
                f.size,
                std::slice::from_ref(&t.pass),
                f.info,
                f.warnings,
            )
            .ok()
            .flatten();
        // A shader that can't be compiled (the effect stage reports it once): the nearer side, whole.
        let shown = joined.unwrap_or(if t.pass.progress < 0.5 { a } else { b });
        let quad = quad_for(FULL, 0.0, 1.0, 0.0, f.size);
        if let Some(i) =
            self.quads
                .push_draw(device, queue, self.effects.texture(shown), quad, f.size)
        {
            draws.push(Draw::Quad(i));
        }
    }

    /// `layers` (one side) drawn alone onto a cleared canvas-sized target, which is returned (busy for the frame
    /// until the transition's pass has read it).
    async fn side(
        &mut self,
        layers: &[Layer],
        frames: &DecodedSet,
        f: &mut FrameCtx<'_>,
        side: &mut Vec<Draw>,
    ) -> usize {
        let target = self.effects.target(&self.dev.device, f.size);
        side.clear();
        // The side's own target size (the effect stage caps a side), for what is placed in pixels (the cursor).
        let extent = self.effects.texture(target).size();
        let outer = std::mem::replace(&mut f.target, (extent.width, extent.height));
        for (index, layer) in layers.iter().enumerate() {
            self.prepare_one(index, layer, frames, f, side).await;
        }
        f.target = outer;
        // A reference count, not a copy: `encode_passes` takes `&mut self`.
        let view = self.effects.view(target).clone();
        self.encode_passes(
            f,
            &view,
            side,
            layers,
            PassStart::Clear(wgpu::Color::TRANSPARENT),
        );
        target
    }
}
