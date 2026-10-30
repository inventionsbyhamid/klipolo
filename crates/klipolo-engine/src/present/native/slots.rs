//! The native presenter's slot textures. The compositor renders each frame into a free slot on the actor; the
//! presenter thread copies each one into a drawable of the layer, presented at its due time. Every hand-over is a
//! state change under one short lock, and nothing waits on the GPU: wgpu orders the slot's write and its copy on the
//! one queue.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::frames::trailer::FrameMeta;
use crate::present::pick::{Waiting, due_host_ns, next};

/// Every frame a native presenter dropped in this process: each ring's own `dropped`, counted again outside its lock
/// so the app's performance block reads it at summary time. Bumped only when a frame is dropped.
static DROPPED_TOTAL: AtomicU64 = AtomicU64::new(0);

/// The frames every native presenter in this process dropped so far.
pub fn dropped_total() -> u64 {
    DROPPED_TOTAL.load(Ordering::Relaxed)
}

fn dropped_one() {
    DROPPED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

/// The slot on screen, the one being drawn, and one waiting (playback renders about one frame ahead). With all three
/// busy, the oldest waiting frame makes way for the new one.
const SLOTS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Free,
    Writing,
    Waiting(Waiting),
    Shown,
}

struct Slot {
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    state: State,
    meta: FrameMeta,
}

#[derive(Default)]
struct Ring {
    size: (u32, u32),
    /// Bumped whenever the slots are made again (a new size): frames of an older generation are dropped.
    generation: u64,
    slots: Vec<Slot>,
    order: u64,
    /// Frames that never reached the screen (a newer one was due first, or the slot was needed).
    dropped: u64,
}

/// What the blit needs to read a slot: its bind group layout and sampler.
pub(crate) struct BlitInputs {
    pub layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
}

pub(crate) struct Slots {
    ring: Mutex<Ring>,
    inputs: BlitInputs,
}

/// A frame drawn into a slot, on its way to the presenter. Dropped before it was queued (an error, a reset), its slot
/// is free again.
pub struct NativeFrame {
    slots: Arc<Slots>,
    index: usize,
    generation: u64,
    view: wgpu::TextureView,
    queued: bool,
}

impl NativeFrame {
    /// The texture the compositor renders this frame into.
    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}

impl Drop for NativeFrame {
    fn drop(&mut self) {
        if !self.queued {
            let mut ring = self.slots.lock();
            if ring.generation == self.generation && ring.slots[self.index].state == State::Writing
            {
                ring.slots[self.index].state = State::Free;
            }
        }
    }
}

/// The frame chosen for the screen: what the blit reads, at what size, and what it shows.
#[derive(Clone)]
pub(crate) struct Taken {
    pub bind: wgpu::BindGroup,
    pub size: (u32, u32),
    pub meta: FrameMeta,
    /// When it is due on the host clock (0: at once).
    pub due_ns: u64,
}

impl Slots {
    pub(crate) fn new(inputs: BlitInputs) -> Arc<Self> {
        Arc::new(Self {
            ring: Mutex::new(Ring::default()),
            inputs,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A free slot of `size` to render the next frame into (slots are made again when the size changes).
    pub(crate) fn lease(self: &Arc<Self>, device: &wgpu::Device, size: (u32, u32)) -> NativeFrame {
        let mut ring = self.lock();
        if ring.size != size || ring.slots.is_empty() {
            ring.slots = (0..SLOTS).map(|_| self.make_slot(device, size)).collect();
            ring.size = size;
            ring.generation += 1;
        }
        let free = ring.slots.iter().position(|s| s.state == State::Free);
        let index = free.unwrap_or_else(|| {
            // The oldest waiting frame makes way (only when the presenter fell a frame or more behind).
            let oldest = ring
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, s)| match s.state {
                    State::Waiting(w) => Some((w.order, i)),
                    _ => None,
                })
                .min()
                .map(|(_, i)| i);
            ring.dropped += 1;
            dropped_one();
            oldest.unwrap_or(0)
        });
        ring.slots[index].state = State::Writing;
        NativeFrame {
            slots: self.clone(),
            index,
            generation: ring.generation,
            view: ring.slots[index].view.clone(),
            queued: false,
        }
    }

    fn make_slot(&self, device: &wgpu::Device, (width, height): (u32, u32)) -> Slot {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Klipolo native slot"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Klipolo native slot"),
            layout: &self.inputs.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.inputs.sampler),
                },
            ],
        });
        Slot {
            view,
            bind,
            state: State::Free,
            meta: FrameMeta::default(),
        }
    }

    /// Hands a rendered frame to the presenter. False when it is stale (the slots were made again meanwhile).
    pub(crate) fn queue(&self, mut frame: NativeFrame, meta: FrameMeta) -> bool {
        frame.queued = true;
        let mut ring = self.lock();
        if ring.generation != frame.generation || ring.slots[frame.index].state != State::Writing {
            return false;
        }
        ring.order += 1;
        let order = ring.order;
        let slot = &mut ring.slots[frame.index];
        slot.state = State::Waiting(Waiting {
            order,
            due_ns: due_host_ns(meta.due_ms),
        });
        slot.meta = meta;
        true
    }

    /// The frame to commit next (`pick::next`) and when it should reach the screen (`None`: at once). It becomes the
    /// shown slot; the slot shown before it and every older waiting frame are freed.
    pub(crate) fn take_next(
        &self,
        now_ns: u64,
        refresh_ns: u64,
        horizon_ns: u64,
    ) -> Option<(Taken, Option<u64>)> {
        let mut ring = self.lock();
        // Arrays, not a Vec: this runs for every frame.
        let mut list = [Waiting {
            order: 0,
            due_ns: 0,
        }; SLOTS];
        let mut index = [0; SLOTS];
        let mut n = 0;
        for (i, slot) in ring.slots.iter().enumerate() {
            if let State::Waiting(w) = slot.state {
                list[n] = w;
                index[n] = i;
                n += 1;
            }
        }
        let (k, at) = next(&list[..n], now_ns, refresh_ns, horizon_ns)?;
        let chosen = index[k];
        let Waiting { order, due_ns } = list[k];
        let mut dropped = 0;
        for slot in &mut ring.slots {
            match slot.state {
                State::Shown => slot.state = State::Free,
                State::Waiting(w) if w.order < order => {
                    slot.state = State::Free;
                    dropped += 1;
                    dropped_one();
                }
                _ => {}
            }
        }
        ring.dropped += dropped;
        let size = ring.size;
        let slot = &mut ring.slots[chosen];
        slot.state = State::Shown;
        let taken = Taken {
            bind: slot.bind.clone(),
            size,
            meta: slot.meta,
            due_ns,
        };
        Some((taken, at))
    }

    /// When the earliest waiting frame is due (host ns; 0 = at once), if any is waiting.
    pub(crate) fn next_due(&self) -> Option<u64> {
        self.lock()
            .slots
            .iter()
            .filter_map(|s| match s.state {
                State::Waiting(w) => Some(w.due_ns),
                _ => None,
            })
            .min()
    }

    /// Frames that never reached the screen.
    pub(crate) fn dropped(&self) -> u64 {
        self.lock().dropped
    }

    /// Lets the slot textures go (the window was hidden); the next frame makes them again.
    pub(crate) fn release(&self) {
        let mut ring = self.lock();
        if ring.slots.iter().all(|s| s.state != State::Writing) {
            ring.slots.clear();
            ring.size = (0, 0);
            ring.generation += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_frame_counts_in_the_process_total() {
        let before = dropped_total();
        dropped_one();
        assert!(dropped_total() > before);
    }
}
