//! Motion graphics in the registry: a scene's stored bytes read (`Scene::from_stored`), checked against
//! the catalogue (`check_scene`) and compiled once (`compile`), then shared by every layer that shows it. Built-in
//! presets and tests join through [`Registry::with_scenes`]; verified library and embedded scenes through
//! `Registry::with_contributions` (`pack_scene::compile_verified`, only once this computer's worker gave the
//! same bytes a verdict).

use std::sync::Arc;

use klipolo_model::contrib::{Catalog, ContribDef};
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
