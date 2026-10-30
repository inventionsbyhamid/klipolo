//! An offscreen node laid back, in CSS's order: its filters
//! first (its blur, then its shadow under it), then its mask, then its opacity, blend, clips and tilt (`composite.rs`).
//! Each filter and the mask is one of the effect stage's passes into a pooled target of the node's own box,
//! which frees what it read (the tint reads a texture and frees nothing). A normal-blend node that doesn't tilt (masked
//! or not) is then drawn over the target it began in as a quad (`quad_lay.rs`: no pass of the target's
//! size, no ping-pong partner); any other is laid by the host pass, at the size of the target it began in. Its blurs are the
//! scene's own: the built-in blur's unprotected twin, run whole only while the frame's budget for layers' passes
//! has room for them, else left out (said as the stage says an effect left out for the budget:
//! `engine.frame.effectsLeftOut`), never drawn into the headroom kept for protective passes nor said as a protection
//! that failed.

use super::composite::{Lay, blur_pass};
use super::encode::{Enc, Open};
use super::groups::Iso;
use crate::render::effects::Input;

/// Where a node was laid back: the target holding the result, and whether a host pass made it (a new target of the
/// one it began in's size) rather than a quad drawn over that one.
pub(super) struct Laid {
    pub target: usize,
    pub hosted: bool,
}

impl Enc<'_, '_> {
    /// Offscreen node `iso`'s target (`group`, with its mask's) laid back into `parent`, the target it began in. None
    /// when it couldn't be drawn (the budget: the node isn't shown).
    pub(super) fn lay_back(
        &mut self,
        iso: &Iso,
        (group, mask, parent): (Open, Option<usize>, Open),
    ) -> Option<Laid> {
        let (size, c) = (group.at.size, self.with.composites);
        let mut g = group.target;
        if let Some(pass) = self.with.blur.and_then(|b| blur_pass(b, iso.blur))
            && self.stage.room_for(size, 2, self.warnings)
        {
            g = self.run(Input::Target(g), size, &pass)?.unwrap_or(g);
        }
        if let Some(sh) = iso.shadow
            && self.stage.room_for(size, 3, self.warnings)
        {
            // Its silhouette tinted (what it read stays: it is laid over), blurred, then laid under it.
            let silhouette = self.stage.texture(g).clone();
            let mut shadow = self.run(Input::Texture(&silhouette), size, &c.tint(sh.color))?;
            if let (Some(s), Some(pass)) =
                (shadow, self.with.blur.and_then(|b| blur_pass(b, sh.sigma)))
            {
                shadow = self.run(Input::Target(s), size, &pass)?.or(Some(s));
            }
            if let Some(s) = shadow {
                let under = c.lay(&Lay {
                    mode: 0.0,
                    opacity: 1.0,
                    offset: sh.offset,
                    gbox: [0.0, 0.0, size.0 as f32, size.1 as f32],
                    porigin: group.at.origin.map(|v| v as f32),
                    ..Lay::default()
                });
                g = self.run(Input::Pair(s, g), size, &under)?.unwrap_or(g);
            }
        }
        if let Some((m, t)) = iso.mask.zip(mask) {
            g = self.run(Input::Pair(g, t), size, &c.mask(m.luma, m.invert))??;
        }
        let (go, po) = (group.at.origin, parent.at.origin);
        let gbox = [
            go[0] as f32 - po[0] as f32,
            go[1] as f32 - po[1] as f32,
            size.0 as f32,
            size.1 as f32,
        ];
        if let Some((quad, slots)) = self.with.quad.filter(|_| iso.quad()) {
            // Over the target it began in, as a quad over its own box: its opacity and clips in the shader.
            let onto = self.stage.view(parent.target);
            quad.draw(
                self.encoder,
                onto,
                (slots, group.slot),
                self.stage.input_group(g),
            );
            self.stage.free_target(g);
            self.lays += 1;
            crate::render::effects::count_pass([size.0 as f32, size.1 as f32]);
            return Some(Laid {
                target: parent.target,
                hosted: false,
            });
        }
        // Laid over the target it began in by a host pass of that target's size.
        let lay = c.lay(&Lay {
            gbox,
            porigin: po.map(|v| v as f32),
            ..iso.lay
        });
        let made = self.run(Input::Pair(parent.target, g), parent.at.size, &lay);
        made.flatten().map(|target| Laid {
            target,
            hosted: true,
        })
    }

    /// One pass over `input` at `size`: its result, or None when it couldn't run (the stage said why).
    fn run(
        &mut self,
        input: Input<'_>,
        size: (u32, u32),
        pass: &crate::plan::EffectPass,
    ) -> Option<Option<usize>> {
        let one = std::slice::from_ref(pass);
        let (device, queue, info) = (self.device, self.queue, self.with.info);
        let made = self.stage.apply(
            device,
            queue,
            self.encoder,
            input,
            size,
            one,
            info,
            self.warnings,
        );
        made.ok()
    }
}
