//! The video layers of a plan (main footage, the camera, overlay video): which source frame, where it lands, and the
//! look and effects it is drawn through.

use std::sync::Arc;

use klipolo_model::layout::BoxN;
use klipolo_model::motion::Blend;
use klipolo_model::time::Us;

use super::EffectPass;
use crate::registry::LookGrade;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoRole {
    Main,
    Camera,
    Overlay,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceRef {
    pub asset_id: Arc<str>,
    /// The file to read: the source, or (preview only) its editing copy (`PlanContext::media`).
    pub path: Arc<str>,
    /// The source file as the project names it (resolved). Equal to `path` unless an editing copy is read: the decoder
    /// pool then opens this when the copy can't be opened, and missing-media messages always name it.
    pub original: Arc<str>,
    /// Source time to show (a copy has the same times).
    pub time_us: Us,
    /// The frame rate of `path` (0 = unknown; the decoder pool probes it): the asset's, or the copy's own.
    pub fps: f64,
    /// `path`'s clockwise turn (None = unknown; the decoder pool probes it): the asset's, or 0 for a copy (stored
    /// upright).
    pub rotation: Option<u16>,
    /// The source's shown size (a copy has its shape).
    pub width: u32,
    pub height: u32,
}

impl SourceRef {
    /// An editing copy is read instead of the source.
    pub fn is_copy(&self) -> bool {
        !Arc::ptr_eq(&self.path, &self.original) && self.path != self.original
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoLayer {
    pub role: VideoRole,
    /// The clip id (main), `camera:<clip id>` (camera) or the item id (overlay).
    pub id: Arc<str>,
    pub source: SourceRef,
    /// Where it lands, canvas-normalized, before rotation.
    pub dest: BoxN,
    /// Which part of the source frame is shown, source-normalized.
    pub crop: BoxN,
    /// Degrees clockwise about the centre of `dest`.
    pub rotation: f64,
    pub opacity: f64,
    /// Corner radius as a fraction of `dest`'s shorter side in pixels (0–0.5).
    pub radius: f64,
    pub mirror: bool,
    /// Camera border: 4 px at a 1080 shorter side, white at 90 %.
    pub border: bool,
    pub look: Option<LookPlan>,
    /// Shader passes over the footage, in order: the main footage's clip effects, or an overlay's own.
    pub effects: Vec<EffectPass>,
    /// How far its effects draw past its box, in 1080p pixels: its passes' margins added up (`plan::pad_of`).
    pub pad_px: f64,
    /// How it mixes with what is under it (an overlay layer's; main footage and the camera are normal).
    pub blend: Blend,
    /// How many times bigger than `dest` the largest box its motion reaches is (1 when it doesn't grow): its scaled
    /// frame and effect targets are sized for that once (`plan::motion::Moved::reach_from_here`).
    pub reach: f64,
}

impl VideoLayer {
    /// This layer drawn by itself, filling its own target (the effect stage then places the result).
    pub fn alone(&self) -> VideoLayer {
        VideoLayer {
            role: self.role,
            id: self.id.clone(),
            source: self.source.clone(),
            dest: crate::plan::framing::FULL,
            crop: self.crop,
            rotation: 0.0,
            opacity: 1.0,
            radius: 0.0,
            mirror: self.mirror,
            border: false,
            look: self.look.clone(),
            effects: Vec::new(),
            pad_px: 0.0,
            blend: Blend::Normal,
            reach: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LookPlan {
    /// The look's id (the grade cache key) and grade; None for a LUT-only look.
    pub look: Option<(Arc<str>, LookGrade)>,
    pub strength: f64,
    pub lut: Option<LutRef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LutRef {
    pub id: Arc<str>,
    pub path: Arc<str>,
    pub strength: f64,
}
