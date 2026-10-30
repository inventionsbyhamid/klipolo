// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/text.rs
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: cap_project imports replaced by compat TextAlign/TextBackgroundStyle (Klipolo builds PreparedText itself and animates in FramePlan); Cap's animation and segment code removed (REFERENCE_HEIGHT, parse_color/parse_rgb_color, ease_out_back, POP_MIN_SCALE/ZOOM_MAX_SCALE/BOUNCE_TRAVEL/TRACKING_TRAVEL_EM, AnimSample/ANIM_REST, sample_animation, edge_progress, reveal_content, prepare_texts) with the tests that built TextSegments; the background_rect/highlight_rect tests are kept.; `layout_width` added (the output width a window of a larger layout wraps against); `highlight` added (Karaoke captions: bytes drawn in another colour); `runs` added (each stretch of the text and the family it asks for: Chinese, Japanese and Korean in their language's face); PreparedText.stretch added (the face's width class).

pub use crate::vendor::cap::compat::{TextAlign, TextBackgroundStyle};

pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 480.0;

#[derive(Debug, Clone)]
pub struct PreparedText {
    pub track: u32,
    pub content: String,
    pub bounds: [f32; 4],
    pub color: [f32; 4],
    pub background_color: Option<[f32; 4]>,
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: f32,
    pub italic: bool,
    /// Klipolo: the face's OS/2 width class (5 normal): a condensed installed face is found only when asked condensed.
    pub stretch: u16,
    pub opacity: f32,
    pub align: TextAlign,
    /// Output px, already scaled from the 1080p reference.
    pub letter_spacing: f32,
    /// Multiplier of `font_size`.
    pub line_height: f32,
    /// 0..1 drop-shadow strength; 0 disables the shadow pass.
    pub shadow: f32,
    /// Animation translation in output px, applied to the whole block.
    pub offset: [f32; 2],
    /// Animation scale about the box center.
    pub scale: f32,
    /// Outline (output px, colour); `None` when the width is 0.
    pub stroke: Option<(f32, [f32; 4])>,
    /// 0..1 halo strength in the text colour; 0 disables the glow passes.
    pub glow: f32,
    /// Right-hand colour of a horizontal gradient that starts at `color`.
    pub gradient_color: Option<[f32; 4]>,
    pub background_style: TextBackgroundStyle,
    /// Fraction of the wrap width left visible by a wipe; 1 = unclipped.
    pub wipe: f32,
    pub stagger_in: Option<StaggerEdge>,
    pub stagger_out: Option<StaggerEdge>,
    /// Klipolo: the output width the wrap slack is judged against when the texture is a window of a larger layout;
    /// None: the texture's own width.
    pub layout_width: Option<f32>,
    /// Klipolo: bytes of `content` the text's own pass draws in another colour (Karaoke captions: the word being said).
    /// The outline, shadow and glow passes keep theirs. Clamped to `content`.
    pub highlight: Option<(std::ops::Range<usize>, [f32; 4])>,
    /// Klipolo: the text's stretches (byte ranges, covering it) and the family each asks for at the text's weight and
    /// style: CJK in its language's fallback face. Empty when the whole text asks for `font_family`.
    pub runs: Vec<(std::ops::Range<usize>, std::sync::Arc<str>)>,
}

/// One edge of a per-word / per-letter reveal: `progress` runs 0 → 1 toward
/// fully visible, and [`stagger_alpha`] turns it into a unit's alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaggerEdge {
    pub by_word: bool,
    pub progress: f32,
}

/// Every unit gets the same-length window; unit `i` of `n` opens its window
/// at `i / n` of the remaining span so the last unit finishes exactly at 1.
pub fn stagger_alpha(edge: StaggerEdge, unit: usize, units: usize) -> f32 {
    if edge.progress >= 1.0 || units == 0 {
        return 1.0;
    }
    const WINDOW: f32 = 0.55;
    let start = (unit as f32 / units as f32) * (1.0 - WINDOW);
    ease_out_cubic(((edge.progress - start) / WINDOW).clamp(0.0, 1.0))
}

/// The block-level background behind a text box. A pill widens the side
/// padding and rounds to half its height.
pub(crate) fn background_rect(
    bounds: [f32; 4],
    laid_out_height: f32,
    font_size: f32,
    center: [f32; 2],
    scale: f32,
    offset: [f32; 2],
    pill: bool,
) -> ([f32; 4], f32) {
    let padding = font_size * 0.2;
    let padding_x = if pill { font_size * 0.5 } else { padding };
    let left = bounds[0] - padding_x;
    let top = bounds[1] - padding;
    let right = bounds[2] + padding_x;
    let bottom = bounds[1] + (bounds[3] - bounds[1]).max(laid_out_height) + padding;
    let [cx, cy] = center;
    let transform = |x: f32, y: f32| {
        [
            cx + (x - cx) * scale + offset[0],
            cy + (y - cy) * scale + offset[1],
        ]
    };
    let [transformed_left, transformed_top] = transform(left, top);
    let [transformed_right, transformed_bottom] = transform(right, bottom);
    let width = (transformed_right - transformed_left).max(0.0);
    let height = (transformed_bottom - transformed_top).max(0.0);
    let radius = if pill {
        height * 0.5
    } else {
        font_size * 0.15 * scale
    };
    (
        [transformed_left, transformed_top, width, height],
        radius.min(width * 0.5).min(height * 0.5),
    )
}

/// One marker stroke behind a laid-out line: `ink` is the line's glyph extent
/// (left, right) and `line` its top and height, all in unscaled buffer space.
pub(crate) fn highlight_rect(
    ink: (f32, f32),
    line: (f32, f32),
    font_size: f32,
    box_center: [f32; 2],
    scale: f32,
    offset: [f32; 2],
) -> ([f32; 4], f32) {
    let padding_x = font_size * 0.12;
    let padding_y = font_size * 0.06;
    let transform = |x: f32, y: f32| {
        [
            box_center[0] + (x - box_center[0]) * scale + offset[0],
            box_center[1] + (y - box_center[1]) * scale + offset[1],
        ]
    };
    let [left, top] = transform(ink.0 - padding_x, line.0 - padding_y);
    let [right, bottom] = transform(ink.1 + padding_x, line.0 + line.1 + padding_y);
    let width = (right - left).max(0.0);
    let height = (bottom - top).max(0.0);
    (
        [left, top, width, height],
        (font_size * 0.12 * scale)
            .min(width * 0.5)
            .min(height * 0.5),
    )
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_rect_expands_authored_bounds_then_transforms_about_center() {
        let (rect, radius) = background_rect(
            [100.0, 200.0, 300.0, 400.0],
            100.0,
            50.0,
            [200.0, 300.0],
            0.5,
            [10.0, -20.0],
            false,
        );
        assert_eq!(rect, [155.0, 225.0, 110.0, 110.0]);
        assert!((radius - 3.75).abs() < 0.000_001);
    }

    #[test]
    fn pill_background_widens_and_rounds_to_half_height() {
        let (rect, radius) = background_rect(
            [100.0, 200.0, 300.0, 260.0],
            60.0,
            50.0,
            [200.0, 230.0],
            1.0,
            [0.0, 0.0],
            true,
        );
        assert_eq!(rect, [75.0, 190.0, 250.0, 80.0]);
        assert_eq!(radius, 40.0);
    }

    #[test]
    fn highlight_rect_hugs_one_line_of_ink() {
        let (rect, radius) = highlight_rect(
            (100.0, 200.0),
            (50.0, 60.0),
            50.0,
            [150.0, 80.0],
            1.0,
            [0.0, 0.0],
        );
        assert_eq!(rect, [94.0, 47.0, 112.0, 66.0]);
        assert_eq!(radius, 6.0);
    }
}
