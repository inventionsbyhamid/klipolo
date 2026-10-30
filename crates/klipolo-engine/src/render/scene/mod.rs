//! The scene drawer: a motion graphic drawn into its generator layer's target.
//!
//! A scene layer's instance (settings resolved, repeats expanded) is built once per (layer, compiled scene, settings,
//! pictures) and kept in a bounded cache (`cache.rs`), ideally ahead of its first frame by the playback lookahead
//! (`prepare`), with its path meshes (`meshes.rs`, from the shared bounded `mesh_cache.rs`); drawn at another size it
//! keeps its instance. Each frame evaluates it (allocation-free), packs its shapes and path draws (`pack.rs`) and, only
//! when they changed, writes them to its buffers; one pass draws them into a pooled target (`target.rs`) with two
//! pipelines, SDF shapes and paths. While the scene holds still (`static_until`) that target is kept and drawn again
//! with no pass (never with a backdrop, an animating fill or live particles). Past the runtime caps it draws what
//! the evaluator kept, its still when it would read too many tracks, and leaves out a path too detailed to draw, and
//! says so once per scene (`said.rs`). Text is laid out once per instance and size (`text_cache.rs`) and
//! drawn by a third pipeline from its atlas (`text.rs` and on). Nodes are drawn offscreen where they need it
//! (groups with blur, a blend, a mask, a tilt or opacity; a text's shadow…: `groups.rs`, `masks.rs`, laid back by the
//! effect stage's host passes in `composite.rs`, encoded by `encode.rs` and `encode_lay.rs`), generator fills
//! (`fills.rs`), backdrops (`backdrop.rs`) and particles (`emitter.rs`), and counts what an instance holds at once
//! (`SceneBytes`).

use std::sync::Arc;

use tokio::sync::Notify;

mod backdrop;
mod bounds;
mod build;
mod cache;
mod composite;
mod draw;
mod draw_frame;
mod draws;
mod emitter;
mod encode;
mod encode_lay;
mod fills;
mod fringe;
mod geometry;
pub mod glyph_sets;
mod gradients;
mod groups;
mod masks;
mod mesh_cache;
mod meshes;
mod motion;
mod node_paths;
mod pack;
mod pack_node;
mod pack_room;
mod path_pipeline;
mod paths;
mod pictures;
mod place;
mod quad_lay;
mod said;
mod shape_pack;
mod shapes;
mod sizes;
mod sizes_built;
mod slots;
mod statics;
mod stencils;
mod stroke;
mod target;
pub mod text;
mod text_atlas;
#[cfg(test)]
mod text_atlas_tests;
mod text_backgrounds;
mod text_blit;
mod text_blur;
mod text_blur_pass;
mod text_builds;
mod text_cache;
mod text_caps;
mod text_counts;
mod text_ensure;
mod text_fit;
mod text_font;
mod text_gpu;
mod text_layouts;
mod text_move;
mod text_nodes;
mod text_pack;
mod text_paint;
mod text_pipeline;
#[cfg(any(test, feature = "test-support"))]
mod text_probe;
mod text_shadow;
mod text_shadow_blur;
mod text_shape;
mod text_units;
mod text_uses;
mod text_worker;
mod types;

pub use backdrop::Backdrop;
pub use cache::{MAX_SCENE_BYTES, MAX_SCENES, reset_scene_bytes_peak, scene_bytes_peak};
pub use groups::{MAX_GROUPS, MAX_OTHERS};
pub use mesh_cache::{MAX_MESH_BYTES, MAX_MESH_VERTICES, mesh_bytes_peak};
pub use shapes::{MAX_GRADIENTS, MAX_STOPS};
#[cfg(any(test, feature = "test-support"))]
pub use stencils::count_stencil_pixels_at;
pub use text_cache::{MAX_IN_FLIGHT, MAX_TEXT_ATLAS_BYTES, MAX_TEXT_SCENES, TextAt};
pub use text_counts::{
    text_builds_on_other_threads, text_layout_calls, text_layout_calls_on_this_thread,
};
pub(crate) use text_ensure::drawn_k;
#[cfg(any(test, feature = "test-support"))]
pub use text_probe::TextProbe;
pub use text_uses::{TextFontUse, setting_faces, text_font_uses};
pub(crate) use types::Drawn;
pub use types::{SCENE_OWN_BYTES, SceneAt, SceneBytes, SceneOut, SceneSplit, SceneStats};

use crate::render::effects::EffectStage;
use cache::SceneCache;
use composite::Composites;
use mesh_cache::MeshCache;
use path_pipeline::PathPipeline;
use pictures::Pictures;
use quad_lay::QuadLay;
use shapes::ShapePipeline;
use stencils::Stencils;
use text_cache::TextCache;
use text_pipeline::TextPipeline;

/// The drawer: its pipeline (made at warm-up), its cache and what it said.
#[derive(Default)]
pub struct SceneDrawer {
    pipeline: Option<ShapePipeline>,
    paths: Option<PathPipeline>,
    cache: SceneCache,
    meshes: MeshCache,
    frame: u64,
    passes: u32,
    stats: SceneStats,
    /// Each scene's degrades already said, until the project closes: (contribution, code).
    said: Vec<(Arc<str>, &'static str)>,
    /// The textures the layer drawn next decoded its pictures to, and their bind groups (kept between frames).
    textures: Vec<Option<wgpu::Texture>>,
    pictures: Pictures,
    /// The scene pass's stencils, one per target size (kept for every layer on screen).
    stencils: Stencils,
    /// The text pipeline (made at warm-up) and each instance's text at each size.
    text: Option<TextPipeline>,
    texts: TextCache,
    /// Fired by the text build thread when a build lands, so the preview draws it.
    redraw: Option<Arc<Notify>>,
    /// The fonts' generation and whether the system's were scanned, as the text was last built for them.
    fonts_seen: (u64, bool),
    /// The host passes offscreen nodes are laid back with, and the frame's generator fills' and backdrops'
    /// targets and bind groups (their room kept between frames).
    composites: Option<Composites>,
    fill_targets: Vec<Option<usize>>,
    backdrop_targets: Vec<Option<usize>>,
    fill_groups: Vec<Option<wgpu::BindGroup>>,
    backdrop_groups: Vec<Option<wgpu::BindGroup>>,
    /// The quad lay-back, made with the host passes.
    quad: Option<QuadLay>,
    /// The offscreen targets and text this frame's scenes drew: in the bytes peak.
    frame_extra: u64,
}

impl SceneDrawer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A drawer whose text build thread fires `redraw` when a build lands (the compositor's).
    pub fn with_redraw(redraw: Arc<Notify>) -> Self {
        Self {
            redraw: Some(redraw),
            ..Self::default()
        }
    }

    /// Compiles the scene pipelines now (`shapes.wgsl`, `paths.wgsl`, `text.wgsl`), so a first scene never compiles
    /// on the render thread.
    pub fn warm(&mut self, device: &wgpu::Device) {
        let shapes = self
            .pipeline
            .get_or_insert_with(|| ShapePipeline::new(device));
        self.paths
            .get_or_insert_with(|| PathPipeline::new(device, &shapes.layout));
        self.text
            .get_or_insert_with(|| TextPipeline::new(device, &shapes.layout));
    }

    /// Compiles the host passes offscreen nodes are laid back with in `stage` now, with `warm`, and the
    /// quad lay-back.
    pub fn warm_passes(&mut self, device: &wgpu::Device, stage: &mut EffectStage) {
        let passes = self.composites.get_or_insert_with(Composites::new);
        stage.warm_also(&passes.all().map(|s| &**s));
        self.quad
            .get_or_insert_with(|| QuadLay::new(device, stage.input_layout()));
    }

    /// Its host passes' pipelines to the compile thread, as a scene comes up.
    pub fn queue_passes(&mut self, stage: &mut EffectStage) {
        let passes = self.composites.get_or_insert_with(Composites::new);
        stage.queue_also(&passes.all());
    }

    /// The fonts changed (the scan finished, a font was registered): every scene's text is built again on the text
    /// build thread, the old drawn until the new lands.
    pub fn fonts_changed(&mut self) {
        self.texts.epoch += 1;
    }

    /// The frame's start: instances unused for a while go, their held targets back to `stage`; text builds that landed
    /// are uploaded and kept now, before any frame uses an entry (none waits in the
    /// channel while no scene draws).
    pub fn begin_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        stage: &mut EffectStage,
    ) {
        self.frame += 1;
        self.passes = 0;
        self.frame_extra = 0;
        self.cache.expire(self.frame, stage);
        self.pictures.expire(self.frame);
        self.stencils.expire(self.frame);
        self.texts.expire(self.frame);
        if let Some(tp) = self.text.as_ref() {
            // What landing costs this thread now its atlas comes uploaded.
            let started = std::time::Instant::now();
            if self.texts.collect((device, queue, tp), self.frame) > 0 {
                self.stats.land_us = started.elapsed().as_secs_f64() * 1e6;
            }
        }
        let fonts = (
            crate::render::fonts::generation(),
            crate::vendor::cap::rendering::layers::fonts_scanned(),
        );
        if fonts != self.fonts_seen {
            self.fonts_seen = fonts;
            self.fonts_changed();
        }
        self.cache.count_stencils(self.stencils.bytes());
    }

    /// Drops every instance (project closed, window hidden, device lost); the pipeline stays.
    pub fn forget(&mut self) {
        self.cache.clear();
        self.meshes.clear();
        self.said.clear();
        self.textures.clear();
        self.pictures.clear();
        self.stencils.clear();
        self.texts.clear();
    }

    /// Scene passes drawn since this frame began.
    pub fn passes(&self) -> u32 {
        self.passes
    }

    pub fn stats(&self) -> SceneStats {
        SceneStats {
            cached: self.cache.len(),
            meshes: self.meshes.len(),
            mesh_bytes: self.meshes.bytes(),
            meshes_made: self.meshes.made(),
            stencil_bytes: self.stencils.bytes(),
            texts: self.texts.len(),
            text_bytes: self.texts.bytes(),
            text_pending: self.texts.pending.len(),
            text_pending_waiting: self.texts.pending_waiting(),
            ..self.stats
        }
    }

    /// Instances kept.
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// The textures the compositor decoded a layer's images to, filled before `draw` (their room is kept).
    pub fn textures(&mut self) -> &mut Vec<Option<wgpu::Texture>> {
        &mut self.textures
    }

    /// The part of a target of `size` (x, y, w, h) that `layer`'s backdrop nodes read of the frame under it, the most
    /// they read across its timeline: what the compositor grabs for it, once the instance was surveyed for
    /// that size; None before, or without a backdrop (the compositor grabs the layer's whole box).
    pub fn backdrop_box(
        &self,
        layer: &crate::plan::SceneLayer,
        size: (u32, u32),
    ) -> Option<[u32; 4]> {
        self.cache.peek(layer)?.sizes.backdrop(size)
    }
}
