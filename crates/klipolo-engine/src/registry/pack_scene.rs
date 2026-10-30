//! Verified motion graphics. A scene that isn't built in (a library version, a project's
//! copy, an export job's folder) is parsed, checked and compiled in this process only once this computer's render
//! worker gave its exact bytes a verdict: the app's one `runnable` predicate in the preview, `verified.json` in an export
//! job. [`compile_verified`] then runs the three steps the worker ran on the same bytes (`Scene::from_stored`,
//! `check_scene` against the registry's catalogue, `compile`), and [`CompiledScenes`] keeps each by its key, so every
//! rebuilt registry shares one compiled scene (the drawer's instances are keyed on its identity: none is built again)
//! and nothing is parsed twice. Until its verdict a scene is in the catalogue only and draws as nothing, noted
//! "Checking an effect…", exactly as an unchecked shader is; the "Agent-made effects" switch covers scenes too. A
//! verified scene draws with the settings its verdict measured ([`VerifiedScene::envelope`]: the plan clamps into it).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_model::contrib::Catalog;
use klipolo_model::scene::SceneProblem;
use klipolo_model::scene::timeline::CompiledScene;

use super::key::ContribSource;
use super::pack::{LoadedContrib, Verdict};
use crate::make::envelope::Envelope;

/// Compiled scenes kept at most (a project uses far fewer; `retain` keeps only those).
pub const MAX_COMPILED_SCENES: usize = 64;

/// A scene compiled, or why it didn't check.
pub type Compiled = Result<Arc<CompiledScene>, Vec<SceneProblem>>;

/// A verified scene as a registry draws it: compiled, and the settings its verdict was measured within (None: a
/// built-in preset or a test's, never clamped).
#[derive(Clone)]
pub struct VerifiedScene {
    pub scene: Arc<CompiledScene>,
    pub envelope: Option<Arc<Envelope>>,
}

impl VerifiedScene {
    /// `scene` with `verdict`'s envelope.
    pub fn new(scene: Arc<CompiledScene>, verdict: &Verdict) -> Self {
        Self {
            scene,
            envelope: Some(Arc::new(verdict.envelope.clone())),
        }
    }
}

/// `c`'s scene read, checked against `catalog` and compiled: only for a scene this computer verified (see the module's
/// doc). A shader's source is refused. Never panics (the loader is total).
pub fn compile_verified(c: &LoadedContrib, catalog: &Catalog) -> Compiled {
    let ContribSource::Scene(bytes) = &c.source else {
        return Err(vec![SceneProblem {
            path: String::new(),
            code: "scene.notAScene",
            message: "A shader isn't drawn as a scene.".into(),
        }]);
    };
    super::compile_scene(bytes, catalog)
}

struct Entry {
    scene: Compiled,
    used: u64,
    /// The catalogue it was checked against (the caller's name for it): a refusal under another is stale.
    catalog: u64,
}

impl Entry {
    /// Whether it still answers for `catalog`: a compiled scene always (its fills name versioned ids, whose definitions
    /// never change), a refusal only under the catalogue it was refused under (a fill's generator may be there now).
    fn answers(&self, catalog: u64) -> bool {
        self.scene.is_ok() || self.catalog == catalog
    }
}

#[derive(Default)]
struct Entries {
    by_key: HashMap<[u8; 32], Entry>,
    clock: u64,
}

/// Verified scenes compiled, kept by their key (as `ShaderCache` keeps shaders): released by `retain` after each
/// snapshot, and never more than [`MAX_COMPILED_SCENES`] (the least recently used go). A scene that didn't compile is
/// tried again under another catalogue, and after `forget_failed` (the person's "Check again"), which moves the
/// cache's `generation` so a snapshot builds again.
#[derive(Default)]
pub struct CompiledScenes {
    entries: Mutex<Entries>,
    compiled: AtomicUsize,
    generation: AtomicU64,
}

impl CompiledScenes {
    pub fn new() -> Self {
        Self::default()
    }

    /// `c`'s compiled scene checked against `catalog`, which the caller names `catalog_id` (a new name when the
    /// catalogue changes): the one kept under its key when it still answers (`Entry::answers`), else compiled
    /// (`compile_verified`, outside the lock) and kept, whether it checked or not. Only for a scene this computer
    /// verified. When two compile one key at once, the first kept stays, so every registry shares one compiled scene.
    pub fn get_or_compile(
        &self,
        c: &LoadedContrib,
        catalog: &Catalog,
        catalog_id: u64,
    ) -> Compiled {
        if let Some(kept) = self.kept(&c.key, catalog_id) {
            return kept;
        }
        self.compiled.fetch_add(1, Ordering::Relaxed);
        let scene = compile_verified(c, catalog);
        if let Some(kept) = self.kept(&c.key, catalog_id) {
            return kept;
        }
        let mut entries = self.lock();
        entries.clock += 1;
        let used = entries.clock;
        let entry = Entry {
            scene: scene.clone(),
            used,
            catalog: catalog_id,
        };
        entries.by_key.insert(c.key, entry);
        if entries.by_key.len() > MAX_COMPILED_SCENES
            && let Some(oldest) = entries
                .by_key
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k)
        {
            entries.by_key.remove(&oldest);
        }
        scene
    }

    /// `key`'s kept scene if it answers for `catalog_id`, counted as a use.
    fn kept(&self, key: &[u8; 32], catalog_id: u64) -> Option<Compiled> {
        let mut entries = self.lock();
        entries.clock += 1;
        let clock = entries.clock;
        let entry = entries
            .by_key
            .get_mut(key)
            .filter(|e| e.answers(catalog_id))?;
        entry.used = clock;
        Some(entry.scene.clone())
    }

    /// `key`'s scene if it is kept (compiled, or refused), never compiling it. Not a use.
    pub fn get(&self, key: [u8; 32]) -> Option<Compiled> {
        self.lock().by_key.get(&key).map(|e| e.scene.clone())
    }

    /// Whether `key`'s scene is kept and answers for `catalog_id`: `get_or_compile` would parse nothing for it. Not a
    /// use.
    pub fn holds(&self, key: [u8; 32], catalog_id: u64) -> bool {
        self.lock()
            .by_key
            .get(&key)
            .is_some_and(|e| e.answers(catalog_id))
    }

    /// Lets go of the scenes that didn't compile, so they are tried again (the person's "Check again", a reopening),
    /// and moves `generation`.
    pub fn forget_failed(&self) {
        self.lock().by_key.retain(|_, e| e.scene.is_ok());
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Moves at each `forget_failed`: a snapshot keyed on it builds again.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Keeps only the scenes of `keys` (the ones the last snapshot used).
    pub fn retain(&self, keys: &HashSet<[u8; 32]>) {
        self.lock().by_key.retain(|key, _| keys.contains(key));
    }

    /// How many times a scene was parsed and compiled through this cache.
    pub fn compiled(&self) -> usize {
        self.compiled.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> usize {
        self.lock().by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
