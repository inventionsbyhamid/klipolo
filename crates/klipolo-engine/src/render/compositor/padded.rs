//! Layers whose filters draw past their box (padding). The passes before the first padded
//! filter run at the layer's own size; from it on they run in a target grown by the margin, sized once for
//! the largest box the layer's motion reaches and clipped to the frame, and the grown quad places the
//! result. An image, video or the main footage enters the grown target through the effect stage's PAD pass, which
//! stands it upright and draws its corners; a square generator whose first filter is the padded one draws straight
//! into it, and a text's texture already holds the margin (`render::text`), so neither needs an extra pass. A layer
//! without a margin left (none declared, or all of it outside the frame) draws exactly as before.

use super::frame::FrameCtx;
use super::staged::{EffectSize, effect_size, upright_turn};
use super::{Compositor, Draw};
use crate::plan::{EffectPass, GeneratorLayer};
use crate::render::effects::{FrameInfo, Input, Withheld};
use crate::render::uniforms::quad_for;
use crate::vendor::cap::rendering::layers::image::ImageQuad;

mod geometry;
pub(super) use geometry::text_content;
use geometry::{Plan, plan};

/// Where the passes from the first padded filter on start (None: no pass draws past the layer).
fn first_padded(passes: &[EffectPass]) -> Option<usize> {
    passes.iter().position(|p| p.pad_px > 0.0)
}

impl Compositor {
    /// [`Compositor::push_quad`] for an image or overlay video whose filters draw `pad_px` past its box (its motion
    /// reaching `reach` times this frame's box): the passes before the first padded filter at its own size, then PAD
    /// into the grown target and the rest there, placed by the grown quad. Exactly `push_quad` without a margin.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_padded(
        &mut self,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
        texture: &wgpu::Texture,
        at: EffectSize,
        passes: &[EffectPass],
        mut quad: ImageQuad,
        pad_px: f64,
        reach: f64,
    ) {
        let Some((k, plan)) =
            first_padded(passes).zip(plan(&quad, at, reach, pad_px, f.info.px, f.size))
        else {
            return self.push_quad(f, draws, texture, at, passes, quad);
        };
        let (before, from) = passes.split_at(k);
        // What the passes before it made (upright when they stood it up), or the texture as it is.
        let Ok(made) = self.own_passes(f, texture, at, before, &mut quad) else {
            return;
        };
        let as_is = made.map_or(texture, |t| self.effects.texture(t)).clone();
        let input = made.map_or(Input::Texture(texture), Input::Target);
        let turn = upright_turn(quad.source_rotation, from).unwrap_or(0);
        self.padded(f, draws, input, &as_is, &plan, turn, from, quad);
    }

    /// The main footage (or a scene, its motion reaching `reach` times this frame's box) drawn alone in `target` (at
    /// `at`) through its `passes`: as [`Compositor::push_padded`] when they draw `pad_px` past it, else run there and
    /// placed by `quad`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn through_padded(
        &mut self,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
        target: usize,
        at: EffectSize,
        passes: &[EffectPass],
        quad: ImageQuad,
        pad_px: f64,
        reach: f64,
    ) {
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        let planned = first_padded(passes).zip(plan(&quad, at, reach, pad_px, f.info.px, f.size));
        let before = planned.map_or(passes, |(k, _)| &passes[..k]);
        let Ok(made) = self.effects.apply(
            device,
            queue,
            f.encoder,
            Input::Target(target),
            at.size,
            before,
            at.info(f.info),
            f.warnings,
        ) else {
            return;
        };
        let made = made.unwrap_or(target);
        let texture = self.effects.texture(made).clone();
        match planned {
            Some((k, plan)) => {
                let input = Input::Target(made);
                self.padded(f, draws, input, &texture, &plan, 0, &passes[k..], quad);
            }
            None => self.place(f, draws, &texture, quad),
        }
    }

    /// `input` (`as_is` when no pass runs) into `plan`'s grown target by PAD (turning it `turn`° clockwise and drawing
    /// `quad`'s corners), through `passes` there, placed by the grown quad; nothing when its protection couldn't run
    /// (the stage said so).
    #[allow(clippy::too_many_arguments)]
    fn padded(
        &mut self,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
        input: Input<'_>,
        as_is: &wgpu::Texture,
        plan: &Plan,
        turn: u16,
        passes: &[EffectPass],
        quad: ImageQuad,
    ) {
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        let out = self.effects.apply_padded(
            device,
            queue,
            f.encoder,
            input,
            &plan.padding(turn, &quad),
            passes,
            plan.info(f.info),
            f.warnings,
        );
        match out {
            Ok(Some(out)) => {
                let texture = self.effects.texture(out).clone();
                // PAD stood it upright and drew its corners.
                let placed = ImageQuad {
                    source_rotation: 0,
                    rounding: 0.0,
                    ..plan.quad
                };
                self.place(f, draws, &texture, placed);
            }
            Ok(None) => self.place(f, draws, as_is, quad),
            Err(Withheld) => {}
        }
    }

    /// A generator's picture at its drawn size (half of it for a half-size one), through its own effects, placed
    /// like an image. Its own pass comes from the effect stage's cache when its picture repeats (`statics`: static, or
    /// within a step). With padded filters: a square generator whose first filter is the padded one draws straight
    /// into the grown target (its `uv` on its box); any other draws at its own size, its filters before the
    /// padded one there, and enters the grown target through PAD, which keeps its corners.
    pub(super) fn generator(
        &mut self,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
        g: &GeneratorLayer,
    ) {
        // A half-size generator's passes read `kl.resolution` 0.5 (their `kl.px` halves with their targets). One the
        // preview's sampler slowed down to half size draws so too.
        let half = g.half || self.effects.halved(&g.source.shader);
        let frame = f.info;
        if half {
            f.info.resolution *= 0.5;
        }
        self.generator_at(f, draws, g, half);
        f.info = frame;
    }

    fn generator_at(
        &mut self,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
        g: &GeneratorLayer,
        half: bool,
    ) {
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        let quad = ImageQuad {
            blend: g.blend,
            ..quad_for(g.dest, g.rotation, g.opacity, g.radius, f.size)
        };
        let at = effect_size(&quad, g.reach);
        let at = if half { at.halved() } else { at };
        let (local, info) = (at.size, at.info(f.info));
        let planned =
            first_padded(&g.effects).zip(plan(&quad, at, g.reach, g.pad_px, f.info.px, f.size));
        if let Some((0, plan)) = planned
            && quad.rounding <= 0.0
        {
            // Its own pass draws the layer itself: its own box is all of its viewport.
            let inner_info = FrameInfo {
                content: FrameInfo::WHOLE,
                ..plan.info(f.info)
            };
            let Some(drawn) = self.effects.generate(
                device,
                queue,
                f.encoder,
                &g.id,
                g.cache_key,
                &g.source,
                plan.padding.grown,
                Some(plan.padding.inner),
                inner_info,
                f.warnings,
            ) else {
                return;
            };
            let Ok(out) = self.effects.apply(
                device,
                queue,
                f.encoder,
                Input::Target(drawn),
                plan.padding.grown,
                &g.effects,
                plan.info(f.info),
                f.warnings,
            ) else {
                return;
            };
            let texture = self.effects.texture(out.unwrap_or(drawn)).clone();
            return self.place(f, draws, &texture, plan.quad);
        }
        let Some(drawn) = self.effects.generate(
            device,
            queue,
            f.encoder,
            &g.id,
            g.cache_key,
            &g.source,
            local,
            None,
            info,
            f.warnings,
        ) else {
            return;
        };
        let before = planned.map_or(&g.effects[..], |(k, _)| &g.effects[..k]);
        // Its protection couldn't run: not drawn.
        let Ok(made) = self.effects.apply(
            device,
            queue,
            f.encoder,
            Input::Target(drawn),
            local,
            before,
            info,
            f.warnings,
        ) else {
            return;
        };
        let made = made.unwrap_or(drawn);
        let texture = self.effects.texture(made).clone();
        match planned {
            Some((k, plan)) => {
                let input = Input::Target(made);
                let from = &g.effects[k..];
                self.padded(f, draws, input, &texture, &plan, 0, from, quad);
            }
            None => self.place(f, draws, &texture, quad),
        }
    }
}
