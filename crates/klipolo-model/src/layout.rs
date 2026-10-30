use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::motion::{Blend, Motion};
use crate::project::FormatKind;

/// Which part of the canvas an item belongs to; drives re-layout between formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    MiddleLeft,
    Center,
    MiddleRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// Normalized rectangle (canvas = 0..1 on both axes), top-left origin.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[ts(export)]
pub struct BoxN {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl BoxN {
    /// The whole canvas.
    pub const FULL: BoxN = BoxN {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
}

/// Placement of a visual item. `rect` is relative to the project's primary format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Layout {
    pub anchor: Anchor,
    pub rect: BoxN,
    /// Degrees, clockwise.
    pub rotation: f64,
    pub opacity: f64,
    /// Corner radius as a fraction of the shorter side.
    pub radius: f64,
    /// Hand-placed rectangles for non-primary formats.
    #[serde(default)]
    pub overrides: BTreeMap<FormatKind, BoxN>,
    /// Keys and presets; left out of the JSON when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub motion: Option<Motion>,
    /// How it mixes with what is under it; None is normal (kept out of the JSON, so every older file and TS literal is
    /// unchanged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub blend: Option<Blend>,
}

impl Layout {
    pub fn new(anchor: Anchor, rect: BoxN) -> Self {
        Self {
            anchor,
            rect,
            rotation: 0.0,
            opacity: 1.0,
            radius: 0.0,
            overrides: BTreeMap::new(),
            motion: None,
            blend: None,
        }
    }

    /// A layer that fills the frame in every format: the whole canvas as its rect and as its own rect in every format
    /// there is, listed or not. A rect that only covers the primary's canvas is derived elsewhere by `resolve_rect`
    /// (sizes follow the shorter side), which in 9:16 makes a 16:9 canvas the square at the top. Place every
    /// motion graphic template with it (`klipolo_model::presets::SCENE_PRESETS`): their nodes are laid out in the frame.
    pub fn full_frame() -> Self {
        let mut layout = Self::new(Anchor::TopLeft, BoxN::FULL);
        for format in [
            FormatKind::Landscape,
            FormatKind::Portrait,
            FormatKind::Square,
        ] {
            layout.overrides.insert(format, BoxN::FULL);
        }
        layout
    }

    /// The blend mode, normal when unset.
    pub fn blend_mode(&self) -> Blend {
        self.blend.unwrap_or_default()
    }
}

#[derive(Clone, Copy)]
enum Band {
    Start,
    Middle,
    End,
}

fn bands(anchor: Anchor) -> (Band, Band) {
    use Anchor::*;
    let col = match anchor {
        TopLeft | MiddleLeft | BottomLeft => Band::Start,
        TopCenter | Center | BottomCenter => Band::Middle,
        TopRight | MiddleRight | BottomRight => Band::End,
    };
    let row = match anchor {
        TopLeft | TopCenter | TopRight => Band::Start,
        MiddleLeft | Center | MiddleRight => Band::Middle,
        BottomLeft | BottomCenter | BottomRight => Band::End,
    };
    (col, row)
}

/// Place one axis: keep the margin (start/end) or centre offset (middle), scaled by `s`.
fn place(band: Band, pos_px: f64, size_px: f64, from: f64, to: f64, s: f64, new_size: f64) -> f64 {
    let placed = match band {
        Band::Start => pos_px * s,
        Band::Middle => (pos_px + size_px / 2.0 - from / 2.0) * s + to / 2.0 - new_size / 2.0,
        Band::End => to - (from - (pos_px + size_px)) * s - new_size,
    };
    placed.clamp(0.0, (to - new_size).max(0.0))
}

/// Rectangle for `target` derived from a layout authored in `primary`.
/// Sizes scale with the canvas's shorter side; positions keep their anchor's margins.
pub fn resolve_rect(layout: &Layout, primary: FormatKind, target: FormatKind) -> BoxN {
    if let Some(rect) = layout.overrides.get(&target) {
        return *rect;
    }
    if target == primary {
        return layout.rect;
    }
    let (wp, hp) = primary.size();
    let (wt, ht) = target.size();
    let (wp, hp, wt, ht) = (f64::from(wp), f64::from(hp), f64::from(wt), f64::from(ht));
    let s = wt.min(ht) / wp.min(hp);
    let r = layout.rect;
    let (w_src, h_src) = (r.w * wp, r.h * hp);
    let w_px = (w_src * s).min(wt);
    let h_px = (h_src * s).min(ht);
    let (col, row) = bands(layout.anchor);
    let x_px = place(col, r.x * wp, w_src, wp, wt, s, w_px);
    let y_px = place(row, r.y * hp, h_src, hp, ht, s, h_px);
    BoxN {
        x: x_px / wt,
        y: y_px / ht,
        w: w_px / wt,
        h: h_px / ht,
    }
}

/// Nearest of the nine anchors to the rectangle's centre.
pub fn anchor_for_rect(rect: &BoxN) -> Anchor {
    use Anchor::*;
    let band = |v: f64| {
        if v < 1.0 / 3.0 {
            0
        } else if v < 2.0 / 3.0 {
            1
        } else {
            2
        }
    };
    match (band(rect.y + rect.h / 2.0), band(rect.x + rect.w / 2.0)) {
        (0, 0) => TopLeft,
        (0, 1) => TopCenter,
        (0, _) => TopRight,
        (1, 0) => MiddleLeft,
        (1, 1) => Center,
        (1, _) => MiddleRight,
        (_, 0) => BottomLeft,
        (_, 1) => BottomCenter,
        _ => BottomRight,
    }
}
