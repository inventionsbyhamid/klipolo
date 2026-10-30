//! The effect stage's targets: pooled by size, handed out per frame, dropped when idle or over budget. Within a
//! frame the pool never grows past [`MAX_FRAME_TARGET_BYTES`] for layers' passes, nor past
//! [`MAX_PROTECTED_TARGET_BYTES`] for protective ones.

use std::sync::atomic::{AtomicU64, Ordering};

use klipolo_model::layout::BoxN;

use super::pipelines::input_group;
use super::{EFFECT_FORMAT, EffectStage, MAX_EFFECT_SIDE, made};

/// Pooled targets unused for this many frames are dropped (bounded memory).
const EVICT_AFTER_FRAMES: u64 = 8;
/// Pooled targets beyond this many bytes are dropped at the next frame's start, least recently used first (a layer
/// being resized makes a new size each frame), cached generators' among them. A generator's picture is cached only
/// while the pool holds less (`statics`).
pub(super) const MAX_TARGET_BYTES: u64 = 192 << 20;
/// The most the pooled targets may hold while a frame is drawn, for layers' passes: a frame that would need more (many
/// layers with effects on screen at once, each up to 4096² while it moves) draws the rest without their effects and
/// says so once, never allocating past it. Twice the budget between frames: a 4K frame fits about ten full-frame
/// layers with effects. The main footage's and a transition's own targets are always made (at most three a frame). A
/// padded layer's grown targets (`padding.rs`) count here like any other.
pub const MAX_FRAME_TARGET_BYTES: u64 = 2 * MAX_TARGET_BYTES;
/// The ceiling for a protective pass (blur, pixelation, a shape mask: `passes::protects`). Other passes stop at
/// [`MAX_FRAME_TARGET_BYTES`], so at least that much is always left for protective ones, whatever the frame drew
/// before them: they are never dropped by the budget for other effects. Past this ceiling a protective pass doesn't
/// run either, and its layer is hidden rather than shown uncovered.
pub const MAX_PROTECTED_TARGET_BYTES: u64 = 2 * MAX_FRAME_TARGET_BYTES;
/// What a frame says once (until the project closes) when it left effects out for the budget (a code).
pub(crate) const OVER_BUDGET: &str = crate::api::FRAME_EFFECTS_LEFT_OUT;

/// The most any stage's pooled targets have held (`effect_target_peak`).
static PEAK: AtomicU64 = AtomicU64::new(0);

/// The most bytes any effect stage's pooled targets have held at once in this process (a diagnostic, for the budget's
/// test).
pub fn effect_target_peak() -> u64 {
    PEAK.load(Ordering::Relaxed)
}

/// Starts [`effect_target_peak`] again (a test measuring one frame).
#[doc(hidden)]
pub fn reset_effect_target_peak() {
    PEAK.store(0, Ordering::Relaxed);
}

/// `size` within the largest target side (a scene pass's stencil takes the same: `render::scene`).
pub(crate) fn clamped(size: (u32, u32)) -> (u32, u32) {
    (
        size.0.clamp(1, MAX_EFFECT_SIDE),
        size.1.clamp(1, MAX_EFFECT_SIDE),
    )
}

fn bytes_of(size: (u32, u32)) -> u64 {
    u64::from(size.0) * u64::from(size.1) * 4
}

/// `dest` snapped outward to whole pixels of an `out`-sized frame and clipped to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PixelRect {
    pub fn of(dest: BoxN, out: (u32, u32)) -> Option<Self> {
        let (w, h) = (f64::from(out.0), f64::from(out.1));
        let x0 = (dest.x * w).floor().clamp(0.0, w) as u32;
        let y0 = (dest.y * h).floor().clamp(0.0, h) as u32;
        let x1 = ((dest.x + dest.w) * w).ceil().clamp(0.0, w) as u32;
        let y1 = ((dest.y + dest.h) * h).ceil().clamp(0.0, h) as u32;
        (x1 > x0 && y1 > y0).then_some(Self {
            x: x0,
            y: y0,
            w: (x1 - x0).min(MAX_EFFECT_SIDE),
            h: (y1 - y0).min(MAX_EFFECT_SIDE),
        })
    }

    pub fn to_box(self, out: (u32, u32)) -> BoxN {
        let (w, h) = (f64::from(out.0), f64::from(out.1));
        BoxN {
            x: f64::from(self.x) / w,
            y: f64::from(self.y) / h,
            w: f64::from(self.w) / w,
            h: f64::from(self.h) / h,
        }
    }
}

pub(super) struct Target {
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    /// This target as `kl_a`, with `kl_b` empty.
    pub(super) input: wgpu::BindGroup,
    pub(super) size: (u32, u32),
    pub(super) used: u64,
    pub(super) busy: bool,
    /// Unique among the targets this stage made (`EffectStage::id`).
    pub(super) id: u64,
    /// It holds a cached generator's picture (`statics`): always busy, so no pass draws into it, until the cache lets
    /// it go. Its bytes count in the pool like any other target's.
    pub(super) kept: bool,
    /// Handed out while the stage watched (`watch.rs`).
    pub(super) watched: bool,
}

impl Target {
    fn bytes(&self) -> u64 {
        bytes_of(self.size)
    }
}

impl EffectStage {
    /// The targets' part of starting a frame (`begin_frame`): targets idle for a while or over the budget are
    /// dropped, and the rest are free to hand out again.
    pub(super) fn evict_targets(&mut self) {
        let now = self.frame;
        self.retain_targets(|t| now - t.used <= EVICT_AFTER_FRAMES);
        let mut bytes = self.target_bytes();
        while bytes > MAX_TARGET_BYTES {
            let Some((i, _)) = self.targets.iter().enumerate().min_by_key(|(_, t)| t.used) else {
                break;
            };
            let gone = self.targets.swap_remove(i);
            if gone.kept {
                self.statics.forget_target(gone.id);
            }
            bytes -= gone.bytes();
        }
        for t in &mut self.targets {
            t.busy = t.kept;
        }
    }

    /// Keeps the targets `keep` says to; a cached generator's target that goes takes its cache entry with it.
    pub(super) fn retain_targets(&mut self, keep: impl Fn(&Target) -> bool) {
        let statics = &mut self.statics;
        self.targets.retain(|t| {
            let kept = keep(t);
            if !kept && t.kept {
                statics.forget_target(t.id);
            }
            kept
        });
    }

    /// The bytes the pool holds, cached generators' targets included.
    pub(super) fn target_bytes(&self) -> u64 {
        self.targets.iter().map(Target::bytes).sum()
    }

    /// The bytes cached generators' targets hold.
    pub(super) fn kept_bytes(&self) -> u64 {
        self.targets
            .iter()
            .filter(|t| t.kept)
            .map(Target::bytes)
            .sum()
    }

    /// The bytes `target` holds.
    pub(super) fn target_bytes_of(&self, target: usize) -> u64 {
        self.targets[target].bytes()
    }

    /// Frees `target` for the rest of the frame, unless a cached generator keeps it.
    pub(super) fn free(&mut self, target: usize) {
        let t = &mut self.targets[target];
        t.busy = t.kept;
    }

    /// A free target of `size` for a layer's pass ([`EffectStage::target`]), unless making one would take the pool past
    /// [`MAX_FRAME_TARGET_BYTES`] (a protective pass, `guarded`: [`MAX_PROTECTED_TARGET_BYTES`]): then None. The layer
    /// is drawn without that pass, said once in `warnings`; a protective pass's caller hides the layer instead.
    /// O(targets), only when no free target of that size is pooled.
    pub(crate) fn budgeted_target(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        warnings: &mut Vec<String>,
        guarded: bool,
    ) -> Option<usize> {
        let size = clamped(size);
        if !self.targets.iter().any(|t| !t.busy && t.size == size) {
            let held = self.target_bytes();
            let ceiling = if guarded {
                MAX_PROTECTED_TARGET_BYTES
            } else {
                MAX_FRAME_TARGET_BYTES
            };
            if held + bytes_of(size) > ceiling {
                if guarded {
                    return None;
                }
                if !self.over_budget_said {
                    self.over_budget_said = true;
                    warnings.push(OVER_BUDGET.into());
                }
                return None;
            }
        }
        Some(self.target(device, size))
    }

    /// [`EffectStage::budgeted_target`], made in the place of the target `old` when no free one of `size` is pooled and
    /// `old` is free: a scene's own target it outgrew, which would otherwise sit idle beside
    /// the new one until `evict_targets` (up to 9 frames, doubling a near-full-frame scene's peak). Made in its slot, so
    /// every index this frame handed out stays the same; the new target has an id of its own.
    pub(crate) fn budgeted_target_replacing(
        &mut self,
        device: &wgpu::Device,
        size: (u32, u32),
        warnings: &mut Vec<String>,
        old: Option<u64>,
    ) -> Option<usize> {
        let size = clamped(size);
        if self.targets.iter().any(|t| !t.busy && t.size == size) {
            return Some(self.target(device, size));
        }
        let slot = old.and_then(|id| self.targets.iter().position(|t| t.id == id && !t.busy));
        let freed = slot.map_or(0, |i| self.targets[i].bytes());
        if self.target_bytes() - freed + bytes_of(size) > MAX_FRAME_TARGET_BYTES {
            if !self.over_budget_said {
                self.over_budget_said = true;
                warnings.push(OVER_BUDGET.into());
            }
            return None;
        }
        let Some(i) = slot else {
            return Some(self.target(device, size));
        };
        self.targets[i] = self.made_target(device, size);
        PEAK.fetch_max(self.target_bytes(), Ordering::Relaxed);
        Some(i)
    }

    /// Whether `n` more targets of `size` fit the frame's budget for layers' passes (free ones of that size first), as
    /// [`EffectStage::budgeted_target`] would hand them out; when they don't, said once as it says it (a
    /// a scene's own blurs ask first, so a separable blur runs whole or not at all, and never take the
    /// headroom kept for protective passes). O(targets).
    pub(crate) fn room_for(
        &mut self,
        size: (u32, u32),
        n: usize,
        warnings: &mut Vec<String>,
    ) -> bool {
        let size = clamped(size);
        let free = self
            .targets
            .iter()
            .filter(|t| !t.busy && t.size == size)
            .count();
        let more = n.saturating_sub(free) as u64;
        let room = self.target_bytes() + more * bytes_of(size) <= MAX_FRAME_TARGET_BYTES;
        if !room && !self.over_budget_said {
            self.over_budget_said = true;
            warnings.push(OVER_BUDGET.into());
        }
        room
    }

    /// Keeps `target` past this frame for a still scene (`render::scene`), as the generator cache keeps a picture: its
    /// id, or None when the pool holds more than its between-frames budget or kept pictures more than their own
    /// (`MAX_CACHED_BYTES`), or in a timed frame (`fresh`: the next timed frame draws the scene again). No pass draws
    /// into it until [`EffectStage::release_held`].
    pub fn hold(&mut self, target: usize) -> Option<u64> {
        if self.fresh {
            return None;
        }
        let bytes = self.target_bytes_of(target);
        if self.target_bytes() > MAX_TARGET_BYTES
            || self.kept_bytes().saturating_add(bytes) > super::statics::MAX_CACHED_BYTES
        {
            return None;
        }
        let t = &mut self.targets[target];
        t.kept = true;
        Some(t.id)
    }

    /// The target held as `id`, marked used this frame; None once the pool dropped it (idle, over budget or trimmed),
    /// and in a timed frame: the render worker's timed frames, and every copy of a batch, draw a still scene's passes
    /// rather than show its held picture, so a batch's slope is a whole frame's (`fresh`, as
    /// note; `fresh`, as for a stepped generator's kept picture). The scene then lets its hold go.
    pub fn held(&mut self, id: u64) -> Option<usize> {
        if self.fresh {
            return None;
        }
        let now = self.frame;
        let i = self.targets.iter().position(|t| t.id == id && t.kept)?;
        self.targets[i].used = now;
        Some(i)
    }

    /// The target held as `id` goes back to the pool: free at once unless this frame used it.
    pub fn release_held(&mut self, id: u64) {
        self.let_go(id);
    }

    /// A free target of `size` for this frame, made or reused.
    pub fn target(&mut self, device: &wgpu::Device, size: (u32, u32)) -> usize {
        let size = clamped(size);
        if let Some(i) = self.targets.iter().position(|t| !t.busy && t.size == size) {
            let t = &mut self.targets[i];
            t.busy = true;
            t.used = self.frame;
            t.watched |= self.watching;
            return i;
        }
        let target = self.made_target(device, size);
        self.targets.push(target);
        PEAK.fetch_max(self.target_bytes(), Ordering::Relaxed);
        self.targets.len() - 1
    }

    /// A new busy target of `size`, used this frame, with an id of its own.
    fn made_target(&mut self, device: &wgpu::Device, size: (u32, u32)) -> Target {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Klipolo effect target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: EFFECT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let input = input_group(device, &self.input_layout, &view, &self.empty_view);
        made();
        self.targets_made += 1;
        Target {
            texture,
            view,
            input,
            size,
            used: self.frame,
            busy: true,
            id: self.targets_made,
            kept: false,
            watched: self.watching,
        }
    }
}
