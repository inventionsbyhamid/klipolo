//! Motion graphics in the registry: a scene's stored bytes read (`Scene::from_stored`), checked against
//! the catalogue (`check_scene`) and compiled once (`compile`), then shared by every layer that shows it. Built-in
//! presets and tests join through [`Registry::with_scenes`]; verified library and embedded scenes through
//! `Registry::with_contributions` (`pack_scene::compile_verified`, only once this computer's worker gave the
//! same bytes a verdict). Animated words make their scene from their settings (`Registry::scene_for`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_model::contrib::{Catalog, ContribDef, ContribId};
use klipolo_model::media::text_motion;
use klipolo_model::params::ParamValue;
use klipolo_model::scene::timeline::{CompiledScene, compile};
use klipolo_model::scene::{Scene, SceneProblem, check_scene};

use super::Registry;
use super::pack_scene::VerifiedScene;

/// A stored scene's bytes read, checked and compiled; its problems when it doesn't check. Never panics (the loader is
/// total). For bytes a verdict covers or a built-in preset.
pub fn compile_scene(
    bytes: &[u8],
    catalog: &Catalog,
) -> Result<Arc<CompiledScene>, Vec<SceneProblem>> {
    let scene = Scene::from_stored(bytes)?;
    check_scene(&scene, catalog)?;
    Ok(Arc::new(compile(&scene)))
}

impl Registry {
    /// This registry with each scene under its definition: the definitions join the catalogue (names and settings show
    /// everywhere) and generator layers naming them draw them. Built once per snapshot, never per frame.
    pub fn with_scenes(
        &self,
        scenes: impl IntoIterator<Item = (ContribDef, Arc<CompiledScene>)>,
    ) -> Registry {
        let mut registry = self.clone();
        for (def, scene) in scenes {
            let scene = VerifiedScene {
                scene,
                envelope: None,
            };
            registry.scenes.insert(def.id.clone(), scene);
            registry.catalog = std::mem::take(&mut registry.catalog).with(def);
        }
        registry
    }
}

/// Animated words compiled at most (a project uses far fewer; past it the oldest go).
pub const MAX_WORD_SCENES: usize = 64;

/// The animated words' scenes compiled so far, by their settings' hash: every rebuilt registry shares them, so a
/// layer's scene is the same `Arc` from frame to frame (the drawer keys its instances on it).
static WORD_SCENES: Mutex<Vec<(u64, Arc<CompiledScene>)>> = Mutex::new(Vec::new());

impl Registry {
    /// The compiled scene a generator layer of `source` with settings `params` draws: animated words
    /// (`media::text_motion::SOURCE`) make theirs from their settings, checked against the catalogue and compiled
    /// once for those settings; any other source is `scene`'s. None draws as nothing.
    pub fn scene_for(
        &self,
        source: &ContribId,
        params: &BTreeMap<String, ParamValue>,
    ) -> Option<Arc<CompiledScene>> {
        if source.as_str() != text_motion::SOURCE {
            return self.scene(source).cloned();
        }
        let key = crate::plan::scene::params_hash(params);
        let mut cache = WORD_SCENES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = cache.iter().position(|(k, _)| *k == key) {
            // Kept newest last, so the oldest goes first.
            let hit = cache.remove(i);
            let scene = hit.1.clone();
            cache.push(hit);
            return Some(scene);
        }
        let scene = text_motion::scene_of(params);
        check_scene(&scene, &self.catalog).ok()?;
        let compiled = Arc::new(compile(&scene));
        if cache.len() >= MAX_WORD_SCENES {
            cache.remove(0);
        }
        cache.push((key, compiled.clone()));
        Some(compiled)
    }
}
