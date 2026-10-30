//! The cursor atlas's texture: 1024² RGBA8 with five mip levels (5.6 MB), made when the first shape
//! arrives and let go with the project. A shape goes in whole: its cell's level 0, the rest of the cell cleared, so
//! nothing of an earlier shape stays in the mips. The mips are made on the GPU (`render/mips.rs`) once per batch and
//! submitted at once, ahead of the frame that draws them.

use std::sync::Arc;

use super::pack::{Cell, SIDE};
use super::prepare::ShapeImage;
use crate::render::mips::MipGenerator;

/// Mip levels kept: cells are at least 16 px and aligned to their size, so up to level 4 no texel mixes two cells.
pub const MIP_LEVELS: u32 = 5;
/// The atlas's bytes on the GPU, its mips included.
pub const ATLAS_BYTES: usize = {
    let mut bytes = 0;
    let mut level = 0;
    while level < MIP_LEVELS {
        let side = (SIDE >> level) as usize;
        bytes += side * side * 4;
        level += 1;
    }
    bytes
};

pub struct AtlasTexture {
    texture: Option<wgpu::Texture>,
    /// Bumped each time the texture is made or let go (the pass's bind group follows it).
    pub epoch: u64,
    mips: Arc<MipGenerator>,
    /// Shapes went in since the mips were last made.
    dirty: bool,
}

impl AtlasTexture {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            texture: None,
            epoch: 0,
            mips: MipGenerator::new(device),
            dirty: false,
        }
    }

    /// Its view, if it exists.
    pub fn view(&self) -> Option<wgpu::TextureView> {
        let texture = self.texture.as_ref()?;
        Some(texture.create_view(&wgpu::TextureViewDescriptor::default()))
    }

    pub fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cell: Cell,
        image: &ShapeImage,
    ) {
        let texture = &*self.texture.get_or_insert_with(|| {
            self.epoch += 1;
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Klipolo cursor atlas"),
                size: wgpu::Extent3d {
                    width: SIDE,
                    height: SIDE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: MIP_LEVELS,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let side = cell.side as usize;
        let mut padded = vec![0u8; side * side * 4];
        let row = image.width as usize * 4;
        for (y, src) in image.rgba.chunks_exact(row).enumerate() {
            padded[y * side * 4..y * side * 4 + row].copy_from_slice(src);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: cell.x,
                    y: cell.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &padded,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(cell.side * 4),
                rows_per_image: Some(cell.side),
            },
            wgpu::Extent3d {
                width: cell.side,
                height: cell.side,
                depth_or_array_layers: 1,
            },
        );
        self.dirty = true;
    }

    /// Makes the mips of what went in since the last time and submits them now: by queue order before the frame that
    /// draws them, and a frame dropped before its own submit never leaves stale levels behind.
    pub fn finish(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let (true, Some(texture)) = (self.dirty, &self.texture) {
            self.mips.generate(device, queue, texture);
            self.dirty = false;
        }
    }

    /// Lets the texture go.
    pub fn forget(&mut self) {
        if self.texture.take().is_some() {
            self.epoch += 1;
        }
        self.dirty = false;
    }

    /// Its bytes on the GPU (0 until a shape arrives).
    pub fn bytes(&self) -> usize {
        if self.texture.is_some() {
            ATLAS_BYTES
        } else {
            0
        }
    }
}
