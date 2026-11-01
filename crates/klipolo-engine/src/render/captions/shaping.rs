//! A caption page's words shaped: each word that isn't hidden, in the page's face at its size step
//! and keyword scale, a Cyrillic word in the style's Cyrillic stand-in, and a page's Chinese, Japanese or Korean in
//! runs read across the page as a whole. Each word is measured through a cache (`measure`), so laying a page out
//! again after a look change shapes only the words it changed.

use std::ops::Range;
use std::sync::Arc;

use arrayvec::ArrayVec;
use glyphon::FontSystem;
use klipolo_fonts::cjk::in_cjk_blocks;
use klipolo_fonts::unicode_script::{Script, UnicodeScript};
use klipolo_model::captions::lines::{drawn_words, is_short};
use klipolo_model::captions::style::CaptionStyleDef;
use klipolo_model::captions::{Page, Pages};
use klipolo_model::font::FontRef;

use super::layout::MAX_WORDS;
use super::measure::measure;
use crate::render::fonts::{self, FaceChoice, family_for};
use crate::render::script_runs::runs;

/// What "Emphasis" scales a word by in a style without keywords (in those with keywords it makes the word one).
pub const EMPHASIS_SCALE: f32 = 1.1;

/// A CJK run of a word: its index among the page's drawn words, bytes of its text, the family it asks for.
pub type WordRun = (u8, Range<usize>, Arc<str>);

/// The page's drawn words, measured at the style's size in this output (`font_px`) times their own scale.
#[derive(Default)]
pub(super) struct Words {
    pub widths: ArrayVec<f32, MAX_WORDS>,
    /// Ascent plus descent of the word's face at its size.
    pub heights: ArrayVec<f32, MAX_WORDS>,
    /// Three letters or fewer: never alone on a line.
    pub short: ArrayVec<bool, MAX_WORDS>,
    /// The person starts a line at it.
    pub forced: ArrayVec<bool, MAX_WORDS>,
    pub scales: ArrayVec<f32, MAX_WORDS>,
    /// Which of the layout's faces it is shaped in.
    pub faces: ArrayVec<u8, MAX_WORDS>,
    /// The words' text has spaces between them (none in a script that joins its words).
    pub spaced: bool,
    /// The drawn words in one string, a space where the text has one, and each word's bytes in it.
    pub shown: String,
    pub ranges: ArrayVec<[u32; 2], MAX_WORDS>,
}

/// `page`'s drawn words measured in `faces[0]` (a Cyrillic one in the style's stand-in, pushed to `faces` when used),
/// each word's CJK runs appended to `runs_out`.
#[allow(clippy::too_many_arguments)]
pub(super) fn measure_words(
    fs: &mut FontSystem,
    style: &CaptionStyleDef,
    language: &str,
    pages: &Pages,
    page: &Page,
    font_px: f32,
    faces: &mut ArrayVec<FaceChoice, 2>,
    runs_out: &mut Vec<WordRun>,
) -> Words {
    let keyword_scale = style.keyword.as_ref().map_or(1.0, |k| k.scale as f32);
    // The drawn words in one string, a space where the text has one: the CJK runs are read across the page.
    let mut shown = String::new();
    let mut ranges: ArrayVec<Range<usize>, MAX_WORDS> = ArrayVec::new();
    let mut words = Words::default();
    // The words drawn, as caption files walk them too (one walk: `drawn_words`): hidden ones left out, a hidden word's
    // "new line here" going to the next word drawn.
    drawn_words(pages, page, |d| {
        if ranges.is_full() {
            return;
        }
        if d.space_before {
            shown.push(' ');
            words.spaced = true;
        }
        let from = shown.len();
        shown.push_str(d.text);
        ranges.push(from..shown.len());
        let look = pages.look(d.word);
        let step = look.size.unwrap_or(0);
        let keyword = pages.keyword(d.index).map_or(1.0, |_| keyword_scale);
        // "Emphasis" in a style without keywords is ×1.1, drawn at its size, with room for it on the line.
        let emphasis = if style.keyword.is_none() && look.emphasis == Some(true) {
            EMPHASIS_SCALE
        } else {
            1.0
        };
        words
            .scales
            .push(1.15f32.powi(i32::from(step)) * keyword * emphasis);
        words.forced.push(d.forced);
    });
    let primary = faces[0].clone();
    let page_runs = shown
        .chars()
        .any(in_cjk_blocks)
        .then(|| runs(&shown, &primary.family, language, primary.cjk));
    let spacing = style.letter_spacing_em as f32;
    for (k, range) in ranges.iter().enumerate() {
        let text = &shown[range.clone()];
        let which = if cyrillic(text) {
            cyrillic_face(style, faces)
        } else {
            0
        };
        let word_runs = page_runs
            .as_ref()
            .map(|r| runs_within(r, range, &primary.family))
            .unwrap_or_default();
        let px = font_px * words.scales[k];
        let (w, h) = measure(fs, &faces[which as usize], text, px, spacing, &word_runs);
        runs_out.extend(word_runs.into_iter().map(|(r, f)| (k as u8, r, f)));
        words.widths.push(w);
        words.heights.push(h);
        words.short.push(is_short(text));
        words.faces.push(which);
        words.ranges.push([range.start as u32, range.end as u32]);
    }
    words.shown = shown;
    words
}

/// Most of the word's letters are Cyrillic.
fn cyrillic(text: &str) -> bool {
    let (cyrillic, letters) = text
        .chars()
        .filter(|c| c.is_alphabetic())
        .fold((0, 0), |(c, l), ch| {
            (c + usize::from(ch.script() == Script::Cyrillic), l + 1)
        });
    cyrillic * 2 > letters
}

/// The face a Cyrillic word takes: the style's `Cyrl` stand-in (Loud's Rubik Black Italic) when the page draws in the
/// style's own face, resolved once a page at the stand-in's weight, so it is an exact face too; else the page's face.
fn cyrillic_face(style: &CaptionStyleDef, faces: &mut ArrayVec<FaceChoice, 2>) -> u8 {
    if faces.len() > 1 {
        return 1;
    }
    let Some(stand_in) = style.fallbacks.iter().find(|f| f.script == "Cyrl") else {
        return 0;
    };
    let index = fonts::index();
    let font = &style.font;
    let own = family_for(
        &index,
        &FontRef::bundled(&font.family),
        font.weight,
        font.italic,
    );
    if faces[0].family != own.family {
        return 0;
    }
    let font = FontRef::bundled(&stand_in.family);
    faces.push(family_for(&index, &font, stand_in.weight, stand_in.italic));
    1
}

/// The page's runs over `word` (bytes of the shown text), rebased to the word's text; none when every byte asks for
/// `primary`.
fn runs_within(
    page: &[(Range<usize>, Arc<str>)],
    word: &Range<usize>,
    primary: &str,
) -> Vec<(Range<usize>, Arc<str>)> {
    let within: Vec<(Range<usize>, Arc<str>)> = page
        .iter()
        .filter(|(r, _)| r.start < word.end && r.end > word.start)
        .map(|(r, f)| {
            let from = r.start.max(word.start) - word.start;
            (from..r.end.min(word.end) - word.start, f.clone())
        })
        .collect();
    if within.iter().all(|(_, f)| &**f == primary) {
        Vec::new()
    } else {
        within
    }
}
