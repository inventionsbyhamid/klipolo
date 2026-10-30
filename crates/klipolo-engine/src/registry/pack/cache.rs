//! Shaders assembled from loaded contributions, kept by their key: a rebuild of the registry links the shaders
//! it already has instead of running naga again, and a contribution whose shader didn't assemble isn't tried again
//! either. Only what the caller asks for is assembled (the ids the open project uses, once runnable), off
//! the session actor. Released by `retain` after each snapshot, and never more than `MAX_CACHED_SHADERS` (the least
//! recently used go).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_model::contrib::ContribDef;

use super::LoadedContrib;
use crate::registry::guard::Diagnostic;
use crate::registry::key::ContribSource;
use crate::registry::{ShaderEffect, ShaderKind};

/// Shaders kept at most (a project uses far fewer; `retain` keeps only those).
pub const MAX_CACHED_SHADERS: usize = 256;

type Assembled = Result<Arc<ShaderEffect>, Vec<Diagnostic>>;

struct Entry {
    shader: Assembled,
    used: u64,
}

#[derive(Default)]
struct Entries {
    /// By key and shader kind (`ShaderKind as u8`).
    by_key: HashMap<([u8; 32], u8), Entry>,
    clock: u64,
}

#[derive(Default)]
pub struct ShaderCache {
    entries: Mutex<Entries>,
    assembled: AtomicUsize,
}

impl ShaderCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// `c`'s shader as `kind`: the one kept under its key, else assembled (`assemble_untrusted`: checked and hardened)
    /// and kept, whether it assembled or not. Two versions with the same key share one assembly, each under its own
    /// id and name.
    pub fn get_or_assemble(&self, c: &LoadedContrib, kind: ShaderKind) -> Assembled {
        let key = (c.key, kind as u8);
        if let Some(found) = self.find(&key) {
            return labelled(found, &c.def);
        }
        let ContribSource::Wgsl(body) = &c.source else {
            return Err(vec![Diagnostic::error(
                "rule.kind",
                "A scene isn't drawn as a shader.",
            )]);
        };
        // naga runs outside the lock: other lookups answer meanwhile.
        self.assembled.fetch_add(1, Ordering::Relaxed);
        let made = ShaderEffect::assemble_untrusted(&c.def, kind, body).map(Arc::new);
        self.keep(key, made.clone());
        labelled(made, &c.def)
    }

    /// Whether the shader of `key` as `kind` is kept (assembled, or refused): `get_or_assemble` would run no naga for it.
    /// It doesn't count as a use.
    pub fn holds(&self, key: [u8; 32], kind: ShaderKind) -> bool {
        self.lock().by_key.contains_key(&(key, kind as u8))
    }

    /// Keeps only the shaders of `keys` (the ones the last snapshot used).
    pub fn retain(&self, keys: &HashSet<[u8; 32]>) {
        self.lock().by_key.retain(|(key, _), _| keys.contains(key));
    }

    /// How many times a shader was assembled (naga ran) through this cache.
    pub fn assembled(&self) -> usize {
        self.assembled.load(Ordering::Relaxed)
    }

    /// How many shaders are kept.
    pub fn len(&self) -> usize {
        self.lock().by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn find(&self, key: &([u8; 32], u8)) -> Option<Assembled> {
        let mut entries = self.lock();
        entries.clock += 1;
        let clock = entries.clock;
        let entry = entries.by_key.get_mut(key)?;
        entry.used = clock;
        Some(entry.shader.clone())
    }

    fn keep(&self, key: ([u8; 32], u8), shader: Assembled) {
        let mut entries = self.lock();
        entries.clock += 1;
        let used = entries.clock;
        entries.by_key.insert(key, Entry { shader, used });
        if entries.by_key.len() > MAX_CACHED_SHADERS
            && let Some(oldest) = entries
                .by_key
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k)
        {
            entries.by_key.remove(&oldest);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The shader under `def`'s own id and name (another version with the same key assembled it).
fn labelled(found: Assembled, def: &ContribDef) -> Assembled {
    let shader = found?;
    if shader.id == def.id && shader.name == def.name {
        return Ok(shader);
    }
    Ok(Arc::new(ShaderEffect {
        id: def.id.clone(),
        name: def.name.clone(),
        ..(*shader).clone()
    }))
}
