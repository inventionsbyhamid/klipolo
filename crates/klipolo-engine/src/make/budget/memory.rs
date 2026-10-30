//! A layer's GPU memory for the budget classes: its own share per kind, and what is extra. Own,
//! per kind: a filter's stack's targets grown by its padding (at most two, the ping-pong pair); a generator's target
//! (grown by any padding); a text layer's texture; a transition's two sides and its result; a scene's target plus its
//! instance buffers, path meshes and unit atlas up to `SCENE_OWN_BYTES`. Anything beyond is extra: layer-sized targets
//! from the pool count in `extra_targets`, everything else in `extra_bytes`.

use super::MemoryUse;

/// A scene's own buffers (its instances, path meshes and unit atlas) count as its own up to this.
pub const SCENE_OWN_BYTES: u64 = 4 << 20;

/// What a layer owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Own {
    /// One of its layer-sized targets, in bytes: the pool's extra targets are counted in these.
    pub target: u64,
    /// How many of them are its own.
    pub targets: u32,
    /// Buffers it owns besides (a scene's, up to `SCENE_OWN_BYTES`).
    pub buffers: u64,
}

/// An RGBA8 target of `w × h` grown by `pad` pixels on every side (the effect pool's format, `EFFECT_FORMAT`).
fn rgba8((w, h): (u32, u32), pad: f64) -> u64 {
    let side = |n: u32| (f64::from(n) + 2.0 * pad.max(0.0)).ceil() as u64;
    side(w) * side(h) * 4
}

impl Own {
    /// A generator: its target, `size`, grown by any padding.
    pub fn generator(size: (u32, u32), pad: f64) -> Self {
        Self::targets(rgba8(size, pad), 1)
    }

    /// A filter: its stack's ping-pong pair, each `size` (the layer's) grown by its padding.
    pub fn filter(size: (u32, u32), pad: f64) -> Self {
        Self::targets(rgba8(size, pad), 2)
    }

    /// A transition: its two sides and its result, each the canvas.
    pub fn transition(canvas: (u32, u32)) -> Self {
        Self::targets(rgba8(canvas, 0.0), 3)
    }

    /// A text layer: its texture.
    pub fn text(texture: (u32, u32)) -> Self {
        Self::targets(rgba8(texture, 0.0), 1)
    }

    /// A scene: its target and its buffers up to `SCENE_OWN_BYTES`.
    pub fn scene(target: (u32, u32)) -> Self {
        Self {
            buffers: SCENE_OWN_BYTES,
            ..Self::targets(rgba8(target, 0.0), 1)
        }
    }

    fn targets(target: u64, targets: u32) -> Self {
        Self {
            target,
            targets,
            buffers: 0,
        }
    }

    /// The layer's memory while drawn alone: `pool`, the effect pool's targets at their peak less the baseline's, and
    /// `buffers`, everything else it made (a scene's), split into its own share and what is extra.
    pub fn split(self, pool: u64, buffers: u64) -> MemoryUse {
        let own_pool = pool.min(self.target.saturating_mul(u64::from(self.targets)));
        let extra_pool = pool - own_pool;
        let extra_targets = extra_pool / self.target.max(1);
        let own_buffers = buffers.min(self.buffers);
        MemoryUse {
            own: own_pool + own_buffers,
            extra_targets: extra_targets.min(u64::from(u32::MAX)) as u32,
            extra_bytes: (extra_pool - extra_targets * self.target.max(1))
                + (buffers - own_buffers),
            peak: pool.saturating_add(buffers),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What is its own is never extra; pool memory past it counts in layer-sized targets, and the rest in bytes; a
    /// scene's buffers past 4 MiB are extra bytes.
    #[test]
    fn own_shares_per_kind() {
        let generator = Own::generator((1280, 720), 0.0);
        let t = 1280 * 720 * 4;
        assert_eq!(generator.split(t, 0).extra_targets, 0);
        let more = generator.split(2 * t + 100, 0);
        assert_eq!(
            (more.own, more.extra_targets, more.extra_bytes),
            (t, 1, 100)
        );
        let scene = Own::scene((1280, 720));
        let s = scene.split(t, 5 << 20);
        assert_eq!(
            (s.own, s.extra_targets, s.extra_bytes),
            (t + (4 << 20), 0, 1 << 20)
        );
        assert_eq!(
            Own::transition((1280, 720)).split(3 * t, 0).extra_targets,
            0
        );
    }
}
