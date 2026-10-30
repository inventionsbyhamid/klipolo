//! VideoToolbox frames straight from their IOSurfaces. The Y and UV planes are
//! imported as `R8`/`RG8` textures without a copy; main and camera video sample them in the composite shader (fused
//! BT.709, `CompositeVideoFramePipeline::nv12_pipeline`), so no full-size RGBA copy is written per video per frame.
//! Overlay video is converted once per new frame into an RGBA texture about the size it's drawn, with a box filter
//! (`nv12_scale::ScaledFrame`), so shrunk b-roll doesn't shimmer and the image quad samples it nearly 1:1.

use crate::vendor::cap::rendering::DecodedFrame;

/// A frame's planes as textures (they keep the IOSurface alive while the GPU reads them).
pub struct Nv12Planes {
    pub y: wgpu::TextureView,
    pub uv: wgpu::TextureView,
    pub size: (u32, u32),
}

/// Imports the planes of IOSurface-backed NV12 frames on the device wgpu renders with.
pub struct PlaneImporter {
    #[cfg(target_os = "macos")]
    cache: Option<crate::vendor::cap::rendering::iosurface_texture::IOSurfaceTextureCache>,
}

impl PlaneImporter {
    pub fn new(device: &wgpu::Device) -> Self {
        #[cfg(not(target_os = "macos"))]
        let _ = device;
        Self {
            #[cfg(target_os = "macos")]
            cache: crate::vendor::cap::rendering::iosurface_texture::IOSurfaceTextureCache::new(
                device,
            ),
        }
    }

    /// The frame's planes, or None when it isn't an NV12 IOSurface (CPU frames take the converter path).
    #[cfg(target_os = "macos")]
    pub fn import(&self, device: &wgpu::Device, frame: &DecodedFrame) -> Option<Nv12Planes> {
        use crate::vendor::cap::rendering::PixelFormat;
        use crate::vendor::cap::rendering::iosurface_texture::import_metal_texture_to_wgpu;
        if frame.format() != PixelFormat::Nv12 {
            return None;
        }
        let cache = self.cache.as_ref()?;
        frame
            .with_image_buf(|buf| {
                let surface = buf.io_surf()?;
                let (w, h) = (buf.width() as u32, buf.height() as u32);
                let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
                let y_plane = cache.create_y_texture(surface, w, h).ok()?;
                let uv_plane = cache.create_uv_texture(surface, w, h).ok()?;
                let y = wgpu::TextureFormat::R8Unorm;
                let y =
                    import_metal_texture_to_wgpu(device, &y_plane, y, w, h, Some("NV12 Y")).ok()?;
                let uv = wgpu::TextureFormat::Rg8Unorm;
                let uv =
                    import_metal_texture_to_wgpu(device, &uv_plane, uv, cw, ch, Some("NV12 UV"))
                        .ok()?;
                Some(Nv12Planes {
                    y: y.create_view(&wgpu::TextureViewDescriptor::default()),
                    uv: uv.create_view(&wgpu::TextureViewDescriptor::default()),
                    size: (w, h),
                })
            })
            .flatten()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn import(&self, _device: &wgpu::Device, _frame: &DecodedFrame) -> Option<Nv12Planes> {
        None
    }
}
