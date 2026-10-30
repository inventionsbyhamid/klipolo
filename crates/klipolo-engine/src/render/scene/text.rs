//! Scene text: a text node shaped through the one face resolver (`family_for`, the faces every title
//! draws in), laid out once into pieces and units (`TextUnits`) when its text, font or box size changes, never per
//! frame. `fit: shrink` steps its size down to half until it fits its box on its own lines, then wraps within `wrap` at
//! half, then clips to the box. Texts from shared projects are held to caps first: at most 500 characters a node
//! (and 2,000 a scene, `room`), a cluster at most 32 characters (a 10,000-mark pile is cut), and the cut is said.

use std::ops::Range;
use std::sync::Arc;

use glyphon::cosmic_text::LayoutGlyph;
use klipolo_fonts::index::FontIndex;
use klipolo_model::font::{FontRef, FontSource};
use klipolo_model::scene::{Align, Fit, RevealBy, SceneLimits, Split};

use super::text_caps::capped;
use super::text_shape::{Shaping, place, shape};
use super::text_units::{Grain, cut};
use crate::render::fonts::{FaceChoice, family_for};
use crate::render::shared_fonts::SharedFonts;
use crate::render::text::faces::runs_and_notes;

pub use super::text_caps::{MAX_CLUSTER_CHARS, MAX_PIECES};
pub(super) use super::text_font::own_look;
pub use super::text_font::{Bindings, NoBindings, NodeBindings, font_ref};
pub use super::text_units::Piece;

/// The family a text without a `font` draws in, as every title's default.
pub const DEFAULT_FAMILY: &str = "Inter";
/// A piece that moves with no unit (the text isn't split, or its grain has none).
pub const NO_UNIT: u16 = u16::MAX;

/// The largest em a scene text is laid out or rasterised at, in target pixels, however far a layer is scaled.
pub const MAX_EM_PX: f32 = 2048.0;

/// One look of a text: its own (first) or a span's, at a size in 1080p pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLook {
    pub font: FontRef,
    pub weight: u16,
    pub italic: bool,
    pub size: f32,
}

/// A text node as it is laid out, its settings resolved: its looks, spacing (1080p pixels), line height (a multiple
/// of the size), alignment, wrap width (a share of the box and 1080p pixels), fit, split and typewriter.
#[derive(Clone, Debug, PartialEq)]
pub struct CTextNode {
    pub looks: Vec<TextLook>,
    pub letter_spacing: f32,
    pub line_height: f32,
    pub align: Option<Align>,
    pub wrap: Option<[f64; 2]>,
    pub fit: Option<Fit>,
    pub split: Option<Split>,
    pub reveal: Option<RevealBy>,
    /// The text language its CJK follows.
    pub language: Arc<str>,
    /// An emoji image node: one picture, as big as its box allows.
    pub emoji: bool,
}

/// What a laid-out text tells the frames that draw it: a stand-in face, CJK whose Noto Sans isn't downloaded (a bit
/// per `CjkScript::index`), and text cut by the caps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextNotes {
    pub stand_in: bool,
    pub cjk_missing: u8,
    pub cut: bool,
}

impl TextNotes {
    pub fn join(&mut self, other: TextNotes) {
        self.stand_in |= other.stand_in;
        self.cjk_missing |= other.cjk_missing;
        self.cut |= other.cut;
    }
}

/// A text laid out once: its pieces in reading order, its split units (each one's box: what it moves about), its
/// typewriter steps, its lines, the node's size after `fit`, whether it clips to its box, and its glyphs (in the box,
/// for the atlas).
#[derive(Clone, Debug, Default)]
pub struct TextUnits {
    pub text: String,
    pub pieces: Vec<Piece>,
    pub units: usize,
    pub unit_rects: Vec<[f32; 4]>,
    pub reveal_units: usize,
    pub lines: Vec<[f32; 4]>,
    /// Each line's glyphs (an animated letter spacing spreads them about the line's alignment).
    pub line_glyphs: Vec<u16>,
    pub font_px: f32,
    /// The fit's scale: 1, or as far down as 0.5.
    pub scale: f32,
    pub clip: bool,
    /// Where an animated size scales the block about: its alignment's edge and its middle.
    pub anchor: [f32; 2],
    /// Where its lines stand in the box: 0 left, 0.5 centred, 1 right (a right-to-left paragraph's own, unaligned).
    pub align_share: f32,
    pub glyphs: Vec<LayoutGlyph>,
    pub notes: TextNotes,
}

/// How a laid-out text is cut and sized beyond its node's own settings.
#[derive(Clone, Copy, Debug)]
pub(super) struct Options {
    /// Characters it may show (500 a node, less when the scene's 2,000 run short).
    pub room: usize,
    /// It is cut at least this finely (a scramble's and a counter's letters are swapped one at a time).
    pub grain: Option<Grain>,
    /// Its fit's scale, decided by another layout (a counter's widest number).
    pub scale: Option<f32>,
    pub tabular: bool,
    /// Its em at most, in target pixels (`MAX_EM_PX`, less in a smaller target).
    pub max_px: f32,
}

impl Options {
    pub(super) const NODE: Self = Self {
        room: SceneLimits::MAX_TEXT_NODE,
        grain: None,
        scale: None,
        tabular: false,
        max_px: MAX_EM_PX,
    };
}

fn grain_of(split: Option<Split>) -> Option<Grain> {
    split.map(|s| match s {
        Split::Chars => Grain::Chars,
        Split::Words => Grain::Words,
        Split::Lines => Grain::Lines,
    })
}

impl TextUnits {
    /// `text` of `node` laid out in a box of `box_px` target pixels, `px` of them per 1080p pixel: units by grapheme
    /// cluster, word or line with their rects in the box, the size after `fit`, each look in the face `family_for`
    /// picks. Only when the text, the font or the box's pixel size changes, never per frame.
    pub fn lay_out(
        fonts: &mut SharedFonts,
        index: &FontIndex,
        node: &CTextNode,
        text: &str,
        box_px: (f32, f32),
        px: f32,
    ) -> TextUnits {
        Self::lay_out_parts(fonts, index, node, &[(text, 0)], box_px, px, Options::NODE)
    }

    /// [`lay_out`](Self::lay_out) of a text made of `parts`, each in its look (its spans).
    pub(super) fn lay_out_parts(
        fonts: &mut SharedFonts,
        index: &FontIndex,
        node: &CTextNode,
        parts: &[(&str, u8)],
        box_px: (f32, f32),
        px: f32,
        opts: Options,
    ) -> TextUnits {
        super::text_counts::count_layout();
        let (text, spans, cut_by_caps) = capped(parts, opts.room);
        let mut out = TextUnits {
            scale: 1.0,
            notes: TextNotes {
                cut: cut_by_caps,
                ..TextNotes::default()
            },
            ..TextUnits::default()
        };
        if text.is_empty() || node.looks.is_empty() {
            out.text = text;
            return out;
        }
        let (bw, bh) = (box_px.0.max(1.0), box_px.1.max(1.0));
        let faces: Vec<FaceChoice> = node
            .looks
            .iter()
            .map(|l| family_for(index, &l.font, l.weight, l.italic))
            .collect();
        let scanning = !fonts.scanned()
            && node
                .looks
                .iter()
                .any(|l| l.font.source == FontSource::Installed);
        let (runs, notes) = runs_and_notes(&text, &faces[0], &node.language, scanning);
        out.notes.stand_in = notes.stand_in() || faces[1..].iter().any(|f| f.stand_in && !scanning);
        out.notes.cjk_missing = notes.cjk_missing();
        let lh = if node.line_height.is_finite() && node.line_height > 0.0 {
            node.line_height
        } else {
            1.2
        };
        let sizes: Vec<f32> = if node.emoji {
            vec![(bw.min(bh) / lh).clamp(1.0, opts.max_px)]
        } else {
            let max = opts.max_px.max(1.0);
            node.looks
                .iter()
                .map(|l| (l.size * px).clamp(1.0, max))
                .collect()
        };
        let spacing: Vec<f32> = node
            .looks
            .iter()
            .map(|l| node.letter_spacing / l.size.max(1.0))
            .collect();
        let grain = [
            grain_of(node.split),
            node.reveal.map(reveal_grain),
            opts.grain,
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(Grain::Lines);
        let shaping = Shaping {
            text: &text,
            spans: &spans,
            faces: &faces,
            sizes: &sizes,
            runs: &runs,
            primary: &faces[0].family,
            letter_spacing_em: &spacing,
            line_height: lh,
            no_ligatures: grain == Grain::Chars,
            tabular: opts.tabular,
        };
        let wrap_w = node
            .wrap
            .map_or(bw, |[f, p]| (f as f32 * bw + p as f32 * px).max(1.0));
        let fs = fonts.system();
        let fit = if node.emoji {
            Some(Fit::Shrink)
        } else {
            node.fit
        };
        let (scale, mut lines) = match (opts.scale, fit) {
            (Some(s), _) => (s, shape(fs, &shaping, s, None)),
            (None, Some(Fit::Shrink)) => {
                // Down to half until it fits the box on its own lines, then wrapped within `wrap` at half.
                let natural = shape(fs, &shaping, 1.0, None);
                let (w, h) = natural.extent();
                let s = (wrap_w / w.max(1e-3)).min(bh / h.max(1e-3)).min(1.0);
                if s >= 1.0 {
                    (1.0, natural)
                } else if s >= 0.5 {
                    (s * 0.995, shape(fs, &shaping, s * 0.995, None))
                } else {
                    (0.5, shape(fs, &shaping, 0.5, Some(wrap_w)))
                }
            }
            (None, _) => (1.0, shape(fs, &shaping, 1.0, Some(wrap_w))),
        };
        out.scale = scale;
        out.font_px = sizes[0] * scale;
        out.clip = fit.is_some();
        let align = match (node.emoji, node.align) {
            (true, _) => Some(Align::Center),
            (_, a) => a,
        };
        place(&mut lines, align, (bw, bh));
        // Where its lines stand: by `align`, else by its first paragraph's direction (right for right-to-left, m3).
        let rtl = lines.lines.first().is_some_and(|l| l.rtl);
        out.align_share = match align {
            Some(Align::Center) => 0.5,
            Some(Align::Right) => 1.0,
            Some(Align::Left) => 0.0,
            None if rtl => 1.0,
            None => 0.0,
        };
        let anchor_x = out.align_share * bw;
        let (_, block_h) = lines.extent();
        let top = lines.lines.first().map_or(0.0, |l| l.top);
        out.anchor = [anchor_x, top + block_h / 2.0];
        let split = grain_of(node.split);
        let reveal = node.reveal.map(reveal_grain);
        let mut c = cut(&lines, &text, &spans, grain, split, reveal);
        if c.pieces.len() > MAX_PIECES {
            c.pieces.truncate(MAX_PIECES);
            out.notes.cut = true;
        }
        out.pieces = c.pieces;
        out.units = c.units.min(SceneLimits::MAX_TEXT_NODE);
        out.unit_rects = c.unit_rects;
        out.reveal_units = c.reveal_units;
        out.lines = c.lines;
        out.line_glyphs = c.line_glyphs;
        out.glyphs = lines.glyphs;
        out.text = text;
        out
    }
}

fn reveal_grain(by: RevealBy) -> Grain {
    match by {
        RevealBy::Chars => Grain::Chars,
        RevealBy::Words => Grain::Words,
    }
}

/// The byte range of `piece` in `units`' text.
pub fn piece_text<'a>(units: &'a TextUnits, piece: &Piece) -> &'a str {
    let r: Range<usize> = piece.bytes[0] as usize..piece.bytes[1] as usize;
    units.text.get(r).unwrap_or_default()
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
