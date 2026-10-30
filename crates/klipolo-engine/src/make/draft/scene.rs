//! A scene as a draft layer: the worker's own compile of the bytes it checked (`scene_check`), drawn as a
//! generator layer over the whole canvas by a registry holding it and the generators its fills name, for its own
//! duration. It never runs at half size, caches or steps: every measured frame draws it, a still one too (a timed frame
//! never shows a held picture: `EffectStage::held`).

use std::path::Path;
use std::sync::Arc;

use klipolo_model::contrib::ContribDef;
use klipolo_model::project::FormatKind;
use klipolo_model::scene::SceneLimits;
use klipolo_model::scene::timeline::CompiledScene;

use super::{Body, DraftLayer};
use crate::make::envelope::Params;
use crate::make::protocol::Key;
use crate::registry::{Registry, ShaderKind};

impl DraftLayer {
    /// `scene` under `def` (whose id `registry` holds it under), at `params`, in `format`.
    pub fn of_scene(
        def: &ContribDef,
        scene: Arc<CompiledScene>,
        registry: Arc<Registry>,
        params: Params,
        format: FormatKind,
    ) -> Self {
        let seconds = scene
            .duration
            .clamp(SceneLimits::MIN_DURATION, SceneLimits::MAX_DURATION);
        Self {
            def: Arc::new(def.clone()),
            kind: ShaderKind::Generator,
            body: Body::Scene(scene),
            notes: Vec::new().into(),
            hardened: Key([0; 32]),
            params,
            format,
            seconds,
            registry: registry.clone(),
            measured: registry,
            halved: None,
            stills: None,
            image: Arc::from(Path::new("")),
            audit: Arc::default(),
        }
    }
}
