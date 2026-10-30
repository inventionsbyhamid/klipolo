//! How the main footage draws its source frame on the canvas (`Place`): where a source point lands, how a movement
//! scales, and whether a point is near enough to the footage's box to draw.

use klipolo_model::layout::BoxN;
use klipolo_model::project::FormatKind;

use super::super::VideoLayer;
use super::super::framing::canvas;

/// How the main footage draws its source frame on the canvas: `dest` shows `crop`.
pub(super) struct Place {
    dest: BoxN,
    crop: BoxN,
    /// The canvas's width over its height.
    aspect: f64,
}

impl Place {
    /// None for a footage that shows nothing (an empty crop).
    pub(super) fn of(footage: &VideoLayer, format: FormatKind) -> Option<Self> {
        let (w, h) = canvas(format);
        let (dest, crop) = (footage.dest, footage.crop);
        (crop.w > 0.0 && crop.h > 0.0 && h > 0.0).then_some(Self {
            dest,
            crop,
            aspect: w / h,
        })
    }

    /// Where the footage draws source point `(x, y)`.
    pub(super) fn point(&self, (x, y): (f32, f32)) -> (f64, f64) {
        let (x, y) = (f64::from(x), f64::from(y));
        let (d, c) = (self.dest, self.crop);
        (d.x + (x - c.x) / c.w * d.w, d.y + (y - c.y) / c.h * d.h)
    }

    /// A source-normalized movement, as the footage draws it.
    pub(super) fn vector(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (x / self.crop.w * self.dest.w, y / self.crop.h * self.dest.h)
    }

    /// The length of a canvas-normalized vector, in canvas heights.
    pub(super) fn length(&self, (x, y): (f64, f64)) -> f64 {
        (x * self.aspect).hypot(y)
    }

    /// How many canvas heights the footage draws one source frame's height at (the zoom and the framing).
    pub(super) fn zoom(&self) -> f64 {
        self.dest.h / self.crop.h
    }

    /// `(x, y)` is inside the footage's box grown by one cursor `height` on every side.
    pub(super) fn near(&self, (x, y): (f64, f64), height: f64) -> bool {
        let (d, grow) = (self.dest, height / self.aspect);
        x >= d.x - grow && x <= d.x + d.w + grow && y >= d.y - height && y <= d.y + d.h + height
    }
}
