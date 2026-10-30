//! Rectangles for the main footage and the camera, in canvas-normalized units (0–1 on both axes).

use klipolo_model::layout::{BoxN, resolve_rect};
use klipolo_model::project::FormatKind;
use klipolo_model::timeline::{CameraLayout, CameraMode, CameraShape};

pub const FULL: BoxN = BoxN {
    x: 0.0,
    y: 0.0,
    w: 1.0,
    h: 1.0,
};

/// Canvas size in pixels.
pub fn canvas(format: FormatKind) -> (f64, f64) {
    let (w, h) = format.size();
    (f64::from(w), f64::from(h))
}

/// The canvas minus the background padding (a fraction of the shorter side, on every edge).
pub fn content_box(padding: f64, format: FormatKind) -> BoxN {
    let (w, h) = canvas(format);
    let inset = padding.clamp(0.0, 0.4) * w.min(h);
    BoxN {
        x: inset / w,
        y: inset / h,
        w: (w - 2.0 * inset) / w,
        h: (h - 2.0 * inset) / h,
    }
}

/// Top and bottom halves of `area` (portrait split).
pub fn split_panes(area: BoxN) -> (BoxN, BoxN) {
    let half = area.h / 2.0;
    (
        BoxN { h: half, ..area },
        BoxN {
            y: area.y + half,
            h: half,
            ..area
        },
    )
}

/// Width ÷ height of a picture; an unknown size takes `fallback` (no letterbox, no crop).
pub fn aspect_of(width: u32, height: u32, fallback: f64) -> f64 {
    if width == 0 || height == 0 {
        fallback
    } else {
        f64::from(width) / f64::from(height)
    }
}

/// Width ÷ height of `area` in canvas pixels.
pub fn area_aspect(area: BoxN, format: FormatKind) -> f64 {
    let (w, h) = canvas(format);
    (area.w * w) / (area.h * h).max(f64::EPSILON)
}

/// The largest rectangle of `aspect` centred in `area` (Fit).
pub fn fit_rect(area: BoxN, aspect: f64, format: FormatKind) -> BoxN {
    let (cw, ch) = canvas(format);
    let (aw, ah) = (area.w * cw, area.h * ch);
    let (w, h) = if aspect > aw / ah {
        (aw, aw / aspect)
    } else {
        (ah * aspect, ah)
    };
    BoxN {
        x: (area.x * cw + (aw - w) / 2.0) / cw,
        y: (area.y * ch + (ah - h) / 2.0) / ch,
        w: w / cw,
        h: h / ch,
    }
}

/// `crop` (source-normalised) in the other of the file's and the shown picture's coordinates: the same box when the
/// footage isn't mirrored, its mirror image across the frame when it is (the shader flips the picture inside its crop).
pub fn as_shown(mirror: bool, crop: BoxN) -> BoxN {
    if mirror {
        BoxN {
            x: 1.0 - crop.x - crop.w,
            ..crop
        }
    } else {
        crop
    }
}

/// The part of a source (source-normalized) that covers a target of `target_aspect` (Fill). `focus` 0–1 picks which part.
pub fn cover_crop(source_aspect: f64, target_aspect: f64, focus_x: f64, focus_y: f64) -> BoxN {
    if source_aspect > target_aspect {
        let w = target_aspect / source_aspect;
        BoxN {
            x: (1.0 - w) * focus_x.clamp(0.0, 1.0),
            y: 0.0,
            w,
            h: 1.0,
        }
    } else {
        let h = source_aspect / target_aspect;
        BoxN {
            x: 0.0,
            y: (1.0 - h) * focus_y.clamp(0.0, 1.0),
            w: 1.0,
            h,
        }
    }
}

/// `base` (a Fill crop) scaled by 1 / `closer` (1–2) and centred on `centre`, kept inside the source frame: the
/// vertical follow-crop. A centre or `closer` that isn't a number reads as the middle, or 1.
pub fn follow_crop(base: BoxN, centre: (f64, f64), closer: f64) -> BoxN {
    let closer = if closer.is_finite() {
        closer.clamp(1.0, 2.0)
    } else {
        1.0
    };
    let (w, h) = ((base.w / closer).min(1.0), (base.h / closer).min(1.0));
    let at = |c: f64, side: f64| {
        let c = if c.is_finite() { c } else { 0.5 };
        (c - side / 2.0).clamp(0.0, 1.0 - side)
    };
    BoxN {
        x: at(centre.0, w),
        y: at(centre.1, h),
        w,
        h,
    }
}

/// The key a follow path is built and found by for a Fill crop of `base`'s size and `closer`: thousandths, so the
/// loader, preview and export name the same path.
pub fn follow_crop_key(base: BoxN, closer: f64) -> (u16, u16, u16) {
    let milli = |v: f64| (v.clamp(0.0, 60.0) * 1000.0).round() as u16;
    let closer = if closer.is_finite() {
        closer.clamp(1.0, 2.0)
    } else {
        1.0
    };
    (milli(base.w), milli(base.h), milli(closer))
}

/// Corner radius of the camera bubble as a fraction of its shorter side (matches the DOM stage's 16 % rounding).
pub fn bubble_radius(shape: CameraShape) -> f64 {
    match shape {
        CameraShape::Circle => 0.5,
        CameraShape::Rounded => 0.16,
        CameraShape::Square => 0.0,
    }
}

/// Where a recording's camera goes in `format`, or None when it's hidden there.
/// Mirrored by `cameraRect` in `apps/desktop/src/lib/layout.ts` and pinned by `tests/parity.rs`.
pub fn camera_rect_for(
    camera: &CameraLayout,
    padding: f64,
    primary: FormatKind,
    format: FormatKind,
) -> Option<BoxN> {
    match camera.mode_for(format) {
        CameraMode::Bubble => Some(resolve_rect(&camera.bubble, primary, format)),
        CameraMode::SplitBottom => Some(split_panes(content_box(padding, format)).1),
        CameraMode::Hidden => None,
    }
}
