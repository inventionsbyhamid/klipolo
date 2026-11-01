//! Edges cut off a picture: a main clip's footage, or a video layer's, before it is framed. The picture that is left
//! keeps its own shape, so Fit shows all of it and Fill covers its area from it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::layout::BoxN;

/// The least share of the picture a crop keeps, across and down: a tenth.
pub const MIN_KEPT: f64 = 0.1;

/// How much is cut off each edge of a picture as it is shown (upright, and mirrored when it is drawn mirrored): left
/// and right as shares of its width, top and bottom of its height. Each is 0 or more, and what is left is at least
/// [`MIN_KEPT`] each way.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Crop {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Crop {
    /// Nothing cut.
    pub const NONE: Crop = Crop {
        left: 0.0,
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
    };

    /// Whether it cuts nothing.
    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    /// Whether every edge is a number, none below 0, and what is left is at least `MIN_KEPT` each way.
    pub fn is_valid(&self) -> bool {
        let edges = [self.left, self.top, self.right, self.bottom];
        // A hair under the tenth: a crop typed as shares that add up to 0.9 must not fail on rounding.
        let keeps = |a: f64, b: f64| 1.0 - a - b >= MIN_KEPT - 1e-9;
        edges.iter().all(|e| e.is_finite() && *e >= 0.0)
            && keeps(self.left, self.right)
            && keeps(self.top, self.bottom)
    }

    /// The part of the picture kept, picture-normalised (0 to 1 on both axes, as shown).
    pub fn kept(&self) -> BoxN {
        BoxN {
            x: self.left,
            y: self.top,
            w: 1.0 - self.left - self.right,
            h: 1.0 - self.top - self.bottom,
        }
    }

    /// Width ÷ height of what is left of a picture of `aspect`.
    pub fn aspect_of(&self, aspect: f64) -> f64 {
        let kept = self.kept();
        aspect * kept.w / kept.h
    }
}

/// `inner` (normalised to `outer`) in `outer`'s own coordinates: a crop of a crop.
pub fn within(outer: BoxN, inner: BoxN) -> BoxN {
    BoxN {
        x: outer.x + inner.x * outer.w,
        y: outer.y + inner.y * outer.h,
        w: inner.w * outer.w,
        h: inner.h * outer.h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crop(left: f64, top: f64, right: f64, bottom: f64) -> Crop {
        Crop {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn no_crop_keeps_the_whole_picture() {
        assert!(Crop::NONE.is_none());
        assert!(Crop::default().is_none());
        assert!(Crop::NONE.is_valid());
        assert_eq!(Crop::NONE.kept(), BoxN::FULL);
        assert_eq!(Crop::NONE.aspect_of(16.0 / 9.0), 16.0 / 9.0);
    }

    #[test]
    fn a_crop_keeps_what_its_edges_leave() {
        let c = crop(0.1, 0.2, 0.3, 0.0);
        assert!(!c.is_none());
        assert!(c.is_valid());
        let kept = c.kept();
        assert!((kept.x - 0.1).abs() < 1e-12 && (kept.y - 0.2).abs() < 1e-12);
        assert!((kept.w - 0.6).abs() < 1e-12 && (kept.h - 0.8).abs() < 1e-12);
        // 16:9 with 60 % of its width and 80 % of its height left.
        assert!((c.aspect_of(16.0 / 9.0) - (16.0 / 9.0) * 0.6 / 0.8).abs() < 1e-12);
    }

    #[test]
    fn a_crop_keeps_at_least_a_tenth_each_way() {
        assert!(crop(0.45, 0.0, 0.45, 0.0).is_valid());
        assert!(crop(0.0, 0.3, 0.0, 0.6).is_valid());
        assert!(!crop(0.5, 0.0, 0.45, 0.0).is_valid());
        assert!(!crop(0.0, 0.95, 0.0, 0.0).is_valid());
        assert!(!crop(-0.1, 0.0, 0.0, 0.0).is_valid());
        assert!(!crop(f64::NAN, 0.0, 0.0, 0.0).is_valid());
        assert!(!crop(0.0, 0.0, f64::INFINITY, 0.0).is_valid());
    }

    #[test]
    fn a_box_within_another_is_put_in_its_coordinates() {
        let outer = BoxN {
            x: 0.2,
            y: 0.1,
            w: 0.5,
            h: 0.8,
        };
        let inner = BoxN {
            x: 0.5,
            y: 0.25,
            w: 0.5,
            h: 0.5,
        };
        let got = within(outer, inner);
        assert!((got.x - 0.45).abs() < 1e-12 && (got.y - 0.3).abs() < 1e-12);
        assert!((got.w - 0.25).abs() < 1e-12 && (got.h - 0.4).abs() < 1e-12);
        assert_eq!(within(BoxN::FULL, inner), inner);
        assert_eq!(within(outer, BoxN::FULL), outer);
    }
}
