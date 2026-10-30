//! Where each of a frame's targets sits in the layer's target: one 256-byte uniform slot per target,
//! read at a dynamic offset. Slot 0 is the scene's own target, only as big as what it draws; slot
//! `s + 1` offscreen node `s`'s, which is only as big as its own box (`bounds.rs`). The scene's pipelines read a slot's
//! first part (`Globals` in `paint.wgsl`: the target's size and its top left in the layer's target), so a node draws
//! where it stands whichever target it lands in; the quad lay-back (`quad_lay.rs`) reads the rest: where the node's
//! target goes in the one it began in, its opacity and its clips. A frame writes the range of slots that changed in
//! one write, none when nothing did; made with the instance (no GPU object per frame).

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};

use super::groups::{Iso, MAX_ISOS};
use super::sizes::Placed;
use crate::render::effects::made;

/// The bytes between slots: what every device allows a dynamic offset to step by (WebGPU's largest alignment).
pub(super) const SLOT_BYTES: u64 = 256;
/// The scene's target and one per offscreen node a frame opens.
pub(super) const SLOTS: usize = 1 + MAX_ISOS;
/// The bytes the scene's pipelines read of a slot (`Globals`).
pub(super) const GLOBALS_BYTES: u64 = 16;
/// The bytes the quad lay-back reads of one (`Slot` in `quad_lay.wgsl`).
pub(super) const LAY_BYTES: u64 = 208;

/// One target's slot.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(super) struct Slot {
    /// The target's size, and its top left in the scene's target (pixels).
    pub size: [f32; 2],
    pub origin: [f32; 2],
    /// Its box in the target it is laid back into: x, y, w, h (that target's pixels).
    pub boxed: [f32; 4],
    /// That target's size, and its top left in the scene's target.
    pub parent: [f32; 4],
    /// Its opacity, then nothing.
    pub misc: [f32; 4],
    /// Its own clip and the two nearest around it, each scene target pixels → its owner's node space (`GpuClip`).
    pub clips: [[f32; 4]; 9],
    pad: [f32; 12],
}

impl Default for Slot {
    fn default() -> Self {
        Self::zeroed()
    }
}

impl Slot {
    /// A target of `size` whose top left is at `origin` in the scene's target.
    pub(super) fn at(size: (u32, u32), origin: [u32; 2]) -> Self {
        Self {
            size: [size.0 as f32, size.1 as f32],
            origin: origin.map(|v| v as f32),
            ..Self::default()
        }
    }
}

/// An instance's slots: the buffer, and what it holds.
pub(super) struct Slots {
    pub(super) buffer: wgpu::Buffer,
    kept: [Slot; SLOTS],
}

impl Slots {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Klipolo scene targets"),
            size: SLOTS as u64 * SLOT_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        made();
        Self {
            buffer,
            kept: [Slot::default(); SLOTS],
        }
    }

    /// A frame's slots: the scene's own target (`root`, slot 0) and each of its offscreen nodes' (`isos[s]` in slot
    /// `s + 1`): its target's place, its box in the target it is laid back into, that one's size and place, its opacity
    /// and its clips. The slots that changed are written in one write.
    pub(super) fn write_frame(&mut self, queue: &wgpu::Queue, root: Placed, isos: &[Iso]) {
        let mut changed: Option<(usize, usize)> = None;
        let mut put = |kept: &mut [Slot; SLOTS], i: usize, slot: Slot| {
            if i < SLOTS && kept[i] != slot {
                kept[i] = slot;
                changed = Some(changed.map_or((i, i), |(lo, hi)| (lo.min(i), hi.max(i))));
            }
        };
        put(&mut self.kept, 0, Slot::at(root.size, root.origin));
        for (s, iso) in isos.iter().enumerate() {
            let p = isos.get(usize::from(iso.parent)).map_or(root, |p| p.target);
            let g = iso.target;
            let mut clips = [[0.0; 4]; 9];
            for (k, c) in iso.lay.clips.iter().enumerate() {
                clips[3 * k..3 * k + 3].copy_from_slice(&[c.m, c.t, c.c]);
            }
            let f = |v: u32| v as f32;
            let slot = Slot {
                boxed: [
                    f(g.origin[0]) - f(p.origin[0]),
                    f(g.origin[1]) - f(p.origin[1]),
                    f(g.size.0),
                    f(g.size.1),
                ],
                parent: [f(p.size.0), f(p.size.1), f(p.origin[0]), f(p.origin[1])],
                misc: [iso.lay.opacity, 0.0, 0.0, 0.0],
                clips,
                ..Slot::at(g.size, g.origin)
            };
            put(&mut self.kept, s + 1, slot);
        }
        if let Some((lo, hi)) = changed {
            let at = lo as u64 * SLOT_BYTES;
            queue.write_buffer(&self.buffer, at, bytemuck::cast_slice(&self.kept[lo..=hi]));
        }
    }

    /// The first `bytes` of a slot, bound at a dynamic offset.
    pub(super) fn binding(&self, bytes: u64) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.buffer,
            offset: 0,
            size: NonZeroU64::new(bytes),
        })
    }

    /// The bytes they hold.
    pub(super) fn bytes() -> u64 {
        SLOTS as u64 * SLOT_BYTES
    }
}

/// The dynamic offset of slot `i`.
pub(super) fn offset(i: usize) -> u32 {
    (i as u64 * SLOT_BYTES) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_fills_its_256_bytes_and_the_lay_reads_its_first_208() {
        assert_eq!(std::mem::size_of::<Slot>() as u64, SLOT_BYTES);
        assert_eq!(std::mem::offset_of!(Slot, clips) as u64 + 144, LAY_BYTES);
    }
}
