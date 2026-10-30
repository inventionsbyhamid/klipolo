//! The GPU cursor pass: each planned `Layer::Cursor` drawn in the frame's own render pass, in
//! plan order, as instanced quads of one pipeline with **one bind group** (made again only when the atlas is made or
//! let go), scissored to the footage's box.
//!
//! **Two slots a frame** (a transition plans two cursor layers), each a header and up to `INSTANCES_PER_SLOT`
//! instances (four ripples under the cursor), written at its own place in a ring of `FRAMES_IN_FLIGHT` frames
//! (2,304 bytes), uploaded once a frame (`finish_frame`). A third layer, which no plan makes today, is skipped and
//! said once.
//!
//! **Shapes** live in the atlas (`atlas`). The preview decodes them on a thread of its own and uploads them between
//! frames (`upload_ready`); a frame whose shape is still loading draws the Arrow look. Export and
//! headless renders preload the shapes in range before frame 1, as many as the atlas holds (`preload`, from
//! `export::prepare::prepare_cursors`, which checks the rest), and a strict frame loads any it still misses before it
//! draws (`ensure`), so they never draw the stand-in. A shape whose image doesn't decode is handed to its take
//! (`take_refused`), which plans the Arrow look for it from then on. Dot and Arrow are signed distances in the shader:
//! sharp at any size, no texture.
//!
//! **Per frame:** `prepare` is a lookup and a few entries written into the ring's mirror; `draw` five pass commands.
//! Neither allocates. **Memory:** the atlas (5.6 MB, once a shape arrives) and the ring, released by `forget`.

mod atlas;
mod instances;
mod load;
mod pack;
mod pipeline;
mod prepare;
mod texture;

use std::sync::Arc;

use tokio::sync::Notify;

pub use atlas::{CursorAtlas, Slot};
pub use instances::{DOT_SIZE, Look};
pub use prepare::{CursorShapeError, READY_MAX, ShapeImage, decode};
pub use texture::{ATLAS_BYTES, MIP_LEVELS};

use crate::plan::{CursorDraw, CursorLayer};

pub const SLOTS_PER_FRAME: usize = 2;
/// The cursor and up to four ripples.
pub const INSTANCES_PER_SLOT: usize = 5;
const FRAMES_IN_FLIGHT: usize = 3;
/// A slot's header, then its instances.
const ENTRIES_PER_SLOT: usize = 1 + INSTANCES_PER_SLOT;
const RING: usize = FRAMES_IN_FLIGHT * SLOTS_PER_FRAME * ENTRIES_PER_SLOT;
/// The ring's bytes on the GPU.
pub const RING_BYTES: usize = RING * size_of::<Instance>();

/// One entry of the ring (64 bytes, 16 × f32): an instance (a cursor or a ripple) or a slot's header. `cursor.wgsl`
/// says what each field holds.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub head: [f32; 4],
    pub place: [f32; 4],
    pub image: [f32; 4],
    pub more: [f32; 4],
}

/// One slot's draw: its instances in the ring, its scissor (x, y, width, height) and the target's size (the scissor
/// is reset to it after the draw, for the layers after the cursor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawRange {
    pub first: u32,
    pub count: u32,
    pub scissor: [u32; 4],
    pub target: (u32, u32),
}

pub struct CursorPass {
    made: pipeline::Made,
    ring: wgpu::Buffer,
    placeholder: wgpu::TextureView,
    bind: wgpu::BindGroup,
    /// The atlas's epoch `bind` was made for.
    bound: u64,
    atlas: CursorAtlas,
    /// The ring as this side writes it; the frame's slots go up in `finish_frame`.
    entries: [Instance; RING],
    /// The ring's frame (0..FRAMES_IN_FLIGHT) and the slots it used.
    frame: usize,
    used: usize,
    told_third: bool,
}

impl CursorPass {
    /// The pipeline, the ring and the bind group (with a 1 × 1 placeholder until a shape arrives). `redraw` fires
    /// when a decoded shape is ready, so the preview draws again with it.
    pub fn new(device: &wgpu::Device, redraw: Arc<Notify>) -> Self {
        let made = pipeline::make(device);
        let ring = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Klipolo cursor ring"),
            size: RING_BYTES as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let placeholder = pipeline::placeholder(device);
        let bind = pipeline::bind(device, &made, &placeholder, &ring);
        Self {
            atlas: CursorAtlas::new(device, redraw),
            made,
            ring,
            placeholder,
            bind,
            bound: 0,
            entries: [Instance::default(); RING],
            frame: 0,
            used: 0,
            told_third: false,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame = (self.frame + 1) % FRAMES_IN_FLIGHT;
        self.used = 0;
        self.atlas.begin_frame();
    }

    /// Writes `layer`'s instances into the frame's next free slot. A shape not resident draws the Arrow look and is
    /// asked for (preview: once a shape, the only time this allocates); with `strict` (export, headless) that is a
    /// bug, as `preload` and `ensure` ran, unless its image can't be used. None when nothing shows.
    pub fn prepare(
        &mut self,
        layer: &CursorLayer,
        target: (u32, u32),
        strict: bool,
    ) -> Option<DrawRange> {
        if self.used == SLOTS_PER_FRAME {
            if !self.told_third {
                self.told_third = true;
                tracing::warn!("a third cursor layer in one frame: not drawn");
            }
            return None;
        }
        let scissor = instances::scissor(layer, target)?;
        let header = (self.frame * SLOTS_PER_FRAME + self.used) * ENTRIES_PER_SLOT;
        self.entries[header] = instances::header(layer, target);
        let mut n = 0;
        for r in layer.active_ripples() {
            n += 1;
            self.entries[header + n] = instances::ripple(r, header, target);
        }
        if layer.opacity > 0.0 {
            let look = self.look(&layer.look, strict);
            n += 1;
            self.entries[header + n] = instances::cursor(layer, look, header, target);
        }
        (n > 0).then(|| {
            self.used += 1;
            DrawRange {
                first: (header + 1) as u32,
                count: n as u32,
                scissor,
                target,
            }
        })
    }

    fn look(&mut self, draw: &CursorDraw, strict: bool) -> Look {
        match draw {
            CursorDraw::Shape {
                take,
                shape,
                file,
                hotspot,
                aspect,
            } => match self.atlas.slot(take, *shape) {
                Some(slot) => Look::Shape {
                    rect: slot.rect,
                    hotspot: *hotspot,
                    aspect: *aspect,
                },
                None => {
                    debug_assert!(
                        !strict || self.atlas.failed(take, *shape),
                        "a strict frame's shapes are loaded first"
                    );
                    // Asked for once; one refused already goes back to its take.
                    self.atlas.request(take.clone(), *shape, file.clone());
                    Look::Arrow
                }
            },
            CursorDraw::Dot => Look::Dot,
            CursorDraw::Arrow => Look::Arrow,
        }
    }

    /// Draws one slot in `pass`, then gives the pass its whole target back. Five commands, no allocation.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, range: DrawRange) {
        let [x, y, w, h] = range.scissor;
        pass.set_pipeline(&self.made.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_scissor_rect(x, y, w, h);
        pass.draw(0..6, range.first..range.first + range.count);
        pass.set_scissor_rect(0, 0, range.target.0, range.target.1);
    }

    /// This frame's slots go up to the GPU (one write; before the frame's commands are submitted).
    pub fn finish_frame(&self, queue: &wgpu::Queue) {
        let from = self.frame * SLOTS_PER_FRAME * ENTRIES_PER_SLOT;
        let to = from + self.used * ENTRIES_PER_SLOT;
        if to > from {
            let offset = (from * size_of::<Instance>()) as u64;
            queue.write_buffer(
                &self.ring,
                offset,
                bytemuck::cast_slice(&self.entries[from..to]),
            );
        }
    }

    /// `take`'s shape `shape` is in the atlas.
    pub fn resident(&self, take: &Arc<str>, shape: u16) -> bool {
        self.atlas.resident(take, shape)
    }

    /// `take`'s shape `shape` was refused (its image can't be used): it draws the Arrow look.
    pub fn failed(&self, take: &Arc<str>, shape: u16) -> bool {
        self.atlas.failed(take, shape)
    }

    /// Some shape the preview asked for hasn't arrived yet (decoding, or waiting for the next frame's upload).
    pub fn shapes_waiting(&self) -> bool {
        self.atlas.waiting()
    }

    /// The pass's GPU memory: the atlas (once a shape arrived), the ring and the placeholder.
    pub fn gpu_bytes(&self) -> usize {
        self.atlas.texture.bytes() + RING_BYTES + 4
    }
}
