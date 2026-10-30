//! The effect stage's pipelines: one per shader, found by its 64-bit key but taken only when the
//! stored text is the shader's own (the same `Arc`, else an equal text), so two shaders whose keys collide never share
//! a pipeline. Built-ins stay; other shaders' pipelines are bounded (`MAX_PIPELINES`, the least recently used go
//! first, never one this frame used) and go when the render worker's registry no longer has them (`retain`).
//!
//! The app's preview pins the shaders of the registry it draws with (`set_pins`): those are
//! never evicted, so a project using more than `MAX_PIPELINES` never compiles one again when the playhead comes back to
//! it. They are bounded by the project instead (its registry holds a shader only for what it uses, at most 64 copies),
//! and go when a registry no longer has them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::registry::{ShaderEffect, ShaderTrust};

/// Pipelines of shaders that aren't built in, kept at most.
pub(super) const MAX_PIPELINES: usize = 32;

struct Entry {
    source: Arc<str>,
    /// None when the shader failed to compile (reported once).
    pipeline: Option<wgpu::RenderPipeline>,
    builtin: bool,
    used: u64,
}

#[derive(Default)]
pub(super) struct PipelineCache {
    entries: HashMap<u64, Entry>,
    /// Keys never evicted (the preview's registry).
    pins: HashSet<u64>,
}

fn same(stored: &Arc<str>, shader: &ShaderEffect) -> bool {
    Arc::ptr_eq(stored, &shader.source) || **stored == *shader.source
}

impl PipelineCache {
    /// Whether `shader` has an entry (compiled or failed).
    pub(super) fn contains(&self, shader: &ShaderEffect) -> bool {
        self.entries
            .get(&shader.key)
            .is_some_and(|e| same(&e.source, shader))
    }

    /// `shader`'s pipeline, marked used at frame `now`; None when it failed or has none.
    pub(super) fn get(&mut self, shader: &ShaderEffect, now: u64) -> Option<&wgpu::RenderPipeline> {
        let e = self
            .entries
            .get_mut(&shader.key)
            .filter(|e| same(&e.source, shader))?;
        e.used = now;
        e.pipeline.as_ref()
    }

    /// Keeps `pipeline` for `shader` (replacing one stored under its key for another text), then evicts the least
    /// recently used pipeline that isn't built in, while there are more than `MAX_PIPELINES`.
    pub(super) fn insert(
        &mut self,
        shader: &ShaderEffect,
        pipeline: Option<wgpu::RenderPipeline>,
        now: u64,
    ) {
        self.entries.insert(
            shader.key,
            Entry {
                source: shader.source.clone(),
                pipeline,
                builtin: shader.trust == ShaderTrust::Builtin,
                used: now,
            },
        );
        let pins = &self.pins;
        let evictable = |(k, e): &(&u64, &Entry)| !e.builtin && !pins.contains(*k);
        while self.entries.iter().filter(evictable).count() > MAX_PIPELINES {
            let oldest = self
                .entries
                .iter()
                .filter(evictable)
                .filter(|(_, e)| e.used < now)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| *k);
            match oldest {
                Some(k) => {
                    self.entries.remove(&k);
                }
                None => break,
            }
        }
    }

    /// Keeps the built-ins and the pipelines of shaders `keep` names (the render worker's new registry).
    pub(super) fn retain(&mut self, keep: &[&ShaderEffect]) {
        self.entries
            .retain(|key, e| e.builtin || keep.iter().any(|s| s.key == *key && same(&e.source, s)));
    }

    /// The keys never evicted from now on (the preview's registry, `EffectStage::prepare`).
    pub(super) fn set_pins(&mut self, keys: HashSet<u64>) {
        self.pins = keys;
    }

    /// Pipelines kept (built-ins included).
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Pipelines of shaders that aren't built in, kept for drawing.
    pub(super) fn checked_len(&self) -> usize {
        self.entries
            .values()
            .filter(|e| !e.builtin && e.pipeline.is_some())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use klipolo_model::contrib::{ContribDef, ContribKind};

    use super::*;
    use crate::registry::ShaderKind;

    fn shader(i: usize) -> ShaderEffect {
        let id = format!("lib.k3f9q2ma/generator.c{i}.v1");
        let def = ContribDef::new(id.as_str(), ContribKind::Generator, "C");
        let body = format!(
            "fn effect(uv: vec2<f32>) -> vec4<f32> {{ return vec4<f32>({i}.0 / 64.0, 0.0, 0.0, 1.0); }}"
        );
        ShaderEffect::assemble_untrusted(&def, ShaderKind::Generator, &body).unwrap()
    }

    /// Bounded, the least recently used first; a key shared by another text is never taken; a registry swap lets
    /// the shaders it doesn't hold go.
    #[test]
    fn the_cache_is_bounded_and_never_shares_a_key_between_texts() {
        let mut cache = PipelineCache::default();
        let all: Vec<ShaderEffect> = (0..40).map(shader).collect();
        for (now, s) in all.iter().enumerate() {
            cache.insert(s, None, now as u64 + 1);
        }
        assert_eq!(cache.len(), MAX_PIPELINES);
        assert!(!cache.contains(&all[0]) && cache.contains(&all[39]));
        let mut forged = shader(99);
        forged.key = all[39].key;
        assert!(!cache.contains(&forged), "another text under the same key");
        assert!(cache.get(&forged, 50).is_none());
        cache.retain(&[&all[38]]);
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&all[38]));
    }

    /// The preview's pinned shaders are never evicted, however many; the rest keep the cap.
    #[test]
    fn pinned_pipelines_are_never_evicted() {
        let mut cache = PipelineCache::default();
        let all: Vec<ShaderEffect> = (0..48).map(shader).collect();
        cache.set_pins(all[..40].iter().map(|s| s.key).collect());
        for (now, s) in all.iter().enumerate() {
            cache.insert(s, None, now as u64 + 1);
        }
        assert!(
            all[..40].iter().all(|s| cache.contains(s)),
            "all 40 pinned stay"
        );
        assert_eq!(cache.len(), 48, "the 8 others are under the cap");
        let more: Vec<ShaderEffect> = (48..90).map(shader).collect();
        for (now, s) in more.iter().enumerate() {
            cache.insert(s, None, now as u64 + 100);
        }
        assert!(all[..40].iter().all(|s| cache.contains(s)));
        assert_eq!(cache.len(), 40 + MAX_PIPELINES);
    }

    /// A built-in's pipeline (the scene drawer's host passes are built-ins) stays through
    /// the render worker's registry change, though the new registry doesn't name it.
    #[test]
    fn a_builtins_pipeline_stays_through_a_registry_change() {
        let mut cache = PipelineCache::default();
        let def = ContribDef::new("klipolo.builtin/internal.kept", ContribKind::Generator, "K");
        let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";
        let host = ShaderEffect::assemble(&def, ShaderKind::Generator, body).unwrap();
        cache.insert(&host, None, 1);
        cache.insert(&shader(1), None, 1);
        cache.retain(&[]);
        assert!(cache.contains(&host) && !cache.contains(&shader(1)));
    }
}
