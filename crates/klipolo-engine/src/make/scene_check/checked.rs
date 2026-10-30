//! One path for a submitted and a stored scene: read with the total loader (a
//! submission's `svg` nodes flattened in place), written as the canonical bytes (`normalise`) and read back as stored, so
//! what an app saves is exactly what the main process will accept; checked against the built-ins and the generators
//! its fills name; compiled. A submission's definition takes the scene's own settings (so the catalogue shows them and
//! the measurement's envelope covers each costly one); a stored definition must already hold them, else its settings
//! could name what was never measured. The key is over the definition and those exact bytes: what an app saves
//! under it needs no second check.

use std::sync::Arc;

use klipolo_model::contrib::{Catalog, ContribDef, ContribKind};
use klipolo_model::scene::timeline::{CompiledScene, compile};
use klipolo_model::scene::{Scene, SceneProblem, check_scene};

use super::svgs::splice;
use super::written;
use crate::make::protocol::Key;
use crate::registry::key::{ContribSource, contrib_key};
use crate::registry::{Registry, ShaderEffect};

/// What a scene's fills may name: the built-ins and the generators the app handed over (`inputs::read_fills`), each
/// checked and hardened here.
pub struct Fills {
    pub catalog: Catalog,
    pub shaders: Vec<(ContribDef, ShaderEffect)>,
}

impl Default for Fills {
    fn default() -> Self {
        Self {
            catalog: Catalog::builtin().clone(),
            shaders: Vec::new(),
        }
    }
}

/// What is checked.
pub enum SceneInput<'a> {
    /// An agent's scene and the SVGs its request brought (name, text).
    Submitted {
        bytes: &'a [u8],
        svgs: &'a [(String, Vec<u8>)],
    },
    /// A library or project folder's `scene.json`, exactly as its key covers it.
    Stored { bytes: &'a [u8] },
}

/// A scene that passed: its definition (with its settings), its stored bytes, its compile, its key, and a registry
/// drawing it under its definition's id with the generators its fills name.
pub struct CheckedScene {
    pub def: ContribDef,
    pub bytes: Vec<u8>,
    pub scene: Arc<CompiledScene>,
    pub key: Key,
    pub registry: Arc<Registry>,
}

/// `input` under `def` (a scene's definition), its fills from `fills`; every problem with its JSON path.
pub fn check(
    def: &ContribDef,
    input: SceneInput<'_>,
    fills: Fills,
) -> Result<CheckedScene, Vec<SceneProblem>> {
    if def.kind != ContribKind::Scene {
        return Err(one(
            "",
            "scene.notAScene",
            "This definition isn't a motion graphic's.",
        ));
    }
    let mut def = def.clone();
    let (scene, bytes) = match input {
        SceneInput::Submitted { bytes, svgs } => {
            let mut scene = Scene::from_submission(bytes)?;
            splice(&mut scene, svgs)?;
            let stored = Scene::from_stored(&written(&scene)?)?;
            let bytes = written(&stored)?;
            def.params = stored.params.clone();
            (stored, bytes)
        }
        SceneInput::Stored { bytes } => {
            let scene = Scene::from_stored(bytes)?;
            if def.params != scene.params {
                return Err(one(
                    "/params",
                    "scene.paramsDiffer",
                    "The definition's settings aren't the scene's own: save it again from its draft.",
                ));
            }
            (scene, bytes.to_vec())
        }
    };
    check_scene(&scene, &fills.catalog)?;
    let compiled = Arc::new(compile(&scene));
    let key = Key(contrib_key(&def, &ContribSource::Scene(bytes.clone())));
    let registry =
        Registry::with_checked(fills.shaders).with_scenes([(def.clone(), compiled.clone())]);
    Ok(CheckedScene {
        def,
        bytes,
        scene: compiled,
        key,
        registry: Arc::new(registry),
    })
}

fn one(path: &str, code: &'static str, message: &str) -> Vec<SceneProblem> {
    vec![SceneProblem {
        path: path.into(),
        code,
        message: message.into(),
    }]
}
