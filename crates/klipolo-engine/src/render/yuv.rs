//! RGBA → YUV for the export encoders, on the GPU: NV12 (8-bit 4:2:0, H.264), P010 (10-bit 4:2:0, HEVC Main 10) or
//! P210 (10-bit 4:2:2, ProRes), BT.709 limited range, into a storage buffer with 256-byte rows. From there the
//! planes are either copied on the GPU into an IOSurface-backed CVPixelBuffer that VideoToolbox encodes without a CPU
//! copy (`SurfacePool`), or read back to memory (SoftwareSafe mode).

use crate::render::readback::{PendingPixels, ReadbackRing, submit_buffer_readback};

/// The pixel layout an encoder takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Yuv {
    Nv12,
    P010,
    P210,
}

impl Yuv {
    fn bytes_per_sample(self) -> u32 {
        match self {
            Self::Nv12 => 1,
            Self::P010 | Self::P210 => 2,
        }
    }

    pub(crate) fn chroma_rows(self, height: u32) -> u32 {
        match self {
            Self::Nv12 | Self::P010 => height.div_ceil(2),
            Self::P210 => height,
        }
    }

    /// The FFmpeg software format of these planes.
    pub fn pixel(self) -> ffmpeg::format::Pixel {
        match self {
            Self::Nv12 => ffmpeg::format::Pixel::NV12,
            Self::P010 => ffmpeg::format::Pixel::P010LE,
            Self::P210 => ffmpeg::format::Pixel::P210LE,
        }
    }

    pub fn from_pixel(pixel: ffmpeg::format::Pixel) -> Option<Self> {
        match pixel {
            ffmpeg::format::Pixel::NV12 => Some(Self::Nv12),
            ffmpeg::format::Pixel::P010LE => Some(Self::P010),
            ffmpeg::format::Pixel::P210LE => Some(Self::P210),
            _ => None,
        }
    }
}

/// Where the planes are in the converter's buffer: both share `stride` bytes per row; UV starts at `uv_offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneLayout {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub uv_offset: u32,
    pub uv_rows: u32,
    pub bytes: u64,
}

impl PlaneLayout {
    pub fn new(yuv: Yuv, (width, height): (u32, u32)) -> Self {
        let stride =
            (width * yuv.bytes_per_sample()).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let uv_rows = yuv.chroma_rows(height);
        Self {
            width,
            height,
            stride,
            uv_offset: stride * height,
            uv_rows,
            bytes: u64::from(stride) * u64::from(height + uv_rows),
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
    stride: u32,
    uv_offset: u32,
    chroma_420: u32,
    pad: [u32; 3],
}

/// The conversion pass for one output size and layout (reused every frame).
pub struct YuvConverter {
    yuv: Yuv,
    layout: PlaneLayout,
    pipeline: wgpu::ComputePipeline,
    group_layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    params: wgpu::Buffer,
    /// The bind group for the texture it was made for (compared by wgpu identity).
    group: Option<(wgpu::Texture, wgpu::BindGroup)>,
}

impl YuvConverter {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, yuv: Yuv, size: (u32, u32)) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Export RGBA to YUV"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../vendor/cap/rendering/shaders/rgba_to_yuv.wgsl").into(),
            ),
        });
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Export YUV"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Export YUV"),
            bind_group_layouts: &[&group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Export YUV"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some(match yuv {
                Yuv::Nv12 => "nv12",
                Yuv::P010 | Yuv::P210 => "p16",
            }),
            compilation_options: Default::default(),
            cache: None,
        });
        let layout = PlaneLayout::new(yuv, size);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Export YUV planes"),
            size: layout.bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Export YUV params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let p = Params {
            width: layout.width,
            height: layout.height,
            stride: layout.stride,
            uv_offset: layout.uv_offset,
            chroma_420: u32::from(yuv != Yuv::P210),
            pad: [0; 3],
        };
        queue.write_buffer(&params, 0, bytemuck::bytes_of(&p));
        Self {
            yuv,
            layout,
            pipeline,
            group_layout,
            buffer,
            params,
            group: None,
        }
    }

    pub fn layout(&self) -> PlaneLayout {
        self.layout
    }

    pub fn yuv(&self) -> Yuv {
        self.yuv
    }

    /// Converts `source` (RGBA, the output size) into the planes buffer at the end of `encoder`.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
    ) {
        if self.group.as_ref().map(|(t, _)| t) != Some(source) {
            let view = source.create_view(&Default::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Export YUV"),
                layout: &self.group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.params.as_entire_binding(),
                    },
                ],
            });
            self.group = Some((source.clone(), group));
        }
        let (_, group) = self.group.as_ref().expect("made above");
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Export RGBA to YUV"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        let PlaneLayout { width, height, .. } = self.layout;
        match self.yuv {
            Yuv::Nv12 => pass.dispatch_workgroups(width.div_ceil(32), height.div_ceil(16), 1),
            Yuv::P010 | Yuv::P210 => {
                pass.dispatch_workgroups(width.div_ceil(16), height.div_ceil(16), 1)
            }
        }
    }

    /// Copies the planes into the two plane textures of an IOSurface (at the end of `encoder`).
    pub fn copy_to_planes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        y: &wgpu::Texture,
        uv: &wgpu::Texture,
    ) {
        let l = self.layout;
        let copy =
            |encoder: &mut wgpu::CommandEncoder, offset: u64, texture: &wgpu::Texture, w, h| {
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &self.buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset,
                            bytes_per_row: Some(l.stride),
                            rows_per_image: None,
                        },
                    },
                    texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            };
        copy(encoder, 0, y, l.width, l.height);
        copy(
            encoder,
            u64::from(l.uv_offset),
            uv,
            l.width.div_ceil(2),
            l.uv_rows,
        );
    }

    /// Submits `encoder` with a copy of the planes into a staging buffer; the pixels follow (SoftwareSafe mode).
    pub(crate) fn submit_readback(
        &self,
        dev: &crate::render::device::RenderDevice,
        ring: &ReadbackRing,
        pool: &crate::frames::buffer::FramePool,
        encoder: wgpu::CommandEncoder,
    ) -> PendingPixels {
        submit_buffer_readback(dev, ring, pool, &self.buffer, self.layout.bytes, encoder)
    }
}

#[cfg(target_os = "macos")]
pub use surface::{PixelBuffer, SurfacePool};

#[cfg(target_os = "macos")]
mod surface;

/// Where a format's converted frames go.
pub enum YuvSink {
    /// Into IOSurface-backed CVPixelBuffers (zero-copy encode).
    #[cfg(target_os = "macos")]
    Surface(SurfacePool),
    /// Read back to memory (SoftwareSafe mode).
    Readback,
}

/// One format's conversion pass and where its frames go.
pub struct YuvOut {
    pub converter: YuvConverter,
    pub sink: YuvSink,
}

/// A converted frame: an IOSurface the GPU is writing (hand it to the encoder once the frame's GPU work is done),
/// or planes on their way back to memory (`PlaneLayout` gives their rows).
pub enum YuvPixels {
    #[cfg(target_os = "macos")]
    Surface(PixelBuffer),
    Readback(PendingPixels),
}
