//! Video layers for the compositor. Main and camera video go through the vendored composite pipeline (crop, mask,
//! border, look), straight from the frame's NV12 planes when VideoToolbox decoded it. Overlay video is converted at
//! about the size it's drawn (`nv12_scale::ScaledFrame`) and drawn by the image quad, which rotates; frames without an
//! IOSurface (the FFmpeg fallback) take Cap's converter path.

use std::sync::Arc;

use crate::plan::{VideoLayer, VideoRole};
use crate::render::look;
use crate::render::nv12::PlaneImporter;
use crate::render::nv12_scale::{Nv12Scaler, ScaledFrame};
use crate::render::uniforms::{quad_for, video_uniforms};
use crate::vendor::cap::compat::ColorCorrection;
use crate::vendor::cap::rendering::DecodedFrame;
use crate::vendor::cap::rendering::composite_frame::{
    ColorGradeUniformParams, CompositeVideoFramePipeline,
};
use crate::vendor::cap::rendering::layers::display::{DisplayLayer, SharedYuvPipelines};
use crate::vendor::cap::rendering::layers::image::ImageQuad;
use crate::vendor::cap::rendering::yuv_converter::YuvConverterPipelines;

/// How one video layer is drawn this frame.
pub enum VideoDraw {
    /// Composite drawer `index` (main or camera), with the clip's LUT bound by the caller.
    Composite(usize),
    /// An RGBA texture through the image quad (overlay video).
    Quad(wgpu::Texture, ImageQuad),
}

pub struct VideoDrawers {
    /// Made by the first frame that needs a conversion (most frames are VideoToolbox's, drawn from their planes).
    yuv: SharedYuvPipelines,
    pub composite: Arc<CompositeVideoFramePipeline>,
    planes: PlaneImporter,
    /// Made by the first overlay video (a first frame of main footage doesn't wait for its compile).
    scaler: Option<Arc<Nv12Scaler>>,
    /// One drawer per concurrent video layer, grown on demand and reused frame to frame (keeps textures warm).
    drawers: Vec<DisplayLayer>,
    overlays: Vec<ScaledFrame>,
    /// The last two looks' grades by (look id, or "" for a LUT-only look; strength bits). Main and camera share
    /// their clip's, and a transition shows two clips; `ColorCorrection` holds a String, so it isn't rebuilt per
    /// frame.
    grades: [Option<(Arc<str>, u64, ColorCorrection)>; 2],
    /// The entry a new grade replaces: the one not used last.
    stale_grade: usize,
    used: (usize, usize),
}

impl VideoDrawers {
    pub fn new(device: &wgpu::Device, composite: Arc<CompositeVideoFramePipeline>) -> Self {
        Self {
            yuv: SharedYuvPipelines::default(),
            composite,
            planes: PlaneImporter::new(device),
            scaler: None,
            drawers: Vec::new(),
            overlays: Vec::new(),
            grades: [None, None],
            stale_grade: 0,
            used: (0, 0),
        }
    }

    /// Makes now the pipelines video without an IOSurface and overlay video need, which the first such frame would
    /// otherwise make (`GpuBackend::warm_all`).
    pub fn warm_all(&mut self, device: &wgpu::Device) {
        self.composite.rgba_pipeline(device);
        self.yuv
            .get_or_init(|| Arc::new(YuvConverterPipelines::new(device)));
        self.scaler.get_or_insert_with(|| Nv12Scaler::new(device));
    }

    /// Drops the frame textures (the project closed).
    pub fn forget(&mut self) {
        self.drawers.clear();
        self.overlays.clear();
    }

    /// Starts a frame: drawers are handed out from the first again.
    pub fn begin_frame(&mut self) {
        self.used = (0, 0);
    }

    fn drawer(&mut self, device: &wgpu::Device) -> usize {
        if self.drawers.len() == self.used.0 {
            self.drawers
                .push(DisplayLayer::new_with_all_shared_pipelines(
                    device,
                    self.yuv.clone(),
                    self.composite.clone(),
                ));
        }
        self.used.0 += 1;
        self.used.0 - 1
    }

    /// Prepares `v` showing `frame` (stored turned `rotation`° clockwise) in an output of `size`, recording any
    /// conversion into `encoder`. None when it has nothing to draw.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        v: &VideoLayer,
        frame: &DecodedFrame,
        rotation: u16,
        size: (u32, u32),
        grain_frame: u32,
    ) -> Option<VideoDraw> {
        // Frames stored rotated (phone video) are drawn upright: the uniforms use the upright size.
        let upright = if rotation % 180 == 90 {
            (frame.height(), frame.width())
        } else {
            (frame.width(), frame.height())
        };
        let mut uniforms = video_uniforms(v, upright, size);
        uniforms.source_rotation = f32::from(rotation);
        if uniforms.target_size[0] < 1.0 || uniforms.target_size[1] < 1.0 {
            return None;
        }
        if v.role == VideoRole::Overlay {
            // A mirrored layer (a separated camera) flips as it is drawn, or in its effects' first pass.
            let quad = ImageQuad {
                source_rotation: rotation,
                mirror: v.mirror,
                blend: v.blend,
                ..quad_for(v.dest, v.rotation, v.opacity, v.radius, size)
            };
            // The scaled texture is in the frame's stored orientation, at the largest size the layer's motion reaches
            // (its `reach`), so a scaling layer converts into one texture rather than a new one per size step.
            let (w, h) = (quad.size.x * v.reach, quad.size.y * v.reach);
            let drawn = if rotation % 180 == 90 { (h, w) } else { (w, h) };
            if self.overlays.len() == self.used.1 {
                self.overlays.push(ScaledFrame::default());
            }
            let scaler = self
                .scaler
                .get_or_insert_with(|| Nv12Scaler::new(device))
                .clone();
            let scaled = &mut self.overlays[self.used.1];
            self.used.1 += 1;
            if let Some(texture) =
                scaled.prepare(device, queue, &scaler, &self.planes, frame, drawn)
            {
                scaled.encode(&scaler, encoder);
                return Some(VideoDraw::Quad(texture, quad));
            }
            // No IOSurface (FFmpeg fallback): convert at full size.
            let index = self.drawer(device);
            let drawer = &mut self.drawers[index];
            if !drawer.prepare(device, queue, frame, uniforms, None) {
                return None;
            }
            drawer.copy_to_texture(encoder);
            return drawer
                .frame_texture()
                .cloned()
                .map(|t| VideoDraw::Quad(t, quad));
        }
        if let Some(look) = &v.look {
            let id = look.look.as_ref().map_or("", |(id, _)| &**id);
            let bits = look.strength.to_bits();
            let cached = self.grades.iter().position(|g| {
                g.as_ref()
                    .is_some_and(|(cached, strength, _)| &**cached == id && *strength == bits)
            });
            let k = cached.unwrap_or_else(|| {
                // A LUT-only look grades nothing (the LUT does the work).
                let (key, grade) = match &look.look {
                    Some((id, g)) => (id.clone(), look::grade(g, look.strength)),
                    None => (Arc::from(""), ColorCorrection::default()),
                };
                self.grades[self.stale_grade] = Some((key, bits, grade));
                self.stale_grade
            });
            self.stale_grade = 1 - k;
            let grade = &self.grades[k].as_ref().expect("cached or just made").2;
            let p =
                ColorGradeUniformParams::from_config(grade, grain_frame, v.role == VideoRole::Main);
            uniforms.color_adjust_a = p.color_adjust_a;
            uniforms.color_adjust_b = p.color_adjust_b;
            uniforms.grain_params = p.grain_params;
        }
        let index = self.drawer(device);
        let drawer = &mut self.drawers[index];
        if !drawer.prepare(device, queue, frame, uniforms, Some(&self.planes)) {
            return None;
        }
        drawer.copy_to_texture(encoder);
        Some(VideoDraw::Composite(index))
    }

    pub fn render(&self, index: usize, pass: &mut wgpu::RenderPass<'_>, lut: &wgpu::BindGroup) {
        self.drawers[index].render(pass, lut);
    }
}
