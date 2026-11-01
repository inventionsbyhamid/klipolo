//! A caption page laid out once: each word that isn't hidden shaped in the face
//! the drawer resolved (the style's, or the person's caption font) at its size step and keyword scale
//! (`shaping`), the lines balanced (`klipolo_model::captions::lines`), the page autofitted down to the style's
//! `autofitMin`, and each word given its place and its cell of the page's atlas (`cells`). Never per frame: once per
//! page, look or font change.
//!
//! The caption rule for a word wider than the line (unlike a title's, which breaks it: `Wrap::WordOrGlyph`):
//! a word is never broken or cut. The page shrinks to `autofitMin`, then takes one more line (Loud's rule), and past
//! that keeps its narrowest arrangement at `autofitMin` and draws wider than the line, but never wider than 96% of
//! the frame (`FRAME_SHARE`): only a word that would reach the frame's edge shrinks below `autofitMin`.
//!
//! Cost: one shaping per word not measured before, then `break_lines`' search (at most C(31, 4) sums), at most twice.
//! Words and lines live in fixed-capacity arrays; the page's shown text is one string, and CJK runs are worked out only
//! on a page that has CJK.

use std::ops::Range;
use std::sync::Arc;

use arrayvec::ArrayVec;
use glyphon::FontSystem;
use klipolo_model::captions::Pages;
use klipolo_model::captions::lines::{LineInput, MAX_LINES, break_lines, fits, pyramid};
use klipolo_model::captions::style::CaptionStyleDef;
use klipolo_model::project::FormatKind;

use super::cells::{pack_cells, pad, place_words};
use super::measure::space_width;
use super::shaping::measure_words;
use crate::plan::captions::place::{active_scale, edge_em};
use crate::render::fonts::FaceChoice;

/// The most words a page holds (`klipolo_model::captions::pages::MAX_PAGE_WORDS`).
pub const MAX_WORDS: usize = 32;
/// The most of the frame's width a block may take, its outline included: a word too wide for that shrinks below the
/// style's `autofitMin` rather than reach the frame's edge.
pub const FRAME_SHARE: f32 = 0.96;

/// One word of the laid-out page: `PageLayout::words[k]` is the page's k-th word that isn't hidden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordBox {
    /// Its box's centre, px from the block's centre.
    pub centre: [f32; 2],
    /// Its box: its advance by its face's ascent plus descent, px at the laid-out size.
    pub size: [f32; 2],
    /// Its cell in the atlas (x, y, w, h): the box centred in it, with `PageLayout::pad` on every side.
    pub cell: [u32; 4],
    pub line: u8,
    /// Its size step and keyword scale (over `font_px × autofit`).
    pub scale: f32,
    /// `PageLayout::faces[face]`: the page's face, or the style's Cyrillic one.
    pub face: u8,
    /// Its bytes in `PageLayout::text`.
    pub text: [u32; 2],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageLayout {
    pub words: ArrayVec<WordBox, MAX_WORDS>,
    /// Each line's rect (left, top, right, bottom), px from the block's centre, the box padding included when the
    /// style boxes lines.
    pub lines: ArrayVec<[f32; 4], MAX_LINES>,
    /// The widest line by the lines' height, px.
    pub block: [f32; 2],
    /// The style's size in this output, before the autofit.
    pub font_px: f32,
    pub autofit: f32,
    /// Room around each word's box in its cell for the outline, the shadow's offset and the blurs, px.
    pub pad: f32,
    /// The atlas's size, px.
    pub atlas: (u32, u32),
    /// The faces the words are shaped in: the page's first, then the style's Cyrillic stand-in when a word uses it.
    pub faces: ArrayVec<FaceChoice, 2>,
    /// The CJK runs of a word on a CJK page (its index in `words`, bytes of its text, the family asked): taken from the
    /// page's shown text as a whole. Empty on every other page.
    pub runs: Vec<(u8, Range<usize>, Arc<str>)>,
    /// The words drawn, as shown (a space where the page's text has one): each word's text is `word_text`, so the atlas
    /// draws exactly the words laid out.
    pub text: String,
}

impl PageLayout {
    /// What `w` shows.
    pub fn word_text(&self, w: &WordBox) -> &str {
        self.text
            .get(w.text[0] as usize..w.text[1] as usize)
            .unwrap_or_default()
    }

    /// The CJK runs of word `k`, rebased to its text, as the glyph layer takes them.
    pub fn runs_of(&self, k: usize) -> Vec<(Range<usize>, Arc<str>)> {
        self.runs
            .iter()
            .filter(|(i, _, _)| usize::from(*i) == k)
            .map(|(_, r, f)| (r.clone(), f.clone()))
            .collect()
    }
}

/// `page` of `pages` laid out for an output of `out` pixels in `format`, its lines no wider than `max_width_share` of
/// the output's width (`plan::captions::place::max_width_share`). `face` is the face the drawer resolved for the
/// captions; `language` the text language its CJK follows (`PlanContext::text_language`).
#[allow(clippy::too_many_arguments)]
pub fn lay_out_page(
    fs: &mut FontSystem,
    style: &CaptionStyleDef,
    face: &FaceChoice,
    language: &str,
    pages: &Pages,
    page: usize,
    format: FormatKind,
    out: (u32, u32),
    max_width_share: f64,
) -> PageLayout {
    let mut layout = PageLayout {
        autofit: 1.0,
        ..PageLayout::default()
    };
    let Some(pg) = pages.pages.get(page) else {
        return layout;
    };
    layout.font_px = (style.size.size(format) * f64::from(out.0.min(out.1)) / 1080.0) as f32;
    layout.faces.push(face.clone());
    let mut words = measure_words(
        fs,
        style,
        language,
        pages,
        pg,
        layout.font_px,
        &mut layout.faces,
        &mut layout.runs,
    );
    if words.widths.is_empty() {
        return layout;
    }
    let spacing = style.letter_spacing_em as f32;
    let space = if words.spaced {
        space_width(fs, face, layout.font_px, spacing)
    } else {
        0.0
    };
    let max_width = (max_width_share * f64::from(out.0)) as f32;
    // Clean's bottom-heavy lines, as its caption files break them.
    let pyramid = pyramid(style);
    let input = |max_lines: usize| LineInput {
        widths: &words.widths,
        space,
        max_width,
        max_lines,
        forced: &words.forced,
        short: &words.short,
        pyramid,
    };
    let least = (style.autofit_min as f32).clamp(0.05, 1.0);
    let max_lines = (style.pages.max_lines as usize).clamp(1, MAX_LINES - 1);
    let mut breaks = break_lines(&input(max_lines));
    if !fits(breaks.widest * least, max_width) {
        // Loud's rule: one more line, kept when it fits at the minimum, or else when it is the narrower.
        let more = break_lines(&input(max_lines + 1));
        if more.widest < breaks.widest {
            breaks = more;
        }
    }
    let biggest = words.scales.iter().copied().fold(1.0, f32::max);
    layout.autofit = if breaks.widest > 0.0 {
        // Below `autofitMin` only so the block, its outline (or box padding) included, stays inside the frame, also
        // while the word being said is drawn at its active scale: a word wider than the line draws wider than it, but
        // never past the frame's edge, which would cut it.
        let edge = edge_em(style) as f32;
        let frame = FRAME_SHARE * out.0 as f32;
        let grown =
            (breaks.widest + 2.0 * edge * layout.font_px * biggest) * active_scale(style) as f32;
        let inside = frame / grown;
        (max_width / breaks.widest).clamp(least, 1.0).min(inside)
    } else {
        1.0
    };
    layout.pad = pad(style, layout.font_px * layout.autofit * biggest);
    place_words(&mut layout, style, &breaks.starts, &words, space);
    layout.block[0] = breaks.widest * layout.autofit;
    pack_cells(&mut layout);
    layout.text = std::mem::take(&mut words.shown);
    layout
}
