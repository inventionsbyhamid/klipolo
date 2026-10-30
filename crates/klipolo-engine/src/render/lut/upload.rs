//! LUT texels on the GPU: a `.cube` as an `Rgba16Float` 3D texture, and the bind group pairing it with a strength.

use wgpu::util::DeviceExt;

use super::CubeLut;
use crate::vendor::cap::rendering::composite_frame::CompositeVideoFramePipeline;

/// A LUT as `Rgba16Float` texels, ready to upload.
pub(super) struct CubeLutHalf {
    pub(super) size: u32,
    texels: Vec<half::f16>,
}

impl CubeLutHalf {
    pub(super) fn identity_1() -> Self {
        Self {
            size: 1,
            texels: vec![half::f16::ZERO; 4],
        }
    }
}

impl From<&CubeLut> for CubeLutHalf {
    fn from(lut: &CubeLut) -> Self {
        let texels = lut
            .data
            .iter()
            .flat_map(|[r, g, b]| [*r, *g, *b, 1.0])
            .map(half::f16::from_f32)
            .collect();
        Self {
            size: lut.size,
            texels,
        }
    }
}

/// Red runs along x, green along y and blue along z, so sampling at (r, g, b) finds the entry.
pub(super) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lut: &CubeLutHalf,
) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: lut.size,
        height: lut.size,
        depth_or_array_layers: lut.size,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("LUT"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // Queue writes land before the next submit, so the frame that first samples it sees the data.
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&lut.texels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(lut.size * 8),
            rows_per_image: Some(lut.size),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(super) fn bind_group(
    device: &wgpu::Device,
    pipeline: &CompositeVideoFramePipeline,
    sampler: &wgpu::Sampler,
    view: &wgpu::TextureView,
    params: [f32; 4],
) -> wgpu::BindGroup {
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("LUT params"),
        contents: bytemuck::cast_slice(&params),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("LUT bind group"),
        layout: &pipeline.lut_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: buffer.as_entire_binding(),
            },
        ],
    })
}
