// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/layers/background.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted; prepare takes Klipolo's BackgroundPlan and the output size (Solid -> Color with alpha 1, Gradient -> Gradient, Image -> image cover) instead of ProjectUniforms + Cap's Background, and re-fits a cached image when the output size changes; prepare_black and forget added; a failed image draws black and returns ImageLoadError carrying the user-facing message (not retried until forget); images decode through Klipolo's `render::image_load` (EXIF orientation, HEIC via ImageIO) instead of `image::open`; AnimatedGradient, Wallpaper, clean_background_path, BackgroundTextureCache (load_background_texture keeps its decode-and-upload, uncached), From<BackgroundSource>, from_source, static_generation, render_surface, serde/specta derives and the BackgroundSource tests removed; image paths are shared `Arc<str>` (from the plan) instead of a String copied per frame. `ImageLoadError` carries the typed `image_load::ImageError`, so a missing background is said as a missing video is.

use bytemuck::{Pod, Zeroable};
use wgpu::{include_wgsl, util::DeviceExt};

use crate::plan::BackgroundPlan;
use crate::vendor::cap::rendering::{RenderingError, create_shader_render_pipeline};

const MAX_BACKGROUND_DIMENSION: u32 = 2560;

fn decode_background_rgba(
    path: &str,
    max_dimension: u32,
) -> Result<(Vec<u8>, u32, u32), crate::render::image_load::ImageError> {
    let img = crate::render::image_load::decode_rgba(std::path::Path::new(path), max_dimension)?;
    let (width, height) = img.dimensions();
    Ok((img.into_raw(), width, height))
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub struct Gradient {
    start: [f32; 4],
    end: [f32; 4],
    angle: f32,
    noise_intensity: f32,
    noise_scale: f32,
}

#[derive(PartialEq)]
pub enum ColorOrGradient {
    Color([f32; 4]),
    Gradient(Gradient),
}

#[derive(Debug, Clone)]
pub enum Background {
    Color([f32; 4]),
    Gradient(Gradient),
    Image { path: std::sync::Arc<str> },
}

pub enum Inner {
    Image {
        path: std::sync::Arc<str>,
        output_size: (u32, u32),
        texture: wgpu::Texture,
        bind_group: wgpu::BindGroup,
    },
    ColorOrGradient {
        value: ColorOrGradient,
        #[allow(unused)]
        buffer: wgpu::Buffer,
        bind_group: wgpu::BindGroup,
    },
}

pub struct BackgroundLayer {
    inner: Option<Inner>,
    /// An image that couldn't be loaded and why; it isn't decoded again until `forget`.
    failed: Option<(std::sync::Arc<str>, crate::render::image_load::ImageError)>,
    image_pipeline: ImageBackgroundPipeline,
    color_pipeline: GradientOrColorPipeline,
}

impl BackgroundLayer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            inner: None,
            failed: None,
            image_pipeline: ImageBackgroundPipeline::new(device),
            color_pipeline: GradientOrColorPipeline::new(device),
        }
    }

    /// Drops the image texture (the project closed or changed) and allows failed images to load again.
    pub fn forget(&mut self) {
        self.inner = None;
        self.failed = None;
    }

    /// Prepares Klipolo's background: Solid draws `Color { value, alpha: 1 }`, Gradient draws `Gradient { from, to,
    /// angle }` and Image covers the output. An image that can't be loaded draws black and returns the error.
    pub async fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        background: &BackgroundPlan,
        output_size: (u32, u32),
    ) -> Result<(), RenderingError> {
        let unit = |c: [u8; 3]| {
            [
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
                1.0,
            ]
        };
        let background = match background {
            BackgroundPlan::Solid { color } => Background::Color(unit(*color)),
            BackgroundPlan::Gradient {
                from,
                to,
                angle_deg,
            } => Background::Gradient(Gradient {
                start: unit(*from),
                end: unit(*to),
                angle: *angle_deg as f32,
                noise_intensity: 0.0,
                noise_scale: 3.0,
            }),
            BackgroundPlan::Image { path, .. } => Background::Image { path: path.clone() },
        };
        match background {
            Background::Image { path } => {
                if let Some((_, message)) = self.failed.as_ref().filter(|(p, _)| *p == path) {
                    let message = message.clone();
                    self.prepare_color(device, [0.0, 0.0, 0.0, 1.0]);
                    return Err(RenderingError::ImageLoadError(message));
                }
                let current_texture = match &self.inner {
                    Some(Inner::Image {
                        path: current_path,
                        output_size: current_size,
                        ..
                    }) if current_path == &path && *current_size == output_size => return Ok(()),
                    Some(Inner::Image {
                        path: current_path,
                        texture,
                        ..
                    }) if current_path == &path => Some(texture.clone()),
                    _ => None,
                };
                let texture = match current_texture {
                    Some(texture) => texture,
                    None => match load_background_texture(device, queue, &path).await {
                        Ok(texture) => texture,
                        Err(error) => {
                            tracing::warn!(%error, "background image couldn't be loaded");
                            self.failed = Some((path.clone(), error.clone()));
                            self.prepare_color(device, [0.0, 0.0, 0.0, 1.0]);
                            return Err(RenderingError::ImageLoadError(error));
                        }
                    },
                };

                let output_ar = output_size.1 as f32 / output_size.0 as f32;
                let image_ar = texture.height() as f32 / texture.width() as f32;

                let y_height = if output_ar < image_ar {
                    ((image_ar - output_ar) / 2.0) / image_ar
                } else {
                    0.0
                };

                let x_width = if output_ar > image_ar {
                    let output_ar = 1.0 / output_ar;
                    let image_ar = 1.0 / image_ar;

                    ((image_ar - output_ar) / 2.0) / image_ar
                } else {
                    0.0
                };

                let image_uniforms = ImageBackgroundUniforms {
                    output_size: [output_size.0 as f32, output_size.1 as f32],
                    padding: 0.0,
                    x_width,
                    y_height,
                    _padding: 0.0,
                };

                let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Image Background Uniforms"),
                    contents: bytemuck::cast_slice(&[image_uniforms]),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });

                let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group =
                    self.image_pipeline
                        .bind_group(device, &uniform_buffer, &texture_view);

                self.inner = Some(Inner::Image {
                    path,
                    output_size,
                    texture,
                    bind_group,
                });
            }
            Background::Color(color) => self.prepare_color(device, color),
            Background::Gradient(gradient) => match &self.inner {
                Some(Inner::ColorOrGradient {
                    value: ColorOrGradient::Gradient(current_gradient),
                    ..
                }) if &gradient == current_gradient => {}
                _ => {
                    let buffer = GradientOrColorUniforms::from(background).to_buffer(device);
                    self.inner = Some(Inner::ColorOrGradient {
                        value: ColorOrGradient::Gradient(gradient),
                        bind_group: self.color_pipeline.bind_group(device, &buffer),
                        buffer,
                    });
                }
            },
        }

        Ok(())
    }

    /// Black at `alpha` over what is already drawn (Klipolo's `Layer::Black`, clip fades).
    pub fn prepare_black(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, alpha: f32) {
        let color = [0.0, 0.0, 0.0, alpha.clamp(0.0, 1.0)];
        match &mut self.inner {
            Some(Inner::ColorOrGradient {
                value: value @ ColorOrGradient::Color(_),
                buffer,
                ..
            }) => {
                if *value != ColorOrGradient::Color(color) {
                    let uniforms = GradientOrColorUniforms::from(Background::Color(color));
                    queue.write_buffer(buffer, 0, bytemuck::cast_slice(&[uniforms]));
                    *value = ColorOrGradient::Color(color);
                }
            }
            _ => self.prepare_color(device, color),
        }
    }

    fn prepare_color(&mut self, device: &wgpu::Device, color: [f32; 4]) {
        match &self.inner {
            Some(Inner::ColorOrGradient {
                value: ColorOrGradient::Color(current_color),
                ..
            }) if &color == current_color => {}
            _ => {
                let buffer =
                    GradientOrColorUniforms::from(Background::Color(color)).to_buffer(device);
                self.inner = Some(Inner::ColorOrGradient {
                    value: ColorOrGradient::Color(color),
                    bind_group: self.color_pipeline.bind_group(device, &buffer),
                    buffer,
                });
            }
        }
    }

    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(Inner::Image { bind_group, .. }) = &self.inner {
            pass.set_pipeline(&self.image_pipeline.render_pipeline);
            pass.set_bind_group(0, bind_group, &[]);
        } else if let Some(Inner::ColorOrGradient { bind_group, .. }) = &self.inner {
            pass.set_pipeline(&self.color_pipeline.render_pipeline);
            pass.set_bind_group(0, bind_group, &[]);
        } else {
            return;
        }

        pass.draw(0..4, 0..1);
    }
}

/// Decodes `path` on a blocking thread and uploads it (Cap's `BackgroundTextureCache::decode_and_upload`, uncached).
async fn load_background_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &str,
) -> Result<wgpu::Texture, crate::render::image_load::ImageError> {
    let max_dimension = MAX_BACKGROUND_DIMENSION.min(device.limits().max_texture_dimension_2d);
    let path_owned = path.to_string();

    let (rgba, width, height) =
        tokio::task::spawn_blocking(move || decode_background_rgba(&path_owned, max_dimension))
            .await
            .map_err(|e| crate::render::image_load::ImageError::Message(e.to_string()))??;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Background Image Texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    Ok(texture)
}

pub struct ImageBackgroundPipeline {
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub render_pipeline: wgpu::RenderPipeline,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ImageBackgroundUniforms {
    output_size: [f32; 2],
    padding: f32,
    x_width: f32,
    y_height: f32,
    _padding: f32, // For alignment
}

impl ImageBackgroundPipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ImageBackgroundBindGroupLayout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(include_wgsl!("../shaders/image-background.wgsl"));

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ImageBackgroundPipeline"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("ImageBackgroundPipelineLayout"),
                    bind_group_layouts: &[&bind_group_layout],
                    push_constant_ranges: &[],
                }),
            ),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[],
                    zero_initialize_workgroup_memory: false,
                },
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &[],
                    zero_initialize_workgroup_memory: false,
                },
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            bind_group_layout,
            render_pipeline,
        }
    }

    pub fn bind_group(
        &self,
        device: &wgpu::Device,
        uniforms: &wgpu::Buffer,
        texture: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ImageBackgroundBindGroup"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(texture),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        })
    }
}

pub struct GradientOrColorPipeline {
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub render_pipeline: wgpu::RenderPipeline,
}

#[derive(Debug, Clone, Copy, Pod, Zeroable, Default)]
#[repr(C)]
pub struct GradientOrColorUniforms {
    pub start: [f32; 4],
    pub end: [f32; 4],
    pub angle: f32,
    pub noise_intensity: f32,
    pub noise_scale: f32,
    _padding: f32,
}

impl GradientOrColorUniforms {
    fn to_buffer(self, device: &wgpu::Device) -> wgpu::Buffer {
        device.create_buffer_init(
            &(wgpu::util::BufferInitDescriptor {
                label: Some("GradientOrColorUniforms Buffer"),
                contents: bytemuck::cast_slice(&[self]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
        )
    }
}

impl From<Background> for GradientOrColorUniforms {
    fn from(value: Background) -> Self {
        match value {
            Background::Color(color) => Self {
                start: [
                    color[0] * color[3],
                    color[1] * color[3],
                    color[2] * color[3],
                    color[3],
                ],
                end: [
                    color[0] * color[3],
                    color[1] * color[3],
                    color[2] * color[3],
                    color[3],
                ],
                angle: 0.0,
                noise_intensity: 0.0,
                noise_scale: 0.0,
                _padding: 0.0,
            },
            Background::Gradient(Gradient {
                start,
                end,
                angle,
                noise_intensity,
                noise_scale,
            }) => Self {
                start: [
                    start[0] * start[3],
                    start[1] * start[3],
                    start[2] * start[3],
                    start[3],
                ],
                end: [end[0] * end[3], end[1] * end[3], end[2] * end[3], end[3]],
                angle,
                noise_intensity,
                noise_scale,
                _padding: 0.0,
            },
            Background::Image { .. } => {
                unreachable!("Textured backgrounds should be handled separately")
            }
        }
    }
}

impl GradientOrColorPipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = Self::bind_group_layout(device);
        let render_pipeline = create_shader_render_pipeline(
            device,
            &bind_group_layout,
            include_wgsl!("../shaders/gradient-or-color.wgsl"),
        );

        Self {
            bind_group_layout,
            render_pipeline,
        }
    }

    fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gradient-or-color.wgsl Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        })
    }

    pub fn bind_group(&self, device: &wgpu::Device, uniforms: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
            label: Some("bind_group"),
        })
    }
}
