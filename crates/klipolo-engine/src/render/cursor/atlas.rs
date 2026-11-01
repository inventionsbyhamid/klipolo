//! The cursor shapes on the GPU: which take's shape is where in the atlas (`texture`),
//! asked for, resident, or refused (drawn as the Arrow look: only an image that can't be used, never for want of
//! room). Each shape has its own cell (`pack`); when none is free, the least recently drawn go (`evict`), never one
//! drawn in this frame, and a preload makes no room at all. The preview's shapes come from the decoder's thread
//! (`prepare`), export's are decoded inline (`load_now`).

use std::collections::HashMap;
use std::sync::Arc;

use super::pack::{Cell, Packer, SIDE};
use super::prepare::{CursorShapeError, Decoded, Decoder, ShapeImage};
use super::texture::AtlasTexture;

mod evict;

/// Where a resident shape is: its image's rectangle in the atlas, 0–1 (u0, v0, u1, v1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub rect: [f32; 4],
}

enum State {
    /// Asked for; decoding on the decoder's thread.
    Pending,
    /// Its image can't be used: it draws the Arrow look.
    Failed,
    Resident(Cell, Slot),
}

struct Entry {
    state: State,
    /// When it was last drawn or uploaded (`tick`): the least recent goes first.
    used: u64,
    /// The frame it was last drawn in: never let go in that frame.
    drawn: u64,
}

pub struct CursorAtlas {
    pub texture: AtlasTexture,
    packer: Packer,
    slots: HashMap<(Arc<str>, u16), Entry>,
    tick: u64,
    frame: u64,
    decoder: Decoder,
    /// Reused for each batch from the decoder.
    batch: Vec<Decoded>,
    /// Shapes refused since the last `take_refused`, for their takes to plan the Arrow look.
    refused: Vec<(Arc<str>, u16)>,
}

impl CursorAtlas {
    pub const SIZE: u32 = SIDE;

    pub fn new(device: &wgpu::Device, redraw: Arc<tokio::sync::Notify>) -> Self {
        Self {
            texture: AtlasTexture::new(device),
            packer: Packer::default(),
            slots: HashMap::with_capacity(64),
            tick: 0,
            // Shapes put in before the first frame (a preload) are never "drawn this frame".
            frame: 1,
            decoder: Decoder::new(redraw),
            batch: Vec::with_capacity(super::prepare::READY_MAX),
            refused: Vec::with_capacity(4),
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// Where `take`'s shape `shape` is, if it is resident; drawn now (the least recently drawn go first). O(1).
    pub fn slot(&mut self, take: &Arc<str>, shape: u16) -> Option<Slot> {
        self.tick += 1;
        let e = self.slots.get_mut(&(take.clone(), shape))?;
        let State::Resident(_, slot) = e.state else {
            return None;
        };
        (e.used, e.drawn) = (self.tick, self.frame);
        Some(slot)
    }

    fn state(&self, take: &Arc<str>, shape: u16) -> Option<&State> {
        self.slots.get(&(take.clone(), shape)).map(|e| &e.state)
    }

    /// Its image can't be used (it draws the Arrow look).
    pub fn failed(&self, take: &Arc<str>, shape: u16) -> bool {
        matches!(self.state(take, shape), Some(State::Failed))
    }

    pub fn resident(&self, take: &Arc<str>, shape: u16) -> bool {
        matches!(self.state(take, shape), Some(State::Resident(..)))
    }

    /// Some shape asked for is still being decoded, or waits for the next frame's upload. O(shapes).
    pub fn waiting(&self) -> bool {
        self.slots
            .values()
            .any(|e| matches!(e.state, State::Pending))
    }

    /// Queues a shape's PNG for decoding on the decoder's thread (once: again only after it was let go). One refused
    /// already is handed back once more (`take_refused`): its take may have been loaded again since, and forgotten it.
    pub fn request(&mut self, take: Arc<str>, shape: u16, png: Arc<str>) {
        let key = (take.clone(), shape);
        if let Some(e) = self.slots.get(&key) {
            if matches!(e.state, State::Failed) && !self.refused.contains(&key) {
                self.refused.push(key);
            }
            return;
        }
        let (state, used, drawn) = (State::Pending, 0, 0);
        self.slots.insert(key, Entry { state, used, drawn });
        self.decoder.submit(take, shape, png);
    }

    /// Decodes `png` now and puts it in (export, headless and a strict frame's miss), letting shapes go for room only
    /// with `evict`. `Ok(false)`: no room, left out (`install`).
    pub fn load_now(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        take: &Arc<str>,
        shape: u16,
        png: &str,
        evict: bool,
    ) -> Result<bool, CursorShapeError> {
        let image =
            super::prepare::decode(std::path::Path::new(&**take), std::path::Path::new(png));
        self.install(device, queue, take.clone(), shape, image, evict)
    }

    /// The shapes the decoder finished since the last frame go in. True if any did.
    pub fn upload_ready(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let mut batch = std::mem::take(&mut self.batch);
        self.decoder.take_ready(&mut batch);
        let any = !batch.is_empty();
        for d in batch.drain(..) {
            // Let go meanwhile (the atlas was forgotten): asked for again when drawn.
            if self.slots.contains_key(&(d.take.clone(), d.shape)) {
                let _ = self.install(device, queue, d.take, d.shape, d.image, true);
            }
        }
        self.batch = batch;
        any
    }

    /// Makes and submits the mips of what went in since the last time (`AtlasTexture::finish`).
    pub fn finish_uploads(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.texture.finish(device, queue);
    }

    /// Puts a decoded (or refused) shape in. Only an image that can't be used is refused (Failed), never one for want
    /// of room: with no room (`evict` off, or every shape drawn this frame) it is left out, `Ok(false)`,
    /// and asked for again when drawn.
    fn install(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        take: Arc<str>,
        shape: u16,
        image: Result<ShapeImage, CursorShapeError>,
        evict: bool,
    ) -> Result<bool, CursorShapeError> {
        let key = (take, shape);
        let image = match image {
            Ok(image) => image,
            Err(e) => {
                self.refused.push(key.clone());
                self.put(key, State::Failed);
                return Err(e);
            }
        };
        let Some(cell) = self.cell_for(&image, evict) else {
            if let Some(Entry {
                state: State::Pending,
                ..
            }) = self.slots.get(&key)
            {
                self.slots.remove(&key);
            }
            return Ok(false);
        };
        self.texture.write(device, queue, cell, &image);
        let (x, y, s) = (cell.x as f32, cell.y as f32, SIDE as f32);
        let (w, h) = (image.width as f32, image.height as f32);
        let rect = [x / s, y / s, (x + w) / s, (y + h) / s];
        self.put(key, State::Resident(cell, Slot { rect }));
        Ok(true)
    }

    fn put(&mut self, key: (Arc<str>, u16), state: State) {
        self.tick += 1;
        let (used, drawn) = (self.tick, 0);
        if let Some(Entry {
            state: State::Resident(old, _),
            ..
        }) = self.slots.insert(key, Entry { state, used, drawn })
        {
            self.packer.give(old);
        }
    }

    /// `f` for each shape refused since the last call (its image can't be used), oldest first. No allocation.
    pub fn take_refused(&mut self, mut f: impl FnMut(&Arc<str>, u16)) {
        for (take, shape) in self.refused.drain(..) {
            f(&take, shape);
        }
    }

    /// Lets go of the texture and every shape (the project closed or the window hid); the pipeline stays.
    pub fn forget(&mut self) {
        self.texture.forget();
        self.slots.clear();
        self.refused.clear();
        self.packer = Packer::default();
        self.decoder.forget();
    }
}
