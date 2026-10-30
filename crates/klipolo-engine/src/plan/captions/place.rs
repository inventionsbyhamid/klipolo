//! Where a caption block sits: the person's own place for that format, never moved; else the
//! style's, kept organic-safe in 9:16 (y 0.15–0.72, lines ≤ 0.70 W) or ad-safe with the switch (y ≤ 0.65,
//! x 120–888 px); Arcade at a portrait split's seam. The plan carries what the project says (`Anchor`), and the drawer
//! places the block with the laid-out page's real size: a two-line page and a one-line page both stay inside the
//! safe band, and a block as wide as the frame allows stays inside it, its outline and the word being said at its
//! active scale included.

use klipolo_model::captions::style::CaptionStyleDef;
use klipolo_model::project::{FormatKind, Project};

const ORGANIC: (f64, f64) = (0.15, 0.72);
const AD_BOTTOM: f64 = 0.65;
const AD_X: (f64, f64) = (120.0 / 1080.0, 888.0 / 1080.0);
const ORGANIC_WIDTH: f64 = 0.70;

/// What the project says about where a format's caption block goes at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    /// The y share where a portrait split's halves meet now (`plan::clip::split_seam`), for a style placed at it.
    pub seam: Option<f64>,
    /// The person's own centre y for this format (`CaptionSettings::place`): never moved.
    pub place: Option<f64>,
    /// The ad-safe switch, in 9:16.
    pub ad_safe: bool,
}

impl Anchor {
    pub fn of(p: &Project, format: FormatKind, seam: Option<f64>) -> Self {
        Self {
            seam,
            place: p.captions.place.get(&format).copied(),
            ad_safe: p.captions.ad_safe && format == FormatKind::Portrait,
        }
    }

    /// The centre (x, y as shares of the frame) of a block `block` (width, height as shares of the frame, its outline
    /// included) drawn in `style`. x is centred (ad-safe: between 120 and 888 px), then kept so the block at the active
    /// word's scale stays inside the frame; y is the person's place, else the style's (at the seam for a style placed
    /// there), clamped into the safe band in 9:16.
    pub fn centre(
        &self,
        style: &CaptionStyleDef,
        format: FormatKind,
        block: (f64, f64),
    ) -> (f64, f64) {
        let wanted = if self.ad_safe {
            (AD_X.0 + AD_X.1) / 2.0
        } else {
            0.5
        };
        let half = block.0 / 2.0 * active_scale(style);
        let x = if half >= 0.5 {
            0.5
        } else {
            wanted.clamp(half, 1.0 - half)
        };
        if let Some(y) = self.place {
            return (x, y);
        }
        let pos = style.position.get(format);
        let mut y = match self.seam {
            Some(seam) if pos.seam == Some(true) => seam,
            _ => pos.y,
        };
        if format == FormatKind::Portrait {
            let hi = if self.ad_safe { AD_BOTTOM } else { ORGANIC.1 };
            let half = block.1 / 2.0;
            y = y.clamp(ORGANIC.0 + half, (hi - half).max(ORGANIC.0 + half));
        }
        (x, y)
    }
}

/// How much the word being said grows (Arcade 1.08, Loud 1.06, Highlight 1.04): at least 1.
pub fn active_scale(style: &CaptionStyleDef) -> f64 {
    style.active.scale.unwrap_or(1.0).max(1.0)
}

/// What a word's ink reaches past its advance on each side, in em of its size: its outline, or the style's box
/// padding (a line's box, or the box behind the word being said), whichever is wider.
pub fn edge_em(style: &CaptionStyleDef) -> f64 {
    let outline = style.outline.as_ref().map_or(0.0, |o| o.width_em);
    let boxed = style.boxes.as_ref().map_or(0.0, |b| b.pad_x_em);
    outline.max(boxed)
}

/// The block's centre as shares of the frame (x, y), for a block `block_h` of the frame's height tall and of no
/// width. `seam`: the y share where a portrait split's halves meet at this moment (`plan::clip::split_seam`), for a
/// style placed at it.
pub fn block_centre(
    p: &Project,
    style: &CaptionStyleDef,
    format: FormatKind,
    block_h: f64,
    seam: Option<f64>,
) -> (f64, f64) {
    Anchor::of(p, format, seam).centre(style, format, (0.0, block_h))
}

/// How wide a line may be, as a share of the frame's width: the style's, and in 9:16 no wider than the organic-safe
/// band (or the ad-safe one with the switch).
pub fn max_width_share(p: &Project, style: &CaptionStyleDef, format: FormatKind) -> f64 {
    let w = style.pages.max_line_width.get(format);
    if format != FormatKind::Portrait {
        return w;
    }
    let cap = if p.captions.ad_safe {
        AD_X.1 - AD_X.0
    } else {
        ORGANIC_WIDTH
    };
    w.min(cap)
}
