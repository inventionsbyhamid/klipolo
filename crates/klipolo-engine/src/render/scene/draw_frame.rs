//! A scene layer's frame drawn once packed: its blurring letters drawn small
//! (`text_blur_pass.rs`), its draws uploaded where they changed, its generator fills and backdrops' copies made, then
//! its passes into its own target (only as big as what it draws) and its offscreen nodes' (`encode.rs`), and
//! what it holds counted (`SceneBytes`); the target is held while the scene holds still.

use std::time::Instant;

use klipolo_model::scene::eval::static_until;

use super::backdrop::{self, Backdrop};
use super::cache::Held;
use super::composite::Composites;
use super::encode::{self, Enc, Tally};
use super::fills;
use super::quad_lay::QuadLay;
use super::sizes::Placed;
use super::text_blur_pass;
use super::text_pipeline::TextDraw;
use super::{SceneBytes, SceneDrawer, SceneOut, target};
use crate::plan::SceneLayer;
use crate::render::effects::{EffectStage, FrameInfo, pass_pixels};

/// A packed frame to draw: its instance (`entry`, an index into the cache) and text entry, its layer, its own target's
/// place in the layer's (`root`), the frame's info at the scene's pixels, the frame under it, whether its pictures are
/// all there (else it isn't held), and when its drawing started.
pub(super) struct Frame<'a> {
    pub entry: usize,
    pub text: Option<usize>,
    pub layer: &'a SceneLayer,
    pub root: Placed,
    pub info: FrameInfo,
    pub backdrop: Option<Backdrop>,
    pub ready: bool,
    pub started: Instant,
}

impl SceneDrawer {
    /// `f` drawn: its target, and where it sits in the layer's; None when the frame's budget has no target left.
    pub(super) fn draw_frame(
        &mut self,
        (device, queue, encoder): (&wgpu::Device, &wgpu::Queue, &mut wgpu::CommandEncoder),
        stage: &mut EffectStage,
        f: Frame,
        warnings: &mut Vec<String>,
    ) -> Option<SceneOut> {
        let (t, info, backdrop) = (f.layer.t, f.info, f.backdrop);
        let entry = self.cache.get_mut(f.entry);
        let (pipeline, paths) = (self.pipeline.as_ref()?, self.paths.as_ref()?);
        // Its blurring letters, drawn small before its passes and its draws are written: with no scratch
        // targets (the frame's budget), they draw sharp.
        let scratch = match (f.text, self.text.as_ref()) {
            (Some(k), Some(tp)) => self.texts.get_mut(k).gpu.as_mut().and_then(|gpu| {
                let budget = &entry.packed.blur;
                let gpu_ = (device, queue, &mut *encoder);
                text_blur_pass::run(gpu_, stage, tp, gpu, budget, warnings)
            }),
            _ => None,
        };
        if scratch.is_none() && entry.packed.blur.count() > 0 {
            sharpen(&mut entry.packed.texts);
        }
        let shapes = entry.gpu.upload(queue, &entry.packed.shapes) as u32;
        let draws = entry.paths.upload(queue, &entry.packed.draws) as u32;
        let (texts, written) = f.text.map_or((0, 0), |k| {
            let gpu = self.texts.get_mut(k).gpu.as_mut();
            let (draws, paints) = (&entry.packed.texts, &entry.packed.text_paints);
            gpu.map_or((0, 0), |g| g.upload(queue, draws, paints))
        });
        self.stats.text_written = written;
        let texts = texts as u32;
        // Its generator fills and backdrops' copies first: its passes read them. Its blurs are its own, the
        // built-in blur's twin that isn't protective.
        let gpu = (device, queue, &mut *encoder);
        let uses = &entry.packed.fill_uses;
        let fill_targets = &mut self.fill_targets;
        fills::run(
            gpu,
            stage,
            &mut entry.fills,
            uses,
            t,
            info,
            warnings,
            fill_targets,
        );
        let composites = self.composites.get_or_insert_with(Composites::new);
        let blur = f.layer.blur.as_ref().map(|b| composites.scene_blur(b));
        // What its passes hold of the pool from here, the frame grabbed under it with them (`EffectStage::watch`).
        stage.watch(backdrop.map(|b| b.target));
        let pixels = pass_pixels();
        let uses = &entry.packed.backdrop_uses;
        let gpu = (device, queue, &mut *encoder);
        let grabbed = &mut self.backdrop_targets;
        backdrop::run(
            gpu,
            stage,
            blur.as_ref(),
            backdrop,
            uses,
            info,
            warnings,
            grabbed,
        );
        let now = self.frame;
        let (fills, groups) = (&self.fill_targets, &mut self.fill_groups);
        self.pictures
            .bind_targets((device, pipeline), stage, now, fills, groups);
        let (from, to) = (&self.backdrop_targets, &mut self.backdrop_groups);
        self.pictures
            .bind_targets((device, pipeline), stage, now, from, to);
        let text = f.text.map(|k| self.texts.get(k));
        // A new size is made in the place of the target its last frame drew into, when nothing
        // holds that one now, so the scene holds one target of its own, never its first class beside the whole layer's.
        let size = crate::render::effects::target_size(f.root.size);
        let old = entry
            .own_target
            .filter(|(_, was)| *was != size)
            .map(|(id, _)| id);
        let Some(out) = stage.budgeted_target_replacing(device, f.root.size, warnings, old) else {
            entry.own_target = None;
            stage.watched();
            return None;
        };
        entry.own_target = Some((stage.id(out), size));
        // Where each of its targets sits (`slots.rs`), and the quad lay-back with its slots bound (made once).
        entry
            .gpu
            .slots
            .write_frame(queue, f.root, &entry.packed.isos);
        let quad = self
            .quad
            .get_or_insert_with(|| QuadLay::new(device, stage.input_layout()));
        let slots = &entry.gpu.slots;
        let quad_group = &*entry.quad.get_or_insert_with(|| quad.group(device, slots));
        let pass = target::Pass {
            shapes: pipeline,
            paths,
            gpu: &entry.gpu,
            draws: &entry.paths,
            meshes: &entry.meshes,
            pictures: &self.pictures.groups,
            text: self.text.as_ref().zip(text.and_then(|t| t.gpu.as_ref())),
            fills: &self.fill_groups,
            backdrops: &self.backdrop_groups,
        };
        let with = encode::With {
            pass: &pass,
            counts: (shapes, draws, texts),
            composites: self.composites.get_or_insert_with(Composites::new),
            blur: blur.as_ref(),
            root: f.root,
            info,
            quad: Some((&*quad, quad_group)),
        };
        let mut enc = Enc {
            device,
            queue,
            encoder: &mut *encoder,
            stage: &mut *stage,
            stencils: &mut self.stencils,
            now: self.frame,
            warnings: &mut *warnings,
            with: &with,
            tally: Tally::default(),
            lays: 0,
        };
        let e = encode::program(&mut enc, &entry.packed, out);
        let (tally, lays) = (enc.tally, enc.lays);
        let held = stage.watched();
        let out = e.target;
        self.passes += 1 + lays + scratch.map_or(0, |_| text_blur_pass::PASSES);
        let scratch = give_back(stage, scratch);
        self.stats.pass_pixels = pass_pixels() - pixels;
        self.stats.blur_fetches = entry.packed.blur.fetches();
        let rect = [
            f.root.origin[0],
            f.root.origin[1],
            f.root.size.0,
            f.root.size.1,
        ];
        let until = static_until(&entry.inst, t);
        if f.ready
            && until > t
            && let Some(id) = stage.hold(out)
        {
            entry.held = Some(Held {
                target: id,
                t,
                until,
                rect,
            });
        }
        // What it holds this frame: its own target, and the targets its
        // passes held of the pool, size by size, beyond it and the lay's partner (as big as it); its buffers and its
        // passes' stencils; its text; its letters' scratch and its fills at their own sizes.
        let own = u64::from(f.root.size.0) * u64::from(f.root.size.1) * 4;
        let partner = if e.laid { own } else { 0 };
        let bytes = SceneBytes {
            own,
            targets: held.saturating_sub(own + partner),
            buffers: entry.bytes() + tally.stencil_bytes(),
            atlas: text.map_or(0, |t| t.bytes),
            scratch: stage.pooled_bytes(&self.fill_targets) + scratch,
            partner,
        };
        self.stats.bytes = bytes;
        self.stats.offscreen = e.offscreen;
        self.frame_extra += bytes.targets + bytes.scratch + bytes.partner + bytes.atlas;
        self.cache.count_stencils(self.stencils.bytes());
        self.cache.count_frame(self.frame_extra);
        self.stats.cpu_us_last = f.started.elapsed().as_secs_f64() * 1e6;
        Some(SceneOut { target: out, rect })
    }
}

/// The scratch targets `text_blur_pass::run` handed out, given back to the pool once the scene has drawn (its passes
/// read them): the bytes they held.
fn give_back(stage: &mut EffectStage, scratch: Option<[usize; 2]>) -> u64 {
    let Some(targets) = scratch else {
        return 0;
    };
    let bytes = stage.pooled_bytes(&targets.map(Some));
    targets.into_iter().for_each(|t| stage.free_target(t));
    bytes
}

/// The frame's draws with no blur (its scratch targets refused): each blurring piece drawn sharp,
/// from the atlas, rather than from scratch targets that aren't there.
fn sharpen(draws: &mut [TextDraw]) {
    for d in draws.iter_mut().filter(|d| d.misc[3] > 0.0) {
        d.misc[2] = 0.0;
        d.misc[3] = 0.0;
    }
}
