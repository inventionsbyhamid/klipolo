//! The cursor layer: everything about one take's cursor in one frame, as `plan::cursor_layer`
//! decided it. The GPU pass draws it from its own slot; a transition between two takes holds two.

use std::sync::Arc;

use klipolo_model::layout::BoxN;

/// The cursor over one main clip's footage, in canvas-normalized units.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorLayer {
    /// `cursor:<clip id>` (`plan::cursor_layer_id`).
    pub id: Arc<str>,
    pub look: CursorDraw,
    /// Where the hotspot lands, canvas-normalized.
    pub hotspot: (f64, f64),
    /// The drawn height, as a fraction of the canvas height.
    pub height: f64,
    /// 0 when only its ripples show (the cursor faded out at rest, or off the zoomed view while they finish).
    pub opacity: f64,
    /// 0.85–1: the press squish.
    pub press: f64,
    /// The smear over the shutter, canvas-normalized (0 at rest).
    pub blur: (f64, f64),
    /// The main footage's box and corner radius: nothing is drawn outside.
    pub clip: BoxN,
    pub radius: f64,
    /// The footage's shape mask, which masks the cursor and its ripples too.
    pub mask: Option<CursorMask>,
    /// Oldest first; the first `ripple_count` are drawn (`active_ripples`).
    pub ripples: [Ripple; 4],
    pub ripple_count: u8,
}

impl CursorLayer {
    /// The ripples drawn, oldest first.
    pub fn active_ripples(&self) -> &[Ripple] {
        &self.ripples[..usize::from(self.ripple_count).min(self.ripples.len())]
    }
}

/// The main footage's shape mask (the built-in `effect.mask`) as its pass is set up for this frame, in the footage's
/// box (`mask.wgsl`'s rules): the GPU pass clips the cursor and its ripples to the same shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorMask {
    /// 0 circle, 1 rounded square, 2 square (each as big as the box's shorter side), 3 frame (the whole box).
    pub shape: u32,
    /// Where the shape sits in the room it has inside the box, 0–1 on each axis.
    pub center: (f32, f32),
    /// The corners, a share of the box's shorter side (rounded square and frame).
    pub corner: f32,
    /// The soft edge inside the shape, in pixels at 1080p.
    pub feather: f32,
}

/// What draws for the cursor.
#[derive(Clone, Debug, PartialEq)]
pub enum CursorDraw {
    /// One of the take's shapes: `take` is its cursor file's path (the atlas key with `shape`), `file` the PNG's checked
    /// path, `hotspot` 0–1 of the image, `aspect` its width over height.
    Shape {
        take: Arc<str>,
        shape: u16,
        file: Arc<str>,
        hotspot: (f32, f32),
        aspect: f32,
    },
    /// Klipolo's filled circle with an outline.
    Dot,
    /// Klipolo's own arrow: also what draws for a shape whose image can't be used.
    Arrow,
}

/// A ring after a real click, growing while it fades.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ripple {
    /// Where the click was, canvas-normalized.
    pub center: (f64, f64),
    /// As a fraction of the canvas height.
    pub radius: f64,
    /// 0–1: the click colour's own alpha, faded.
    pub alpha: f64,
    /// The style's click colour, RGBA.
    pub color: [u8; 4],
}
