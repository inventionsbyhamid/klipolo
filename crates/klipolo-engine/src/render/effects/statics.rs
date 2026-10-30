//! Generators whose picture repeats (effect ABI v1.1): one that reads none of `kl.time`,
//! `kl.progress` or `kl.frame` (static), or one whose time holds on `fps` steps, within a step. The plan gives such a
//! layer a cache key (`plan::GeneratorLayer::cache_key`); here its own pass is drawn into a pooled target the cache
//! keeps, and a later frame whose pass would write the same uniforms at the same size and place reuses that target and
//! runs no pass. The layer's filters still run on it every frame.
//!
//! **Bounded.** Keyed by (layer id, key, size): one entry per layer, at most [`MAX_CACHED_GENERATORS`]; a new one past
//! that replaces the least recently used entry, never one this frame drew. An entry goes after 8 frames unused, on
//! `forget`, and with its target when the pool drops that target (past the pool's byte budget, or under memory
//! pressure). Its target stays in the pool, so its bytes count against the pool's budgets; a picture is kept only
//! while the pool holds at most its between-frames budget and kept pictures at most [`MAX_CACHED_BYTES`]. A lookup is
//! O(entries + targets) and allocates nothing.

use std::sync::Arc;

use super::frame_info::kl_frame;
use super::padding::Inner;
use super::targets::MAX_TARGET_BYTES;
use super::{EffectStage, FrameInfo, Input};
use crate::plan::EffectPass;
use crate::registry::fnv1a64_on;

/// The most generators whose pictures are kept at once.
pub const MAX_CACHED_GENERATORS: usize = 16;
/// The most bytes kept pictures may hold, whatever else the pool holds: about two full-frame 4K
/// generators, eight at 1080p. Other layers' effects keep the rest of the frame's budget.
pub const MAX_CACHED_BYTES: u64 = 64 << 20;
/// A picture unused for this many frames goes, as an idle pooled target does.
const EXPIRE_AFTER_FRAMES: u64 = 8;
/// The key of a generator the preview's sampler made a still starts from this.
const STILL: u64 = 0x5354_494c_4c00_0000;

/// One layer's kept picture: the key it was drawn for, and the id of the pooled target holding it.
struct Entry {
    layer: Arc<str>,
    key: u64,
    target: u64,
    used: u64,
}

/// What [`GeneratorCache::keep`] did.
#[derive(Debug, PartialEq, Eq)]
enum Kept {
    /// In a free slot.
    Fresh,
    /// In place of the least recently used entry: the pool takes its target back.
    Replacing(u64),
    /// Not kept: every entry is this frame's.
    Full,
}

/// The kept pictures, by layer (the bookkeeping; the targets are the pool's).
pub(super) struct GeneratorCache {
    entries: Vec<Entry>,
}

impl GeneratorCache {
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::with_capacity(MAX_CACHED_GENERATORS),
        }
    }

    /// The target `layer` keeps for `key`, marked used at `now`.
    fn hit(&mut self, layer: &str, key: u64, now: u64) -> Option<u64> {
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.key == key && *e.layer == *layer)?;
        entry.used = now;
        Some(entry.target)
    }

    /// Takes out `layer`'s entry (one per layer): the target it kept.
    fn take_layer(&mut self, layer: &str) -> Option<u64> {
        let i = self.entries.iter().position(|e| *e.layer == *layer)?;
        Some(self.entries.swap_remove(i).target)
    }

    /// Keeps `target` for `layer` under `key` at `now`.
    fn keep(&mut self, layer: &Arc<str>, key: u64, target: u64, now: u64) -> Kept {
        let entry = Entry {
            layer: layer.clone(),
            key,
            target,
            used: now,
        };
        if self.entries.len() < MAX_CACHED_GENERATORS {
            self.entries.push(entry);
            return Kept::Fresh;
        }
        let oldest = self
            .entries
            .iter_mut()
            .filter(|e| e.used < now)
            .min_by_key(|e| e.used);
        match oldest {
            Some(slot) => Kept::Replacing(std::mem::replace(slot, entry).target),
            None => Kept::Full,
        }
    }

    /// Forgets the entry whose target the pool dropped.
    pub(super) fn forget_target(&mut self, target: u64) {
        self.entries.retain(|e| e.target != target);
    }

    /// Drops the entries unused for more than [`EXPIRE_AFTER_FRAMES`] at `now`, handing each one's target to
    /// `let_go`.
    fn expire(&mut self, now: u64, mut let_go: impl FnMut(u64)) {
        self.entries.retain(|e| {
            let keep = now - e.used <= EXPIRE_AFTER_FRAMES;
            if !keep {
                let_go(e.target);
            }
            keep
        });
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Every entry's target, the entries gone (the render worker's registry swap).
    pub(super) fn take_all(&mut self) -> Vec<u64> {
        self.entries.drain(..).map(|e| e.target).collect()
    }
}

/// The plan's `cache_key` with what the compositor decides: the target's size, the inner rect a padded layer's
/// generator draws into, and the `KlFrame` bytes its pass would read (its drawn size, `kl.px`, `kl.content`,
/// `kl.resolution` among them). A picture is reused only when all of it is the same.
fn key(cache_key: u64, size: (u32, u32), inner: Option<Inner>, kl: &[u8]) -> u64 {
    let rect = inner.map_or([u32::MAX; 4], |r| {
        [r.x.to_bits(), r.y.to_bits(), r.w.to_bits(), r.h.to_bits()]
    });
    let h = [size.0, size.1]
        .into_iter()
        .chain(rect)
        .fold(cache_key, |h, v| fnv1a64_on(h, &v.to_le_bytes()));
    fnv1a64_on(h, kl)
}

impl EffectStage {
    /// A generator's own pass (layer `layer`) drawn into a new target of `size` (into `inner` of it when given, as
    /// `apply_padded` draws a square generator into its grown target), or the picture it drew before: with
    /// `cache_key` (static, or within a step) and the same uniforms, size and place, the kept target and no pass. A
    /// new picture replaces the layer's old one, whose target the pool takes back first (so a stepped generator draws
    /// each step into the same target). None when the pass can't run (it doesn't compile, or the frame's budget).
    #[allow(clippy::too_many_arguments)]
    pub fn generate(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layer: &Arc<str>,
        cache_key: Option<u64>,
        pass: &EffectPass,
        size: (u32, u32),
        inner: Option<Inner>,
        info: FrameInfo,
        warnings: &mut Vec<String>,
    ) -> Option<usize> {
        let shader = &pass.shader;
        // A generator has one pass (no host-run second pass, `ShaderEffect::then`): this draws only that.
        debug_assert!(shader.then.is_none(), "{} has a second pass", shader.id);
        // One the preview's sampler made a still keeps the first picture it draws from now on,
        // whatever its time, while its size and place stay: the static cache's own path, with no time in its key.
        let still = self.degraded(shader) == Some(super::sampling::Degrade::Still);
        let cache_key = if self.fresh {
            // A timed frame draws its pass: a kept picture would leave its cost out.
            None
        } else if still {
            Some(fnv1a64_on(STILL, &shader.key.to_le_bytes()))
        } else {
            cache_key
        };
        let Some(cache_key) = cache_key else {
            return self.draw(
                device,
                queue,
                encoder,
                shader,
                pass,
                Input::Empty,
                size,
                inner,
                info,
                warnings,
                false,
            );
        };
        let drawn = inner.map_or([size.0 as f32, size.1 as f32], |r| [r.w, r.h]);
        let frame = kl_frame(pass, drawn, info);
        let kl = if still {
            &[][..]
        } else {
            bytemuck::bytes_of(&frame)
        };
        let key = key(cache_key, size, inner, kl);
        let now = self.frame;
        if let Some(id) = self.statics.hit(layer, key, now) {
            if let Some(i) = self.targets.iter().position(|t| t.id == id) {
                self.targets[i].used = now;
                // A kept picture counts as a frame that drew it: a static or still agent generator's
                // draw mark closes too.
                if self.mode == crate::render::mode::RenderMode::Interactive
                    && shader.trust == crate::registry::ShaderTrust::Checked
                {
                    self.handshake.drew(shader, now, std::time::Instant::now());
                }
                return Some(i);
            }
            self.statics.forget_target(id);
        }
        if let Some(old) = self.statics.take_layer(layer) {
            self.let_go(old);
        }
        let out = self.draw(
            device,
            queue,
            encoder,
            shader,
            pass,
            Input::Empty,
            size,
            inner,
            info,
            warnings,
            false,
        )?;
        // A heavy frame keeps nothing more, and kept pictures stay within their own budget: other layers' effects need
        // the room.
        let picture = self.target_bytes_of(out);
        if may_keep(self.target_bytes(), self.kept_bytes(), picture) {
            let id = self.targets[out].id;
            match self.statics.keep(layer, key, id, now) {
                Kept::Fresh => self.targets[out].kept = true,
                Kept::Replacing(old) => {
                    self.targets[out].kept = true;
                    self.let_go(old);
                }
                Kept::Full => {}
            }
        }
        Some(out)
    }

    /// The pool takes back the target the cache kept as `id`: free at once unless this frame used it.
    pub(super) fn let_go(&mut self, id: u64) {
        let now = self.frame;
        if let Some(t) = self.targets.iter_mut().find(|t| t.id == id) {
            t.kept = false;
            if t.used < now {
                t.busy = false;
            }
        }
    }

    /// Every kept picture goes back to the pool (the render worker's registry swap: a picture is another draft's).
    pub(super) fn let_go_statics(&mut self) {
        for id in self.statics.take_all() {
            self.let_go(id);
        }
    }

    /// The frame's start: pictures unused for a while go back to the pool (which then drops their idle targets).
    pub(super) fn expire_statics(&mut self) {
        let (now, targets) = (self.frame, &mut self.targets);
        self.statics.expire(now, |id| {
            if let Some(t) = targets.iter_mut().find(|t| t.id == id) {
                t.kept = false;
            }
        });
    }
}

/// Whether a new picture of `picture` bytes may be kept: the pool holds at most its between-frames budget
/// (`pool_bytes` ≤ `MAX_TARGET_BYTES`), and kept pictures, this one with them, at most `MAX_CACHED_BYTES`.
fn may_keep(pool_bytes: u64, kept_bytes: u64, picture: u64) -> bool {
    pool_bytes <= MAX_TARGET_BYTES && kept_bytes.saturating_add(picture) <= MAX_CACHED_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kept pictures stop at 64 MiB of their own (about two full-frame 4K generators), and
    /// nothing is kept while the pool is over its 192 MiB.
    #[test]
    fn kept_pictures_stay_within_64_mib_and_the_pools_budget() {
        let uhd = 3840 * 2160 * 4;
        assert!(may_keep(2 * uhd, uhd, uhd), "a second 4K picture fits");
        assert!(!may_keep(3 * uhd, 2 * uhd, uhd), "a third doesn't");
        assert!(may_keep(0, MAX_CACHED_BYTES - 16, 16), "exactly the cap");
        assert!(!may_keep(0, MAX_CACHED_BYTES - 16, 17));
        assert!(
            !may_keep(MAX_TARGET_BYTES + 1, 0, 16),
            "a heavy frame keeps nothing"
        );
        assert!(!may_keep(0, u64::MAX, 1), "never wraps");
    }

    fn layer(i: usize) -> Arc<str> {
        format!("layer{i}").into()
    }

    /// One entry per layer, at most 16: a 17th layer replaces the least recently used entry, never one this frame
    /// drew; entries go after 8 unused frames and with their target.
    #[test]
    fn the_cache_stays_bounded_and_replaces_the_least_recently_used() {
        let mut cache = GeneratorCache::new();
        for i in 0..MAX_CACHED_GENERATORS {
            assert_eq!(cache.keep(&layer(i), 7, i as u64, i as u64), Kept::Fresh);
        }
        assert_eq!(cache.hit("layer3", 7, 20), Some(3));
        assert_eq!(cache.hit("layer3", 8, 20), None, "another key");
        // Layer 0 was used longest ago.
        assert_eq!(cache.keep(&layer(99), 7, 99, 20), Kept::Replacing(0));
        assert_eq!(cache.hit("layer0", 7, 21), None);
        assert_eq!(cache.entries.len(), MAX_CACHED_GENERATORS);
        // All used this frame: nothing is replaced.
        for i in 1..MAX_CACHED_GENERATORS {
            cache.hit(&layer(i), 7, 30);
        }
        cache.hit("layer99", 7, 30);
        assert_eq!(cache.keep(&layer(100), 7, 100, 30), Kept::Full);
        // A layer's new picture takes its old entry out first.
        assert_eq!(cache.take_layer("layer5"), Some(5));
        assert_eq!(cache.take_layer("layer5"), None);
        cache.forget_target(6);
        assert_eq!(cache.hit("layer6", 7, 31), None);
        let mut gone = Vec::new();
        cache.expire(38, |t| gone.push(t));
        assert!(gone.is_empty(), "used at 30: kept 8 frames on");
        cache.expire(39, |t| gone.push(t));
        assert_eq!(gone.len(), MAX_CACHED_GENERATORS - 2);
        assert!(cache.entries.is_empty());
    }
}
