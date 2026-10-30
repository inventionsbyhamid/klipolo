// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/layers/display.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: paths re-rooted; prepare takes one DecodedFrame (its size comes from the frame) instead of Cap's DecodedSegmentFrames + frame_size, and skips re-uploads by storage identity only (no recording time); prepare_with_encoder, the CPU-conversion preference (new_with_options, prefer_cpu_conversion) and the Windows/Linux paths (try_d3d11_staging_fallback, D3D11/CUDA branches) removed. One instance draws one video layer. `render` also binds the composite shader's `@group(1)` LUT bind group (passed in), and `frame_texture` exposes the converted RGBA frame (Klipolo draws overlay video through the image pipeline so it can rotate). Klipolo: a frame backed by an NV12 IOSurface is drawn straight from its planes through the composite `nv12_pipeline` when `prepare` is given a `PlaneImporter` (no YUV→RGBA conversion, copy or intermediate texture); the YUV converter is made lazily for the frames that still need it, and the frame textures start as 16×16 placeholders instead of 1920×1080 (about 40 MB less GPU memory per video layer). The composite RGBA pipeline and the YUV converter pipelines (`SharedYuvPipelines`) are made on the first frame that needs a conversion, not with the layer.

use crate::vendor::cap::compat::XY;

use std::sync::Arc;

use crate::render::nv12::PlaneImporter;
use crate::vendor::cap::rendering::{
    DecodedFrame, PixelFormat,
    composite_frame::{CompositeVideoFramePipeline, CompositeVideoFrameUniforms},
    decoder::DecodedFrameStorageIdentity,
    yuv_converter::{YuvConverterPipelines, YuvToRgbaConverter},
};

/// Klipolo: the YUV converter pipelines every layer shares, made by the first layer whose frame needs them.
pub type SharedYuvPipelines = Arc<std::sync::OnceLock<Arc<YuvConverterPipelines>>>;

struct PendingTextureCopy {
    width: u32,
    height: u32,
    dst_texture_index: usize,
}

pub struct DisplayLayer {
    frame_textures: [wgpu::Texture; 2],
    frame_texture_views: [wgpu::TextureView; 2],
    current_texture: usize,
    uniforms_buffer: wgpu::Buffer,
    pipeline: std::sync::Arc<CompositeVideoFramePipeline>,
    bind_groups: [Option<wgpu::BindGroup>; 2],
    last_frame_storage: Option<DecodedFrameStorageIdentity>,
    /// Klipolo: made on the first frame that needs a conversion (VideoToolbox frames are sampled as planes instead).
    yuv_converter: Option<YuvToRgbaConverter>,
    yuv_pipelines: SharedYuvPipelines,
    pending_copy: Option<PendingTextureCopy>,
    has_valid_frame: bool,
    /// Klipolo: the current frame drawn straight from its NV12 planes (`nv12_pipeline`), when it has an IOSurface.
    nv12_group: Option<wgpu::BindGroup>,
}

impl DisplayLayer {
    /// Forget the last frame shown, as a new layer would.
    pub(crate) fn reset_frame_state(&mut self) {
        self.last_frame_storage = None;
        self.has_valid_frame = false;
        self.nv12_group = None;
    }

    #[allow(dead_code)]
    pub fn new(device: &wgpu::Device) -> Self {
        Self::new_with_all_shared_pipelines(
            device,
            SharedYuvPipelines::default(),
            Arc::new(CompositeVideoFramePipeline::new(device)),
        )
    }

    pub fn new_with_all_shared_pipelines(
        device: &wgpu::Device,
        yuv_pipelines: SharedYuvPipelines,
        composite_pipeline: Arc<CompositeVideoFramePipeline>,
    ) -> Self {
        // Klipolo: placeholders, sized on the first frame that needs a conversion (most never do).
        let frame_texture_0 = CompositeVideoFramePipeline::create_frame_texture(device, 16, 16);
        let frame_texture_1 = CompositeVideoFramePipeline::create_frame_texture(device, 16, 16);
        let frame_texture_view_0 = frame_texture_0.create_view(&Default::default());
        let frame_texture_view_1 = frame_texture_1.create_view(&Default::default());

        let uniforms_buffer = CompositeVideoFrameUniforms::default().to_buffer(device);
        let bind_group_0 =
            Some(composite_pipeline.bind_group(device, &uniforms_buffer, &frame_texture_view_0));
        let bind_group_1 =
            Some(composite_pipeline.bind_group(device, &uniforms_buffer, &frame_texture_view_1));

        Self {
            frame_textures: [frame_texture_0, frame_texture_1],
            frame_texture_views: [frame_texture_view_0, frame_texture_view_1],
            current_texture: 0,
            uniforms_buffer,
            pipeline: composite_pipeline,
            bind_groups: [bind_group_0, bind_group_1],
            last_frame_storage: None,
            yuv_converter: None,
            yuv_pipelines,
            pending_copy: None,
            has_valid_frame: false,
            nv12_group: None,
        }
    }

    /// Uploads `frame` (skipped when it is the frame already shown) and sets the uniforms. Returns whether there is
    /// a frame to draw. Record `copy_to_texture` in the frame's encoder before `render`. With `planes`, a frame
    /// backed by an NV12 IOSurface is drawn straight from its planes (no conversion, no copy).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &DecodedFrame,
        uniforms: CompositeVideoFrameUniforms,
        planes: Option<&PlaneImporter>,
    ) -> bool {
        self.pending_copy = None;

        let actual_width = frame.width();
        let actual_height = frame.height();
        let source_size = XY::new(actual_width, actual_height);
        let format = frame.format();
        let frame_storage = frame.storage_identity();

        let skipped = self
            .last_frame_storage
            .as_ref()
            .is_some_and(|last| last.matches(&frame_storage));

        let mut frame_uploaded = false;

        if !skipped {
            if let Some(planes) = planes.and_then(|p| p.import(device, frame)) {
                self.nv12_group = Some(self.pipeline.nv12_bind_group(
                    device,
                    &self.uniforms_buffer,
                    &planes.y,
                    &planes.uv,
                ));
                self.last_frame_storage = Some(frame_storage);
                self.has_valid_frame = true;
                uniforms.write_to_buffer(queue, &self.uniforms_buffer);
                return true;
            }
            self.nv12_group = None;
        }

        if !skipped {
            // Klipolo: the conversion's pipelines and the RGBA composite are made on the first frame that needs them.
            self.pipeline.rgba_pipeline(device);
            let pipelines = &self.yuv_pipelines;
            let converter = self.yuv_converter.get_or_insert_with(|| {
                let shared = pipelines.get_or_init(|| Arc::new(YuvConverterPipelines::new(device)));
                YuvToRgbaConverter::new_with_shared_pipelines(device, shared.clone())
            });
            let next_texture = 1 - self.current_texture;

            if self.frame_textures[next_texture].width() != source_size.x
                || self.frame_textures[next_texture].height() != source_size.y
            {
                self.frame_textures[next_texture] =
                    CompositeVideoFramePipeline::create_frame_texture(
                        device,
                        source_size.x,
                        source_size.y,
                    );
                self.frame_texture_views[next_texture] =
                    self.frame_textures[next_texture].create_view(&Default::default());

                self.bind_groups[next_texture] = Some(self.pipeline.bind_group(
                    device,
                    &self.uniforms_buffer,
                    &self.frame_texture_views[next_texture],
                ));
            }

            frame_uploaded = match format {
                PixelFormat::Rgba => {
                    let frame_data = frame.data();
                    let src_bytes_per_row = source_size.x * 4;

                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.frame_textures[next_texture],
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        frame_data,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(src_bytes_per_row),
                            rows_per_image: Some(source_size.y),
                        },
                        wgpu::Extent3d {
                            width: source_size.x,
                            height: source_size.y,
                            depth_or_array_layers: 1,
                        },
                    );
                    true
                }
                PixelFormat::Nv12 => {
                    #[cfg(target_os = "macos")]
                    let iosurface_converted = frame
                        .with_image_buf(|image_buf| {
                            converter
                                .convert_nv12_from_iosurface(device, queue, image_buf)
                                .is_ok()
                        })
                        .unwrap_or(false);
                    #[cfg(not(target_os = "macos"))]
                    let iosurface_converted = false;

                    if iosurface_converted && converter.output_texture().is_some() {
                        self.pending_copy = Some(PendingTextureCopy {
                            width: source_size.x,
                            height: source_size.y,
                            dst_texture_index: next_texture,
                        });
                        true
                    } else if let (Some(y_data), Some(uv_data)) =
                        (frame.y_plane(), frame.uv_plane())
                    {
                        let y_stride = frame.y_stride();
                        let uv_stride = frame.uv_stride();
                        let convert_result = converter.convert_nv12(
                            device,
                            queue,
                            y_data,
                            uv_data,
                            source_size.x,
                            source_size.y,
                            y_stride,
                            uv_stride,
                        );

                        match convert_result {
                            Ok(_) => {
                                if converter.output_texture().is_some() {
                                    self.pending_copy = Some(PendingTextureCopy {
                                        width: source_size.x,
                                        height: source_size.y,
                                        dst_texture_index: next_texture,
                                    });
                                    true
                                } else {
                                    false
                                }
                            }
                            Err(_) => false,
                        }
                    } else {
                        false
                    }
                }
                PixelFormat::Yuv420p => {
                    let y_plane = frame.y_plane();
                    let u_plane = frame.u_plane();
                    let v_plane = frame.v_plane();

                    if let (Some(y_data), Some(u_data), Some(v_data)) = (y_plane, u_plane, v_plane)
                    {
                        let convert_result = converter.convert_yuv420p(
                            device,
                            queue,
                            y_data,
                            u_data,
                            v_data,
                            source_size.x,
                            source_size.y,
                            frame.y_stride(),
                            frame.uv_stride(),
                        );

                        match convert_result {
                            Ok(_) => {
                                if converter.output_texture().is_some() {
                                    self.pending_copy = Some(PendingTextureCopy {
                                        width: source_size.x,
                                        height: source_size.y,
                                        dst_texture_index: next_texture,
                                    });
                                    true
                                } else {
                                    false
                                }
                            }
                            Err(_) => false,
                        }
                    } else {
                        false
                    }
                }
            };

            if frame_uploaded {
                self.last_frame_storage = Some(frame_storage);
                self.current_texture = next_texture;
                self.has_valid_frame = true;
            } else {
                tracing::warn!(
                    width = actual_width,
                    height = actual_height,
                    format = ?format,
                    "Display frame upload failed"
                );
            }
        }

        uniforms.write_to_buffer(queue, &self.uniforms_buffer);
        (skipped && self.has_valid_frame) || frame_uploaded
    }

    pub fn copy_to_texture(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(pending) = self.pending_copy.take() else {
            return;
        };

        let Some(src_texture) = self
            .yuv_converter
            .as_ref()
            .and_then(YuvToRgbaConverter::output_texture)
        else {
            tracing::warn!("copy_to_texture: no source texture from YUV converter");
            return;
        };

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: src_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.frame_textures[pending.dst_texture_index],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: pending.width,
                height: pending.height,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>, lut: &wgpu::BindGroup) {
        if !self.has_valid_frame {
            return;
        }

        if let Some(planes) = &self.nv12_group {
            pass.set_pipeline(&self.pipeline.nv12_pipeline);
            pass.set_bind_group(0, planes, &[]);
            pass.set_bind_group(1, lut, &[]);
            pass.draw(0..3, 0..1);
            return;
        }

        // Klipolo: `prepare` made the RGBA pipeline before uploading a converted frame.
        if let (Some(bind_group), Some(pipeline)) = (
            &self.bind_groups[self.current_texture],
            self.pipeline.made_rgba_pipeline(),
        ) {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.set_bind_group(1, lut, &[]);
            pass.draw(0..3, 0..1);
        } else {
            tracing::warn!(
                current_texture_index = self.current_texture,
                "DisplayLayer::render - no bind group available"
            );
        }
    }

    /// Klipolo: the RGBA texture holding the frame `prepare` uploaded (after `copy_to_texture` runs in the encoder).
    pub fn frame_texture(&self) -> Option<&wgpu::Texture> {
        (self.has_valid_frame && self.nv12_group.is_none())
            .then(|| &self.frame_textures[self.current_texture])
    }

    pub fn has_valid_frame(&self) -> bool {
        self.has_valid_frame
    }

    pub fn prepare_for_video_dimensions(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let pipelines = &self.yuv_pipelines;
        let converter = self.yuv_converter.get_or_insert_with(|| {
            let shared = pipelines.get_or_init(|| Arc::new(YuvConverterPipelines::new(device)));
            YuvToRgbaConverter::new_with_shared_pipelines(device, shared.clone())
        });
        if let Err(e) = converter.prepare_for_dimensions(device, width, height) {
            tracing::warn!(
                width = width,
                height = height,
                error = ?e,
                "Failed to pre-allocate YUV converter textures"
            );
        }
    }
}
