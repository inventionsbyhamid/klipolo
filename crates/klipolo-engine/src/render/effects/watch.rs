//! What a run of passes really holds of the pool, the pixels passes draw, and the pool's targets for a
//! pass of the stage's caller's own (the scene drawer's quad lay-back). While watching, each target handed out is
//! marked; the marked targets' bytes are what the pool holds for that run. The pool hands out the first free target of
//! a size, so the targets of each size marked are exactly the most of that size busy at once, the internal targets of a
//! separable blur or a halving among them: what a fresh pool allocates for the run, and what a warm one keeps for it.

use std::sync::atomic::{AtomicU64, Ordering};

use super::EffectStage;

/// Pixels every stage's passes have drawn (a diagnostic, as `gpu_objects_made`: what a frame's passes cost).
static PASS_PIXELS: AtomicU64 = AtomicU64::new(0);

/// How many pixels effect stages' passes (and the scene drawer's quad lay-backs) have drawn in this process (the
/// performance numbers' tests read it around a frame).
pub fn pass_pixels() -> u64 {
    PASS_PIXELS.load(Ordering::Relaxed)
}

/// One pass drawn over `drawn` (its size, or its inner rect's): the stage's, or a pass of its caller's own over one of
/// its targets (the scene drawer's quad lay-back).
pub(crate) fn count_pass(drawn: [f32; 2]) {
    let px = (drawn[0].max(0.0) * drawn[1].max(0.0)) as u64;
    PASS_PIXELS.fetch_add(px, Ordering::Relaxed);
}

impl EffectStage {
    /// Starts watching what the pool hands out, `already` (a busy target the run holds from before: the frame grabbed
    /// under a backdrop) counted with it.
    pub(crate) fn watch(&mut self, already: Option<usize>) {
        for t in &mut self.targets {
            t.watched = false;
        }
        if let Some(t) = already.and_then(|i| self.targets.get_mut(i)) {
            t.watched = true;
        }
        self.watching = true;
    }

    /// Stops watching: the bytes of the targets handed out since [`EffectStage::watch`] (and those it counted).
    pub(crate) fn watched(&mut self) -> u64 {
        self.watching = false;
        self.targets
            .iter()
            .filter(|t| t.watched)
            .map(|t| u64::from(t.size.0) * u64::from(t.size.1) * 4)
            .sum()
    }

    /// The layout a pass reads one of the pool's targets through (`kl_a`, and `kl_b` empty), for a pipeline of the
    /// caller's own that reads `input_group`.
    pub(crate) fn input_layout(&self) -> &wgpu::BindGroupLayout {
        &self.input_layout
    }

    /// `target` as a pass's input (`kl_a`), its bind group kept with it: no GPU object.
    pub(crate) fn input_group(&self, target: usize) -> &wgpu::BindGroup {
        &self.targets[target].input
    }

    /// The bytes the targets in `list` hold.
    pub(crate) fn pooled_bytes(&self, list: &[Option<usize>]) -> u64 {
        list.iter()
            .flatten()
            .map(|&t| self.target_bytes_of(t))
            .sum()
    }

    /// `target` back to the pool for the rest of the frame: a pass of the caller's own read it last.
    pub(crate) fn free_target(&mut self, target: usize) {
        self.free(target);
    }
}
