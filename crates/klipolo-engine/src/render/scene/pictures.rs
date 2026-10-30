//! The pictures a scene's image nodes draw: the compositor decodes each one the layer's
//! settings name through the image cache (`SceneDrawer::textures`); here each gets its bind group for the shapes
//! pipeline (group 1: the picture and a trilinear sampler), made once per picture and kept while it is drawn. Runs of
//! shapes bind a transparent texel there.
//!
//! **Bounded.** At most [`MAX_PICTURES`] bind groups (the fill and backdrop targets among them); one unused for 8
//! frames goes, and so does the least recently used
//! past the bound (never one this frame drew). A lookup is O(pictures) and allocates nothing.

use super::shapes::ShapePipeline;
use crate::render::effects::{EFFECT_FORMAT, made};

/// The most pictures' bind groups kept (the layer's pictures, and a frame's generator fills' and backdrops' targets).
const MAX_PICTURES: usize = 32;
/// A picture's bind group unused for this many frames goes.
const EXPIRE_AFTER_FRAMES: u64 = 8;

#[derive(Default)]
pub(super) struct Pictures {
    /// (picture, its bind group, the frame it was last drawn).
    kept: Vec<(wgpu::Texture, wgpu::BindGroup, u64)>,
    /// This layer's: each picture's size and bind group, or None while it decodes (room kept between frames).
    pub(super) dims: Vec<Option<(u32, u32)>>,
    pub(super) groups: Vec<Option<wgpu::BindGroup>>,
}

impl Pictures {
    /// The layer's `textures` ready to draw at frame `now`: their sizes and bind groups (made for a new picture). True
    /// when every one is ready (a frame with one still decoding isn't held).
    pub(super) fn bind(
        &mut self,
        device: &wgpu::Device,
        pipeline: &ShapePipeline,
        textures: &[Option<wgpu::Texture>],
        now: u64,
    ) -> bool {
        self.dims.clear();
        self.groups.clear();
        let mut ready = true;
        for texture in textures {
            let Some(texture) = texture else {
                ready = false;
                self.dims.push(None);
                self.groups.push(None);
                continue;
            };
            let group = self.group_for(device, pipeline, texture, now);
            self.dims.push(Some((texture.width(), texture.height())));
            self.groups.push(Some(group));
        }
        ready
    }

    /// `texture`'s bind group (made the first time, then kept while it is drawn): a picture's, or a generator fill's or
    /// a backdrop's pooled target (the pool hands the same target out again, so its group is found again).
    pub(super) fn group_for(
        &mut self,
        device: &wgpu::Device,
        pipeline: &ShapePipeline,
        texture: &wgpu::Texture,
        now: u64,
    ) -> wgpu::BindGroup {
        if let Some((_, group, used)) = self.kept.iter_mut().find(|(t, _, _)| t == texture) {
            *used = now;
            return group.clone();
        }
        if self.kept.len() >= MAX_PICTURES
            && let Some(oldest) = (0..self.kept.len())
                .filter(|&i| self.kept[i].2 < now)
                .min_by_key(|&i| self.kept[i].2)
        {
            self.kept.swap_remove(oldest);
        }
        let group = pipeline.pictures.group(device, texture);
        self.kept.push((texture.clone(), group.clone(), now));
        group
    }

    /// `targets`' bind groups (the frame's generator fills' or backdrops' targets in `stage`, None where one didn't
    /// draw) into `out`, kept as a picture's are.
    pub(super) fn bind_targets(
        &mut self,
        (device, pipeline): (&wgpu::Device, &ShapePipeline),
        stage: &crate::render::effects::EffectStage,
        now: u64,
        targets: &[Option<usize>],
        out: &mut Vec<Option<wgpu::BindGroup>>,
    ) {
        out.clear();
        for t in targets {
            out.push(t.map(|t| self.group_for(device, pipeline, stage.texture(t), now)));
        }
    }

    /// The frame's start: pictures unused for a while let go of their bind groups (and so their textures).
    pub(super) fn expire(&mut self, now: u64) {
        self.kept
            .retain(|(_, _, used)| now - used <= EXPIRE_AFTER_FRAMES);
    }

    pub(super) fn clear(&mut self) {
        self.kept.clear();
        self.groups.clear();
    }
}

/// How a picture is bound to the shapes pipeline: its layout, the sampler, and the transparent texel runs of shapes bind.
pub(super) struct PictureBinding {
    pub(super) layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pub(super) none: wgpu::BindGroup,
}

impl PictureBinding {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Klipolo scene picture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // Pictures are mipmapped (the image cache's): sampled trilinear by their own level.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Klipolo scene picture"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // wgpu zero-initializes textures: a transparent texel.
        let blank = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Klipolo scene no picture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: EFFECT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = blank.create_view(&wgpu::TextureViewDescriptor::default());
        let none = bind(device, &layout, &view, &sampler);
        for _ in 0..3 {
            made();
        }
        Self {
            layout,
            sampler,
            none,
        }
    }

    /// The bind group `picture` draws with (made once per picture: `Pictures` keeps it).
    pub(super) fn group(&self, device: &wgpu::Device, picture: &wgpu::Texture) -> wgpu::BindGroup {
        made();
        let view = picture.create_view(&wgpu::TextureViewDescriptor::default());
        bind(device, &self.layout, &view, &self.sampler)
    }
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Klipolo scene picture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
