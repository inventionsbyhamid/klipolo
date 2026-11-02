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
    /// Each word's bidi level on the page (`word_levels`): odd reads right to left. Empty: all 0.
    pub levels: ArrayVec<u8, MAX_WORDS>,
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
        let spacing = if joins_letters(text) { 0.0 } else { spacing };
        let (w, h) = measure(fs, &faces[which as usize], text, px, spacing, &word_runs);
        runs_out.extend(word_runs.into_iter().map(|(r, f)| (k as u8, r, f)));
        words.widths.push(w);
        words.heights.push(h);
        words.short.push(is_short(text));
        words.faces.push(which);
        words.ranges.push([range.start as u32, range.end as u32]);
    }
    words.levels = word_levels(&shown, &ranges, language);
    words.shown = shown;
    words
}

/// Which way a word reads: by its first letter, else a number, else neither (punctuation, symbols).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reads {
    Ltr,
    Rtl,
    Number,
    Neutral,
}

fn reads(word: &str) -> Reads {
    let mut number = false;
    for c in word.chars() {
        if c.is_alphabetic() {
            return if right_to_left(c.script()) {
                Reads::Rtl
            } else {
                Reads::Ltr
            };
        }
        number |= c.is_numeric();
    }
    if number {
        Reads::Number
    } else {
        Reads::Neutral
    }
}

/// The scripts written right to left.
fn right_to_left(script: Script) -> bool {
    matches!(
        script,
        Script::Arabic
            | Script::Hebrew
            | Script::Syriac
            | Script::Thaana
            | Script::Nko
            | Script::Adlam
            | Script::Samaritan
            | Script::Mandaic
    )
}

/// Each word's bidi level, a word taken as one unit. The page reads right to left when the captions' language is
/// written that way (Arabic, Hebrew, Persian, Urdu ...), when more of its words read right to left than left to right,
/// or, on a tie, when its first word with a letter does (Unicode's paragraph rule): so "YouTube على قناتي" reads from
/// the right. On a right-to-left page (1) a word in Latin letters or a number is 2; on a left-to-right one (0) a word
/// in Arabic or Hebrew letters is 1, and so is a number or a mark between two of them, which stays in their run. All 0
/// on a page with no right-to-left letter, so nothing is reordered there.
fn word_levels(shown: &str, ranges: &[Range<usize>], language: &str) -> ArrayVec<u8, MAX_WORDS> {
    let read: ArrayVec<Reads, MAX_WORDS> = ranges
        .iter()
        .map(|r| reads(shown.get(r.clone()).unwrap_or_default()))
        .collect();
    if !read.contains(&Reads::Rtl) {
        return ArrayVec::new();
    }
    let count = |want: Reads| read.iter().filter(|&&r| r == want).count();
    let (rtl, ltr) = (count(Reads::Rtl), count(Reads::Ltr));
    let first_rtl = read
        .iter()
        .find(|r| matches!(r, Reads::Ltr | Reads::Rtl))
        .is_some_and(|r| *r == Reads::Rtl);
    let rtl_page = right_to_left_language(language) || rtl > ltr || (rtl == ltr && first_rtl);
    // Whether the nearest word with a letter before word `i`, and after it, reads right to left.
    let lettered = |r: &&Reads| matches!(r, Reads::Ltr | Reads::Rtl);
    let rtl_before = |i: usize| read[..i].iter().rev().find(lettered) == Some(&Reads::Rtl);
    let rtl_after = |i: usize| read[i + 1..].iter().find(lettered) == Some(&Reads::Rtl);
    (0..read.len())
        .map(|i| match (rtl_page, read[i]) {
            (true, Reads::Ltr | Reads::Number) => 2,
            (true, _) => 1,
            (false, Reads::Rtl) => 1,
            (false, Reads::Number | Reads::Neutral) if rtl_before(i) && rtl_after(i) => 1,
            (false, _) => 0,
        })
        .collect()
}

/// Languages written right to left, by their primary subtag.
fn right_to_left_language(language: &str) -> bool {
    let primary = language.split(['-', '_']).next().unwrap_or_default();
    matches!(
        primary.to_ascii_lowercase().as_str(),
        "ar" | "he" | "iw" | "fa" | "ur" | "yi" | "ps" | "sd" | "ug" | "dv" | "ckb"
    )
}

/// Whether `text` holds letters of a script that joins them (Arabic's cursive, the Indic scripts' headline and
/// conjuncts): letter spacing would pull them apart, so such a word is drawn and measured with none.
pub(super) fn joins_letters(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c.script(),
            Script::Arabic
                | Script::Syriac
                | Script::Nko
                | Script::Mongolian
                | Script::Devanagari
                | Script::Bengali
                | Script::Gurmukhi
                | Script::Gujarati
                | Script::Oriya
                | Script::Tamil
                | Script::Telugu
                | Script::Kannada
                | Script::Malayalam
                | Script::Sinhala
        )
    })
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

#[cfg(test)]
mod tests {
    use super::{joins_letters, word_levels};

    /// Each word's level for `text` split on spaces, in captions of `language`.
    fn levels(text: &str, language: &str) -> Vec<u8> {
        let mut at = 0;
        let ranges: Vec<_> = text
            .split(' ')
            .map(|w| {
                let r = at..at + w.len();
                at = r.end + 1;
                r
            })
            .collect();
        word_levels(text, &ranges, language).to_vec()
    }

    /// A page of more Arabic words than English ones reads from the right though an English word starts it, and so
    /// does any page in Arabic captions.
    #[test]
    fn more_arabic_words_or_arabic_captions_make_a_right_to_left_page() {
        assert_eq!(levels("YouTube على قناتي", "en"), [2, 1, 1]);
        assert_eq!(levels("Hello world مرحبا", "ar"), [2, 2, 1]);
        assert_eq!(levels("Hello world مرحبا", "en"), [0, 0, 1]);
        assert!(levels("Hello world", "ar").is_empty());
    }

    /// On a left-to-right page a number or a mark between two Arabic words stays in their run.
    #[test]
    fn a_number_between_arabic_words_stays_in_their_run() {
        assert_eq!(levels("Hello مرحبا 5 بكم world", "en"), [0, 1, 1, 1, 0]);
        assert_eq!(levels("Hello مرحبا 5 world", "en"), [0, 1, 0, 0]);
    }

    #[test]
    fn arabic_and_indic_words_join_their_letters() {
        assert!(joins_letters("مرحبا") && joins_letters("नमस्ते") && joins_letters("வணக்கம்"));
        assert!(!joins_letters("Hello") && !joins_letters("שלום") && !joins_letters("สวัสดี"));
    }
}
