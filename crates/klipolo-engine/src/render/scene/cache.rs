//! The scene drawer's cache: per (layer id, compiled scene, settings hash, pictures), one built instance
//! (its settings resolved and repeats expanded), its reused frame, its static part, its packing room, its GPU buffers
//! and its path meshes (held from the shared mesh cache), and the target it last drew while the scene holds
//! still (hold caching). The size the layer is drawn at
//! isn't in the key: another size rewrites the target size and drops the hold, never rebuilds; the
//! compiled scene is compared by identity, so new bytes under one id (a pending draft) are a new instance.
//!
//! **Bounded** (CPU and GPU memory together): at most [`MAX_SCENES`] entries and [`MAX_SCENE_BYTES`]
//! (the scene pass's stencils have their own bound, `stencils.rs`, and only join the peak);
//! past either, entries go in [`evict_rank`] order: drawn ones no longer on screen first, then prepared ones, never one
//! this frame used. A drawn entry goes after [`EXPIRE_AFTER_FRAMES`] unused frames; one the lookahead prepared waits for
//! its first frame, at most [`PREPARED_FRAMES`] (one that never shows, playback
//! stopped or sought away, is let go). Everything goes on `forget` (project closed, device lost); a held target goes
//! back to the effect stage's pool with its entry. A lookup is O(entries) and allocates nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use klipolo_model::scene::eval::SceneFrame;
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::timeline::CompiledScene;

use super::SceneAt;
use super::build::Built;
use super::fills::FillPass;
use super::meshes::Meshes;
use super::pack::{Packed, Room};
use super::path_pipeline::PathBuffers;
use super::shapes::ShapeBuffers;
use super::sizes::Sizes;
use super::sizes_built::SurveyAsk;
use super::statics::{Look, NodeStatic};
use crate::plan::{SceneFills, SceneLayer};
use crate::render::effects::EffectStage;

/// The most scene instances kept at once.
pub const MAX_SCENES: usize = 8;
/// The most bytes they may hold together, CPU and GPU (a 1,000-node scene's are about 2 MiB).
pub const MAX_SCENE_BYTES: u64 = 32 << 20;
/// An instance drawn and then unused for this many frames goes.
const EXPIRE_AFTER_FRAMES: u64 = 8;
/// An instance the lookahead prepared waits this many frames for its first: the lookahead looks 1 s ahead, so four
/// times that at 120 fps.
const PREPARED_FRAMES: u64 = 480;

/// The order entries go in when room is needed (the smallest first): drawn ones off screen (not drawn last frame), the
/// oldest first; then prepared ones, the oldest first; then those on screen.
fn evict_rank(drawn: bool, used: u64, now: u64) -> (u8, u64) {
    let class = match (drawn, now.saturating_sub(used) <= 1) {
        (true, false) => 0,
        (false, _) => 1,
        (true, true) => 2,
    };
    (class, used)
}

/// The most bytes scene instances have held at once in this process (`scene_bytes_peak`).
static PEAK: AtomicU64 = AtomicU64::new(0);

/// The most bytes every scene drawer's instances and stencils have held at once in this process, CPU and GPU, with the
/// offscreen targets and the text the frame's scenes drew (`SceneBytes`; a diagnostic, like
/// `effect_target_peak`; a held target counts in the effect stage's pool instead).
pub fn scene_bytes_peak() -> u64 {
    PEAK.load(Ordering::Relaxed)
}

/// Starts [`scene_bytes_peak`] again (a test measuring one scene).
#[doc(hidden)]
pub fn reset_scene_bytes_peak() {
    PEAK.store(0, Ordering::Relaxed);
}

/// The target holding an entry's last frame while the scene is still: its id in the effect stage, the scene time it
/// shows, when the scene next changes after it, and where it sits in the layer's target (`SceneOut::rect`).
#[derive(Clone, Copy, Debug)]
pub(super) struct Held {
    pub target: u64,
    pub t: f64,
    pub until: f64,
    pub rect: [u32; 4],
}

/// One built scene instance.
pub(super) struct Entry {
    layer: Arc<str>,
    scene: Arc<CompiledScene>,
    params: u64,
    images: Arc<[(Arc<str>, Arc<str>)]>,
    /// Where it was last drawn (or prepared for): its target's size.
    pub at: SceneAt,
    /// Drawn at least once (an entry the lookahead prepared isn't, and doesn't age until it is).
    pub drawn: bool,
    /// Its bytes, CPU and GPU, counted when it is built.
    bytes: u64,
    pub inst: SceneInstance,
    pub frame: SceneFrame,
    pub statics: Vec<NodeStatic>,
    pub packed: Packed,
    pub gpu: ShapeBuffers,
    /// Its path meshes (the shared cache's) and its path draws' buffer.
    pub meshes: Meshes,
    pub paths: PathBuffers,
    pub held: Option<Held>,
    /// It draws text: its text at each size is the text cache's (`text_cache.rs`).
    pub has_text: bool,
    /// The text entry whose units it took last (0: none yet).
    pub text_id: u64,
    /// Its generator fills' passes, the plan's shaders they were made with, and its spans' gradients.
    pub fills: Vec<FillPass>,
    fill_key: SceneFills,
    pub spans: Vec<i32>,
    /// Its offscreen nodes' target sizes and its own, and its slots bound for the quad lay-back.
    pub sizes: Sizes,
    /// It holds one pose its whole timeline: its classes are its first frame's boxes, never surveyed.
    pub still: bool,
    /// Its survey asked of the text build thread alone (`sizes_built.rs`).
    pub survey_ask: SurveyAsk,
    pub quad: Option<wgpu::BindGroup>,
    /// The pool's id and size of the target it drew its last frame into: one it outgrew is
    /// let go before the next is made, so it never holds two of its own.
    pub own_target: Option<(u64, (u32, u32))>,
    used: u64,
}

impl Entry {
    /// An entry for `layer` drawn `at`.
    pub(super) fn new(layer: &SceneLayer, at: SceneAt, built: Built) -> Self {
        let Built {
            inst,
            statics,
            spans,
            gpu,
            meshes,
            paths,
            fills,
        } = built;
        let frame = SceneFrame::for_instance(&inst);
        let has_text = statics
            .iter()
            .any(|s| matches!(s.look, Look::Text | Look::Emoji));
        let packed = Packed::with_room(Room::of(&inst, &statics, &meshes));
        let sizes = Sizes::with_room(inst.len());
        let duration = inst.compiled().duration;
        let still = klipolo_model::scene::eval::static_until(&inst, 0.0) >= duration;
        let survey_ask = SurveyAsk::of(has_text, still, &statics);
        let bytes = gpu.bytes()
            + gpu.cpu_bytes()
            + paths.bytes()
            + (inst.approx_bytes()
                + frame.approx_bytes()
                + packed.bytes()
                + meshes.bytes()
                + sizes.bytes()
                + statics.capacity() * std::mem::size_of::<NodeStatic>()) as u64;
        Self {
            layer: layer.id.clone(),
            scene: layer.scene.clone(),
            params: layer.params_hash,
            images: layer.images.clone(),
            at,
            drawn: false,
            bytes,
            frame,
            packed,
            inst,
            statics,
            gpu,
            meshes,
            paths,
            held: None,
            has_text,
            text_id: 0,
            fills,
            fill_key: layer.fills.clone(),
            spans,
            sizes,
            survey_ask,
            still,
            quad: None,
            own_target: None,
            used: 0,
        }
    }

    fn is(&self, layer: &SceneLayer) -> bool {
        self.params == layer.params_hash
            && Arc::ptr_eq(&self.scene, &layer.scene)
            && *self.layer == *layer.id
            && self.images == layer.images
            && self.fill_key == layer.fills
    }

    /// Its buffers' bytes, CPU and GPU (`SceneBytes::buffers`, less the stencil).
    pub(super) fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[derive(Default)]
pub(super) struct SceneCache {
    entries: Vec<Entry>,
    /// The bytes the scene pass's stencils hold (`stencils.rs`): in the peak, not in the entries' budget.
    stencils: u64,
}

impl SceneCache {
    /// The entry for `layer`, marked used at `now`.
    pub(super) fn find(&mut self, layer: &SceneLayer, now: u64) -> Option<usize> {
        let i = self.entries.iter().position(|e| e.is(layer))?;
        self.entries[i].used = now;
        Some(i)
    }

    pub(super) fn get_mut(&mut self, i: usize) -> &mut Entry {
        &mut self.entries[i]
    }

    /// The entry for `layer`, if built, left as it was used.
    pub(super) fn peek(&self, layer: &SceneLayer) -> Option<&Entry> {
        self.entries.iter().find(|e| e.is(layer))
    }

    /// Keeps `entry` (used at `now`), first making room: past [`MAX_SCENES`] entries or [`MAX_SCENE_BYTES`], entries
    /// go in [`evict_rank`] order (never one this frame used), their held targets back to `stage`'s pool. Its index.
    pub(super) fn insert(&mut self, mut entry: Entry, now: u64, stage: &mut EffectStage) -> usize {
        entry.used = now;
        let adding = entry.bytes;
        while self.entries.len() >= MAX_SCENES || self.entry_bytes() + adding > MAX_SCENE_BYTES {
            let Some(oldest) = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.used < now)
                .min_by_key(|(_, e)| evict_rank(e.drawn, e.used, now))
                .map(|(i, _)| i)
            else {
                break;
            };
            let gone = self.entries.swap_remove(oldest);
            release(&gone, stage);
        }
        self.entries.push(entry);
        PEAK.fetch_max(self.bytes(), Ordering::Relaxed);
        self.entries.len() - 1
    }

    /// The frame's start: drawn entries unused for more than [`EXPIRE_AFTER_FRAMES`] go; prepared ones wait for their
    /// first frame at most [`PREPARED_FRAMES`].
    pub(super) fn expire(&mut self, now: u64, stage: &mut EffectStage) {
        self.entries.retain(|e| {
            let wait = if e.drawn {
                EXPIRE_AFTER_FRAMES
            } else {
                PREPARED_FRAMES
            };
            let keep = now - e.used <= wait;
            if !keep {
                release(e, stage);
            }
            keep
        });
    }

    /// Every entry goes (the stage forgets its targets itself), and the stencils with them.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.stencils = 0;
    }

    /// The stencils now hold `bytes`.
    pub(super) fn count_stencils(&mut self, bytes: u64) {
        self.stencils = bytes;
        PEAK.fetch_max(self.bytes(), Ordering::Relaxed);
    }

    /// This frame's scenes also drew `extra` bytes of offscreen targets and text: into the peak.
    pub(super) fn count_frame(&self, extra: u64) {
        PEAK.fetch_max(self.bytes() + extra, Ordering::Relaxed);
    }

    /// The bytes the entries hold, CPU and GPU: what their budget counts.
    fn entry_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes).sum()
    }

    /// The bytes the entries and the stencils hold, CPU and GPU (the peak).
    pub(super) fn bytes(&self) -> u64 {
        self.entry_bytes() + self.stencils
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// `entry`'s held target back to the pool.
fn release(entry: &Entry, stage: &mut EffectStage) {
    if let Some(h) = entry.held {
        stage.release_held(h.target);
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
