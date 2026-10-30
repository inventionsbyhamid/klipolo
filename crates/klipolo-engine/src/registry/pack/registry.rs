//! The registry built from loaded contributions: every definition joins the catalogue, so names,
//! settings and kinds show everywhere; a contribution draws only with the shader or the scene the caller gives it,
//! which decides what is runnable (the switch, the crash guard, a verified cost; scenes the same way,
//! compiled only once verified, `pack_scene`).

use std::sync::Arc;

use klipolo_model::contrib::ContribDef;

use super::LoadedContrib;
use crate::registry::key::ContribSource;
use crate::registry::pack_scene::VerifiedScene;
use crate::registry::{Registry, ShaderEffect, ShaderTrust};

/// Counts only: the feed (`ProjectFeed`) prints it in its own `Debug`.
impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Registry({} definitions, {} shaders, {} scenes)",
            self.catalog.defs().count(),
            self.shaders.len(),
            self.scenes.len()
        )
    }
}

impl Registry {
    /// `base` plus `contribs`: each definition in the catalogue, and a shader only where `shader_for` gives one that
    /// was checked and hardened for that very id, a scene only where `scene_for` gives one (compiled once verified:
    /// `pack_scene::compile_verified`). A built-in id is never replaced; a contribution without either draws as
    /// nothing, even if `base` had one under its id.
    pub fn with_contributions<'a>(
        base: &Registry,
        contribs: impl IntoIterator<Item = &'a LoadedContrib>,
        shader_for: &dyn Fn(&LoadedContrib) -> Option<Arc<ShaderEffect>>,
        scene_for: &dyn Fn(&LoadedContrib) -> Option<VerifiedScene>,
    ) -> Registry {
        let mut registry = base.clone();
        for c in contribs {
            let id = &c.def.id;
            if id.is_builtin() {
                continue;
            }
            registry.catalog = std::mem::take(&mut registry.catalog).with(c.def.clone());
            registry.shaders.remove(id);
            registry.scenes.remove(id);
            match &c.source {
                ContribSource::Wgsl(_) => match shader_for(c) {
                    Some(shader) if shader.trust == ShaderTrust::Checked && shader.id == *id => {
                        registry.shaders.insert(id.clone(), shader);
                    }
                    _ => {}
                },
                ContribSource::Scene(_) => {
                    if let Some(scene) = scene_for(c) {
                        registry.scenes.insert(id.clone(), scene);
                    }
                }
            }
        }
        registry
    }

    /// `base` with `defs` in its catalogue only: what the library and a project's copies hold that the
    /// project doesn't use, or that can't draw yet, is listed (named, its settings shown) and draws as nothing. A
    /// built-in id is never replaced, and no shader or scene is added or kept for these ids.
    pub fn with_definitions<'a>(
        base: &Registry,
        defs: impl IntoIterator<Item = &'a ContribDef>,
    ) -> Registry {
        let mut registry = base.clone();
        for def in defs {
            if def.id.is_builtin() {
                continue;
            }
            registry.shaders.remove(&def.id);
            registry.scenes.remove(&def.id);
            registry.catalog = std::mem::take(&mut registry.catalog).with(def.clone());
        }
        registry
    }
}
