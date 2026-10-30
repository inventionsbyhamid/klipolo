//! What one frame of a scene says about each node and each split unit: plain `Copy` values the renderer reads, every
//! number held within its field's range. Positions are a fraction of the parent's box plus 1080p pixels; transforms
//! and opacity are the node's own (the renderer composes them down the tree).

use super::super::units::{MAX_FRACTION, MAX_PX};

/// A clip as lengths that are each a fraction of the box plus pixels (`[fraction, px]`), so two clips written in
/// different units ease into each other.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClipState {
    pub shape: ClipShape,
    /// `inset`: top, right, bottom, left, the corners' round; `circle`: radius, x, y (then unused).
    pub v: [[f64; 2]; 5],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipShape {
    #[default]
    None,
    Inset,
    Circle,
}

impl ClipState {
    /// Eased from `a` to `b` by `p`: lengths of the same shape mix; no clip mixes with an inset as `inset(0)`; other
    /// shapes switch at the end.
    pub fn mix(a: &ClipState, b: &ClipState, p: f64) -> ClipState {
        let shape = match (a.shape, b.shape) {
            (x, y) if x == y => x,
            (ClipShape::None, ClipShape::Inset) | (ClipShape::Inset, ClipShape::None) => {
                ClipShape::Inset
            }
            _ => return if p >= 1.0 { *b } else { *a },
        };
        let mut out = ClipState {
            shape,
            v: [[0.0; 2]; 5],
        };
        // An ease past its ends (back, elastic) can carry a length out: each is held within the box's ±10 and
        // ±100,000 pixels.
        let limits = [MAX_FRACTION, MAX_PX];
        for (o, (x, y)) in out.v.iter_mut().zip(a.v.iter().zip(&b.v)) {
            for c in 0..2 {
                o[c] = (x[c] + (y[c] - x[c]) * p).clamp(-limits[c], limits[c]);
            }
        }
        out
    }
}

/// Which text a text node shows this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextNow {
    /// Its texts' one at this index (0 is its own text; holds, text keys and tween texts follow).
    #[default]
    Base,
    Text(u16),
    /// The frame's buffer for it: a counter's number, or a scramble of its text at this index (the renderer composes
    /// the scramble from that text's glyphs).
    Buffer(u16),
}

/// A drop shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowState {
    pub x: f64,
    pub y: f64,
    pub blur: f64,
    pub spread: f64,
    pub color: [f32; 4],
}

/// One node at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeState {
    /// Visible and above 0 opacity, and so is every node above it: it draws.
    pub drawn: bool,
    pub visible: bool,
    /// A group drawn offscreen this frame (blur, a tilt, opacity between 0 and 1, a blend or a mask).
    pub isolated: bool,
    pub x: f64,
    pub x_px: f64,
    pub y: f64,
    pub y_px: f64,
    /// In its own box (fractions, then pixels): 50 % 50 % unless set.
    pub origin: [f64; 2],
    pub origin_px: [f64; 2],
    pub scale: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    /// Degrees, clockwise.
    pub rotate: f64,
    pub rotate_x: f64,
    pub rotate_y: f64,
    pub perspective: f64,
    pub opacity: f64,
    pub blur: f64,
    pub clip: ClipState,
    /// Straight RGBA, 0–1: the colour fill (black unless set, as in SVG); a gradient or generator fill is the
    /// node's own, slid and turned by `fill_offset` and `fill_angle`.
    pub fill: [f32; 4],
    pub fill_offset: f64,
    pub fill_angle: f64,
    /// None (alpha 0) unless set.
    pub stroke: [f32; 4],
    pub stroke_width: f64,
    pub dash_offset: f64,
    /// Start, end and offset, fractions of the stroke.
    pub trim: [f64; 3],
    /// Top left, top right, bottom right, bottom left.
    pub radius: [f64; 4],
    pub inner_radius: f64,
    pub shadow: ShadowState,
    /// Along its motion path, 0–1.
    pub progress: f64,
    pub size: f64,
    pub letter_spacing: f64,
    pub line_height: f64,
    pub tint: [f32; 4],
    pub text: TextNow,
    /// The typewriter's progress, 0–1 (1 without one): see `modifiers::reveal_count`.
    pub reveal: f64,
}

impl NodeState {
    pub const REST: Self = Self {
        drawn: true,
        visible: true,
        isolated: false,
        x: 0.0,
        x_px: 0.0,
        y: 0.0,
        y_px: 0.0,
        origin: [0.5, 0.5],
        origin_px: [0.0, 0.0],
        scale: 1.0,
        scale_x: 1.0,
        scale_y: 1.0,
        rotate: 0.0,
        rotate_x: 0.0,
        rotate_y: 0.0,
        perspective: 1000.0,
        opacity: 1.0,
        blur: 0.0,
        clip: ClipState {
            shape: ClipShape::None,
            v: [[0.0; 2]; 5],
        },
        fill: [0.0, 0.0, 0.0, 1.0],
        fill_offset: 0.0,
        fill_angle: 0.0,
        stroke: [0.0; 4],
        stroke_width: 1.0,
        dash_offset: 0.0,
        trim: [0.0, 1.0, 0.0],
        radius: [0.0; 4],
        inner_radius: 0.5,
        shadow: ShadowState {
            x: 0.0,
            y: 0.0,
            blur: 0.0,
            spread: 0.0,
            color: [0.0, 0.0, 0.0, 0.5],
        },
        progress: 0.0,
        size: 48.0,
        letter_spacing: 0.0,
        line_height: 1.2,
        tint: [0.0; 4],
        text: TextNow::Base,
        reveal: 1.0,
    };
}

/// One unit of a split text (a character, word or line) at one moment: its own offsets from where the layout put it,
/// on top of its node's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitState {
    pub x: f64,
    pub x_px: f64,
    pub y: f64,
    pub y_px: f64,
    pub scale: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotate: f64,
    /// Multiplies the node's.
    pub opacity: f64,
    pub blur: f64,
}

impl UnitState {
    pub const REST: Self = Self {
        x: 0.0,
        x_px: 0.0,
        y: 0.0,
        y_px: 0.0,
        scale: 1.0,
        scale_x: 1.0,
        scale_y: 1.0,
        rotate: 0.0,
        opacity: 1.0,
        blur: 0.0,
    };
}
