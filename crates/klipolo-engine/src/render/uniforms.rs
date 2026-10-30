//! Layer geometry for the GPU: plan boxes (canvas-normalized) as composite uniforms and image quads in output pixels.

use klipolo_model::layout::BoxN;

use crate::plan::VideoLayer;
use crate::vendor::cap::compat::XY;
use crate::vendor::cap::rendering::composite_frame::CompositeVideoFrameUniforms;
use crate::vendor::cap::rendering::layers::image::ImageQuad;

/// Camera border width at a 1080-pixel shorter side.
pub const CAMERA_BORDER_PX: f32 = 4.0;

/// A canvas-normalized box as an image quad in output pixels; `radius` is a share of the shorter side.
pub fn quad_for(
    dest: BoxN,
    rotation: f64,
    opacity: f64,
    radius: f64,
    out: (u32, u32),
) -> ImageQuad {
    let (ow, oh) = (f64::from(out.0), f64::from(out.1));
    let (w, h) = (dest.w * ow, dest.h * oh);
    ImageQuad {
        center: XY::new(dest.x * ow + w / 2.0, dest.y * oh + h / 2.0),
        size: XY::new(w, h),
        rotation_deg: rotation,
        opacity,
        rounding: radius.clamp(0.0, 0.5) * w.min(h),
        source_rotation: 0,
        mirror: false,
        blend: klipolo_model::motion::Blend::Normal,
    }
}

/// Composite uniforms for one video layer: canvas-normalized dest → output pixels, source-normalized crop → source
/// pixels, the radius as a share of the shorter side, the border scaled from 1080, and its clockwise turn.
pub fn video_uniforms(
    layer: &VideoLayer,
    frame: (u32, u32),
    out: (u32, u32),
) -> CompositeVideoFrameUniforms {
    let (ow, oh) = (out.0 as f32, out.1 as f32);
    let (fw, fh) = (frame.0 as f32, frame.1 as f32);
    let (d, c) = (layer.dest, layer.crop);
    let target = [
        d.x as f32 * ow,
        d.y as f32 * oh,
        (d.x + d.w) as f32 * ow,
        (d.y + d.h) as f32 * oh,
    ];
    let crop = [
        c.x as f32 * fw,
        c.y as f32 * fh,
        (c.x + c.w) as f32 * fw,
        (c.y + c.h) as f32 * fh,
    ];
    let size = [target[2] - target[0], target[3] - target[1]];
    CompositeVideoFrameUniforms {
        crop_bounds: crop,
        target_bounds: target,
        output_size: [ow, oh],
        frame_size: [fw, fh],
        target_size: size,
        rounding_px: layer.radius as f32 * size[0].min(size[1]),
        corner_radii: [1.0; 4],
        mirror_x: if layer.mirror { 1.0 } else { 0.0 },
        opacity: layer.opacity as f32,
        // Within one turn before it becomes f32, so large keyed angles stay precise.
        rotation_deg: (layer.rotation % 360.0) as f32,
        border_enabled: if layer.border { 1.0 } else { 0.0 },
        border_width: CAMERA_BORDER_PX * ow.min(oh) / 1080.0,
        border_color: [1.0, 1.0, 1.0, 0.9],
        ..CompositeVideoFrameUniforms::default()
    }
}
