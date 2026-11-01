//! Building a scene layer's instance (with its path meshes and its text): ahead of its first
//! frame from the playback lookahead (`prepare`), else on the frame that needs it.

use klipolo_model::scene::instance::SceneInstance;

use super::cache::Entry;
use super::fills::FillPass;
use super::meshes::{Meshes, Source};
use super::pack::Room;
use super::path_pipeline::{PathBuffers, PathPipeline};
use super::shapes::{ShapeBuffers, ShapePipeline};
use super::statics::NodeStatic;
use super::text_cache::TextAt;
use super::text_ensure::ensure;
use super::text_pipeline::TextPipeline;
use super::{Drawn, SceneAt, SceneDrawer, statics};
use crate::plan::SceneLayer;
use crate::render::effects::EffectStage;

/// What building an instance makes: the instance, its static part, its shapes' buffers, its path meshes and its path
/// draws' buffer.
pub(super) struct Built {
    pub inst: SceneInstance,
    pub statics: Vec<NodeStatic>,
    pub spans: Vec<i32>,
    pub gpu: ShapeBuffers,
    pub meshes: Meshes,
    pub paths: PathBuffers,
    pub fills: Vec<FillPass>,
}

impl SceneDrawer {
    /// Builds `layer`'s instance for `at` (its buffers, static part and path meshes for `px` target pixels per 1080p
    /// pixel with it, and its text asked of the text build thread as `text` says) unless it is built: off the drawing
    /// path, from the playback lookahead for a scene layer about to appear.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        stage: &mut EffectStage,
        text: TextAt,
        layer: &SceneLayer,
        at: SceneAt,
        px: f32,
    ) {
        if self.cache.find(layer, self.frame).is_none() {
            self.build(device, queue, stage, text, layer, at, px);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        stage: &mut EffectStage,
        text: TextAt,
        layer: &SceneLayer,
        at: SceneAt,
        px: f32,
    ) -> usize {
        self.stats.builds += 1;
        let pipeline = &*self
            .pipeline
            .get_or_insert_with(|| ShapePipeline::new(device));
        let paths = &*self
            .paths
            .get_or_insert_with(|| PathPipeline::new(device, &pipeline.layout));
        let mut inst = SceneInstance::new(&layer.scene, &layer.params);
        inst.set_fills_animate(layer.fills_animate);
        let (mut statics, gradients, spans) = statics::statics(&inst, &layer.images);
        super::fills::mark_undrawn(&mut statics, &layer.fills);
        // Its generator fills' passes, their settings resolved with the instance.
        let seed = crate::registry::fnv1a64(layer.id.as_bytes()) as u32;
        let fills = super::fills::passes(&inst, &statics, &layer.fills, seed);
        let mut src = Source {
            device,
            cache: &mut self.meshes,
            now: self.frame,
        };
        let drawn = Drawn {
            size: at.size,
            k: f64::from(px),
        };
        let meshes = Meshes::build(&mut src, &inst, &statics, drawn);
        // Room for a frame's shapes and path draws: a mask's node drawn again in its segment, each live particle's.
        let room = Room::of(&inst, &statics, &meshes);
        let again = if room.masks { 2 } else { 1 };
        let shapes = inst.len() * again + room.particles;
        let gpu = pipeline.buffers(device, queue, shapes, &gradients);
        let particle_paths = if room.path_particles {
            room.particles
        } else {
            0
        };
        let paths = paths.buffers(device, 2 * (meshes.count() * again + particle_paths));
        let built = Built {
            inst,
            statics,
            spans,
            gpu,
            meshes,
            paths,
            fills,
        };
        let i = self
            .cache
            .insert(Entry::new(layer, at, built), self.frame, stage);
        // Its text asked of the text build thread now too, never waited for here.
        let tp = &*self
            .text
            .get_or_insert_with(|| TextPipeline::new(device, &pipeline.layout));
        let entry = self.cache.get_mut(i);
        let ask = TextAt {
            block: false,
            ..text
        };
        let redraw = self.redraw.as_ref();
        let _ = ensure(
            &mut self.texts,
            (device, queue, tp),
            layer,
            entry,
            at,
            ask,
            redraw,
            self.frame,
        );
        // A moving scene without text asks for its survey alone.
        self.survey_off(i, layer, (at, f64::from(px)), (device, queue));
        i
    }
}
