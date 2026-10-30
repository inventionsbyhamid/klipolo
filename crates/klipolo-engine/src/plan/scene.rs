//! Scene layers: a generator layer whose source is a motion graphic (`scene@1`) plans
//! a scene layer, which the compositor draws into the layer's own target at scene time and places like any layer. Its
//! motion, effects, blend and padding come exactly as a generator layer's do (`overlay.rs`); its settings are shared
//! while they don't change (`strings::Table::scene_inputs`), with their hash for the drawer's cache. A verified scene's
//! settings are held inside what its verdict measured (`inputs`), as a shader's are, and kept so.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use klipolo_model::contrib::{ContribId, ContribKind};
use klipolo_model::layout::BoxN;
use klipolo_model::motion::Blend;
use klipolo_model::params::ParamValue;
use klipolo_model::project::Project;
use klipolo_model::scene::segments::{piece_scene_time, settled_time};
use klipolo_model::scene::timeline::CompiledScene;
use klipolo_model::time::Us;
use klipolo_model::timeline::PieceTime;

use super::EffectPass;
use super::effects;
use super::layers::MissingContrib;
use super::strings::{SceneInputs, Table};
use crate::registry::{Registry, ShaderEffect};

/// A motion graphic drawn in `dest`, placed like an image.
#[derive(Clone)]
pub struct SceneLayer {
    pub id: Arc<str>,
    pub dest: BoxN,
    pub rotation: f64,
    pub opacity: f64,
    pub radius: f64,
    pub effects: Vec<EffectPass>,
    pub blend: Blend,
    /// As `VideoLayer::reach`: its target is sized once for the largest box its motion reaches.
    pub reach: f64,
    /// As `VideoLayer::pad_px` (its `effects`').
    pub pad_px: f64,
    pub scene: Arc<CompiledScene>,
    /// The scene's contribution id.
    pub contrib: Arc<str>,
    pub params: Arc<BTreeMap<String, ParamValue>>,
    /// FNV-1a of the settings' canonical JSON: the drawer's cache key.
    pub params_hash: u64,
    /// Scene seconds.
    pub t: f64,
    /// The files its image settings name, by asset id (`image` nodes draw them).
    pub images: Arc<[(Arc<str>, Arc<str>)]>,
    /// One of its generator fills animates, so no stretch of it is ever still.
    pub fills_animate: bool,
    /// The text language its Chinese, Japanese and Korean follow (the context's `text_language`).
    pub language: Arc<str>,
    /// The largest scale its motion reaches over the layer's timeline (`motion::reach`, 1 without motion): its text is
    /// laid out once for it, never for this frame's scale.
    pub motion_reach: f64,
    /// The shaders its generator fills draw with: the registry's, only for a built-in or a generator the one
    /// runnable predicate let through, by the scene's `generator_fills` order.
    pub fills: SceneFills,
    /// The built-in blur its groups, backdrops and shadows blur with.
    pub blur: Option<Arc<ShaderEffect>>,
    /// It has a backdrop node: the compositor grabs the frame under it before drawing it.
    pub backdrop: bool,
    /// Its motion can turn it (a spin or wiggle loop, or rotation keys that change): a backdrop's grab then keeps the
    /// square that holds every turn (`motion::turns`).
    pub turns: bool,
}

/// The most generators one scene's shapes are filled with that draw (the rest draw without their fill).
pub const MAX_SCENE_FILLS: usize = 8;

/// A scene layer's generator fills' shaders, by the scene's `generator_fills` order (None: not drawn here). Fixed size,
/// so planning a frame allocates nothing; compared by identity.
#[derive(Clone, Default)]
pub struct SceneFills([Option<Arc<ShaderEffect>>; MAX_SCENE_FILLS]);

impl SceneFills {
    /// Fill `k`'s shader, if it draws.
    pub fn get(&self, k: usize) -> Option<&Arc<ShaderEffect>> {
        self.0.get(k)?.as_ref()
    }

    /// `scene`'s generator fills as `registry` draws them: each a generator shader there, else None and noted as being
    /// checked (a library generator this computer hasn't checked yet) or missing.
    pub(super) fn of(
        scene: &CompiledScene,
        registry: &Registry,
        s: &mut Table,
        missing: &mut Vec<MissingContrib>,
    ) -> Self {
        let mut out = Self::default();
        for (slot, id) in out.0.iter_mut().zip(&scene.facts.generator_fills) {
            *slot = registry
                .shader(id)
                .filter(|sh| sh.kind == crate::registry::ShaderKind::Generator)
                .cloned();
            if slot.is_none() {
                effects::note_not_drawn(registry, missing, ContribKind::Generator, id, s);
            }
        }
        out
    }
}

impl PartialEq for SceneFills {
    fn eq(&self, other: &Self) -> bool {
        self.0.iter().zip(&other.0).all(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        })
    }
}

impl fmt::Debug for SceneFills {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|s| s.as_ref().map(|s| s.id.as_str())))
            .finish()
    }
}

impl PartialEq for SceneLayer {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.dest == other.dest
            && self.rotation == other.rotation
            && self.opacity == other.opacity
            && self.radius == other.radius
            && self.effects == other.effects
            && self.blend == other.blend
            && self.reach == other.reach
            && self.pad_px == other.pad_px
            && Arc::ptr_eq(&self.scene, &other.scene)
            && self.contrib == other.contrib
            && self.params_hash == other.params_hash
            && self.t == other.t
            && self.images == other.images
            && self.fills_animate == other.fills_animate
            && self.language == other.language
            && self.motion_reach == other.motion_reach
            && self.fills == other.fills
            && self.blur.as_ref().map(Arc::as_ptr) == other.blur.as_ref().map(Arc::as_ptr)
            && self.backdrop == other.backdrop
            && self.turns == other.turns
    }
}

impl fmt::Debug for SceneLayer {
    // The compiled scene is left out: it is the contribution's, and long.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SceneLayer")
            .field("id", &self.id)
            .field("contrib", &self.contrib)
            .field("dest", &self.dest)
            .field("t", &self.t)
            .field("opacity", &self.opacity)
            .field("effects", &self.effects.len())
            .field("params_hash", &self.params_hash)
            .finish_non_exhaustive()
    }
}

/// Where `scene` is at `local_us` into a layer piece: its settled pose for a still layer (the end of `in`, or without
/// segments its hold point, before any exit: never a fade-out's blank end), else its scene time on the piece's span.
pub fn scene_time(scene: &CompiledScene, piece: PieceTime, local_us: Us, still: bool) -> f64 {
    if still {
        return match scene.segments {
            Some(_) => settled_time(scene.duration, scene.segments),
            None => scene.hold,
        };
    }
    piece_scene_time(piece, local_us, scene.duration, scene.segments)
}

/// Layer `item_id`'s settings `values` of scene `source`, held inside what its verdict measured (m4: its costly ones,
/// `scene_check::costly`), as `effects::clamped` holds a shader's, and kept between frames (`Table::scene_inputs`). A
/// layer that had to be held is noted as being checked; the app measures it again.
pub(super) fn inputs(
    s: &mut Table,
    item_id: &str,
    (source, scene): (&ContribId, &CompiledScene),
    values: &BTreeMap<String, ParamValue>,
    project: &Project,
    registry: &Registry,
    missing: &mut Vec<MissingContrib>,
) -> SceneInputs {
    let envelope = registry.scene_envelope(source);
    let inputs = s.scene_inputs(item_id, &scene.params, values, envelope, project, registry);
    if inputs.clamped {
        effects::note_checking(missing, ContribKind::Scene, s.str(source.as_str()));
    }
    inputs
}

/// FNV-1a of `params`' canonical JSON (a `BTreeMap` serialises its keys in order).
pub fn params_hash(params: &BTreeMap<String, ParamValue>) -> u64 {
    let json = serde_json::to_vec(params).unwrap_or_default();
    crate::registry::fnv1a64(&json)
}
