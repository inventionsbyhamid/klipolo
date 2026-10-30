//! How wide a text is shaped: against its own whole texture, its box plus `padding` on both sides (at
//! most `MAX_SIDE`), which its box never fills, so it always wraps at box × 1.05 + 4 (`wrap_width`). The canvas editor
//! and the webview's measure follow that rule (`wrapWidth` in `textBox.ts`); the test below holds the wiring to it.

use super::geometry::{MAX_SIDE, padding};

/// Where a text box `box_w` × `box_h` px with a `font` px font, a `stroke` px outline and a `margin` px margin its padded
/// filters draw past it sits in its texture, and what it is shaped against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Shaped {
    /// The room around the box: each side, and top and bottom.
    pub(super) pad: (f32, f32),
    /// The texture's whole width.
    pub(super) full_w: u32,
    /// `PreparedText::layout_width`: the whole width, at most `MAX_SIDE`, whatever window of it is drawn.
    pub(super) layout_width: u32,
}

pub(super) fn shaped(box_w: f32, font: f32, stroke: f32, margin: f32) -> Shaped {
    let (pad_x, pad_y) = padding(f64::from(font), f64::from(stroke), f64::from(box_w));
    let (pad_x, pad_y) = (pad_x as f32 + margin, pad_y as f32 + margin);
    let full_w = (box_w + 2.0 * pad_x) as u32;
    Shaped {
        pad: (pad_x, pad_y),
        full_w,
        layout_width: full_w.clamp(1, MAX_SIDE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor::cap::rendering::layers::text::wrap_width;

    /// Shaped as `lay_out` and `draw` shape it, a text always wraps
    /// at box × 1.05 + 4: a full-width title in 9:16 included, never at the frame's width. This fails if the width a
    /// text is shaped against changes (to the frame's, as Cap meant): the webview's `wrapWidth` would then be wrong.
    #[test]
    fn a_text_wraps_at_its_box_plus_slack_even_across_the_whole_frame() {
        // (box, font, stroke): a 16:9 title full width in a 1080 px 9:16 frame, the 253 px preview's, a narrow box with
        // an outline, and the widest box a 4K frame has.
        for (box_w, font, stroke) in [
            (1080.0, 72.0, 0.0),
            (253.0, 16.9, 0.0),
            (96.0, 24.0, 3.0),
            (3840.0, 400.0, 0.0),
        ] {
            let s = shaped(box_w, font, stroke, 0.0);
            let wrap = wrap_width(box_w, s.layout_width as f32);
            assert!(
                (wrap - (box_w * 1.05 + 4.0)).abs() < 1e-2,
                "a {box_w} px box wraps at {wrap}, not its box plus slack"
            );
        }
    }

    /// A padded filter's margin is room around the box in the texture, on every side, and never changes
    /// where the text wraps.
    #[test]
    fn a_filters_margin_widens_the_texture_but_not_the_wrap() {
        let (plain, padded) = (
            shaped(400.0, 48.0, 0.0, 0.0),
            shaped(400.0, 48.0, 0.0, 24.0),
        );
        assert_eq!(padded.pad, (plain.pad.0 + 24.0, plain.pad.1 + 24.0));
        assert_eq!(padded.full_w, plain.full_w + 48);
        let wrap = |s: Shaped| wrap_width(400.0, s.layout_width as f32);
        assert_eq!(wrap(padded), wrap(plain));
    }
}
