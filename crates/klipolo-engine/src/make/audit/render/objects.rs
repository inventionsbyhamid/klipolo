//! The audit draw's GPU objects that don't depend on the draw: the effect stage's bind group layouts and the inputs.

use super::super::AUDIT_SIZE;
use crate::render::device::RenderDevice;

/// The effect stage's layouts (`EffectStage::new`): the frame group and the input group.
pub(super) fn layouts(
    device: &wgpu::Device,
) -> (
    wgpu::PipelineLayout,
    wgpu::BindGroupLayout,
    wgpu::BindGroupLayout,
) {
    let uniform = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let frame = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Klipolo audit frame"),
        entries: &[
            uniform(0),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            uniform(2),
        ],
    });
    let input = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Klipolo audit input"),
        entries: &[texture(0), texture(1)],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Klipolo audit"),
        bind_group_layouts: &[&frame, &input],
        push_constant_ranges: &[],
    });
    (layout, frame, input)
}

/// An input at `AUDIT_SIZE`: the draft's test pattern (its negative for a transition's second side), or one
/// transparent texel when the effect reads none.
pub(super) fn input(dev: &RenderDevice, reads: bool, second: bool) -> wgpu::Texture {
    let (w, h) = if reads { AUDIT_SIZE } else { (1, 1) };
    let texels: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            let v = if !reads {
                0
            } else if (x / 8 + y / 8) % 2 == 0 {
                128
            } else {
                255
            };
            let v = if second { 255 - v / 2 } else { v };
            [v, v, v, if reads { 255 } else { 0 }]
        })
        .collect();
    let texture = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Klipolo audit input"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    dev.queue.write_texture(
        texture.as_image_copy(),
        &texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    texture
}
