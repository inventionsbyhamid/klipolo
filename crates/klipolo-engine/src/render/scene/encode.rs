//! A scene's frame encoded: its runs in paint order into its
//! target, where each offscreen node (`Run::Begin` … `Run::End`) draws into a pooled target only as big as its own box
//! (`sizes.rs`; its slot tells the scene's pipelines where that target sits), which its effects then lay back into the
//! target it began in (`encode_lay.rs`: as a quad, or by a host pass); a masked node's mask is drawn just before, into
//! a target of the same box (at most one live). Each target comes from the stage's pool and goes back
//! as the passes read it (the stage counts what they hold: `EffectStage::watch`), and a pass that draws a translucent
//! stroke has a stencil of its target's size (no other pass has one). A node whose target (or mask) the
//! frame's budget refused isn't drawn: drawn in place it would show at full opacity and unclipped. Fixed-size
//! stacks: nothing allocates.

use std::sync::Arc;

use super::composite::Composites;
use super::groups::MAX_ISOS;
use super::pack::{Packed, Run};
use super::quad_lay::QuadLay;
use super::sizes::Placed;
use super::slots::SLOTS;
use super::stencils::{self, Stencils};
use super::target::{self, Onto, Pass};
use crate::registry::ShaderEffect;
use crate::render::effects::{EffectStage, FrameInfo};

/// What the frame's own passes draw with besides the scene's pass: the group passes, the scene's blur (a twin of the
/// built-in blur that isn't protective), where the scene's own target sits in the layer's (`root`: only as big as
/// what it draws), and the quad lay-back with the instance's slots bound for it.
pub(super) struct With<'a> {
    pub pass: &'a Pass<'a>,
    pub counts: (u32, u32, u32),
    pub composites: &'a Composites,
    pub blur: Option<&'a Arc<ShaderEffect>>,
    pub root: Placed,
    pub info: FrameInfo,
    pub quad: Option<(&'a QuadLay, &'a wgpu::BindGroup)>,
}

/// The frame's GPU side while it encodes: the stage and its pool, the scene's stencils, the frame number.
pub(super) struct Enc<'a, 'b> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub stage: &'a mut EffectStage,
    pub stencils: &'a mut Stencils,
    pub now: u64,
    pub warnings: &'a mut Vec<String>,
    pub with: &'a With<'b>,
    pub tally: Tally,
    /// Nodes laid back as quads (each one pass of its own box).
    pub lays: u32,
}

/// The sizes the frame's passes took stencils at (each one's bytes in `SceneBytes::buffers`).
#[derive(Clone, Copy, Default)]
pub(super) struct Tally {
    sizes: [(u32, u32); SLOTS],
    n: usize,
}

impl Tally {
    fn note(&mut self, size: (u32, u32)) {
        if !self.sizes[..self.n].contains(&size) && self.n < SLOTS {
            self.sizes[self.n] = size;
            self.n += 1;
        }
    }

    /// The bytes of the stencils the frame drew with.
    pub(super) fn stencil_bytes(&self) -> u64 {
        self.sizes[..self.n]
            .iter()
            .map(|&s| stencils::bytes_for(s))
            .sum()
    }
}

/// What encoding a frame came to: the target holding the scene, the offscreen nodes drawn, and whether one was laid
/// back into the scene's own target by a host pass (whose result is the scene's ping-pong partner).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Encoded {
    pub target: usize,
    pub offscreen: u32,
    pub laid: bool,
}

/// One target being drawn: its index, whether a pass has cleared it, whether its runs are skipped (its node's own
/// target, or one around it, was refused), its slot and where it sits in the scene's target.
#[derive(Clone, Copy)]
pub(super) struct Open {
    pub target: usize,
    pub cleared: bool,
    pub skip: bool,
    pub slot: usize,
    pub at: Placed,
}

/// `packed`'s frame encoded into `out` (the scene's own target, placed at `with.root`).
pub(super) fn program(enc: &mut Enc, packed: &Packed, out: usize) -> Encoded {
    let runs = &packed.runs;
    let split = packed.mask_from.min(runs.len());
    let main = &runs[..split];
    let mut e = Encoded::default();
    let mut stack = [Open {
        target: out,
        cleared: false,
        skip: false,
        slot: 0,
        at: enc.with.root,
    }; MAX_ISOS + 1];
    let mut depth = 1usize;
    let mut from = 0usize;
    for (j, run) in main.iter().enumerate() {
        match *run {
            Run::Begin(slot) => {
                enc.flush(&mut stack[depth - 1], &main[from..j]);
                from = j + 1;
                debug_assert!(depth <= MAX_ISOS, "pack opens at most MAX_ISOS");
                if depth > MAX_ISOS {
                    continue;
                }
                let top = stack[depth - 1];
                let at = packed.isos.get(usize::from(slot)).map(|iso| iso.target);
                let own = at.filter(|_| !top.skip).and_then(|at| {
                    let t = enc
                        .stage
                        .budgeted_target(enc.device, at.size, enc.warnings, false);
                    t.map(|t| (t, at))
                });
                stack[depth] = match own {
                    Some((target, at)) => {
                        e.offscreen += 1;
                        Open {
                            target,
                            cleared: false,
                            skip: false,
                            slot: usize::from(slot) + 1,
                            at,
                        }
                    }
                    None => Open { skip: true, ..top },
                };
                depth += 1;
            }
            Run::End(slot) => {
                enc.flush(&mut stack[depth - 1], &main[from..j]);
                from = j + 1;
                if depth <= 1 {
                    continue;
                }
                depth -= 1;
                let group = stack[depth];
                let Some(iso) = packed.isos.get(usize::from(slot)).filter(|_| !group.skip) else {
                    continue;
                };
                let parent = &mut stack[depth - 1];
                enc.flush(parent, &[]);
                // Its mask, now: at most one is held at a time. One the budget refused hides its node.
                let mask = match iso.mask {
                    Some(_) => match enc.draw_mask(&group, (runs, split, slot)) {
                        Some(t) => Some(t),
                        None => continue,
                    },
                    None => None,
                };
                if let Some(laid) = enc.lay_back(iso, (group, mask, *parent)) {
                    e.laid |= laid.hosted && depth == 1;
                    parent.target = laid.target;
                }
            }
            _ => {}
        }
    }
    enc.flush(&mut stack[depth - 1], &main[from..]);
    e.target = stack[0].target;
    e
}

impl Enc<'_, '_> {
    /// `runs` drawn into `open`'s target (clearing it first when nothing has); nothing when there is nothing to draw
    /// and it is cleared, or its runs are skipped.
    pub(super) fn flush(&mut self, open: &mut Open, runs: &[Run]) {
        if open.skip || (runs.is_empty() && open.cleared) {
            return;
        }
        // A stencil only for a pass that draws a translucent stroke.
        let stencil = target::needs_stencil(runs).then(|| {
            self.tally.note(open.at.size);
            self.stencils.view(self.device, open.at.size, self.now)
        });
        let onto = Onto {
            view: self.stage.view(open.target),
            stencil,
            slot: open.slot,
        };
        let w = self.with;
        target::encode(self.encoder, onto, w.pass, w.counts, runs, !open.cleared);
        open.cleared = true;
    }

    /// Offscreen node `slot`'s mask segment (after `split` in `runs`) drawn into a target of its node's box (`group`'s):
    /// None when it has none or the budget refused it.
    fn draw_mask(
        &mut self,
        group: &Open,
        (runs, split, slot): (&[Run], usize, u8),
    ) -> Option<usize> {
        let start = split + runs[split..].iter().position(|r| *r == Run::Mask(slot))?;
        let end = runs[start..]
            .iter()
            .position(|r| matches!(r, Run::MaskEnd(_)))
            .map_or(runs.len(), |e| start + e);
        let size = group.at.size;
        let target = self
            .stage
            .budgeted_target(self.device, size, self.warnings, false)?;
        let mut open = Open {
            target,
            cleared: false,
            ..*group
        };
        self.flush(&mut open, &runs[start + 1..end]);
        Some(target)
    }
}
