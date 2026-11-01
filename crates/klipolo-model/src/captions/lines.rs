//! Line breaking for a caption page: the fewest lines that fit the width, then the most balanced
//! (the narrowest widest line), never a word of three letters or fewer alone on a line, and for Clean a bottom-heavy
//! pyramid. Pages hold at most 32 words and 4 lines (+1 when nothing fits), so every split is tried: at most
//! C(31, 4) ≈ 31k sums for a 32-word page, and in practice pages have under 10 words. Allocates nothing.
//!
//! Pure, so the drawn captions (the engine's page layout) and the caption files balance their lines with the
//! same code.

use arrayvec::ArrayVec;

use super::pages::{Page, PageWord, Pages};
use super::style::{CaptionStyleDef, Punctuation, TextCase};

/// A word of a page as it is drawn: what the renderer lays out and what a caption file writes (one walk for both:
/// `drawn_words`).
pub struct Drawn<'a> {
    /// Its index in `Pages::words`.
    pub index: u32,
    pub word: &'a PageWord,
    /// What it shows there (`Pages::word_text`: without its final period where the page drops it).
    pub text: &'a str,
    /// A space before it on its line: never before the page's first word drawn, nor where its script joins words.
    pub space_before: bool,
    /// A line starts at it: its own "New line here", or a hidden word's just before it.
    pub forced: bool,
}

/// Calls `each` with every word of `page` that is drawn, in order: hidden words stay in the pages and out of this
/// (the word list can still show them). Allocates nothing.
pub fn drawn_words<'a>(pages: &'a Pages, page: &Page, mut each: impl FnMut(Drawn<'a>)) {
    let mut pending = false;
    let mut first = true;
    for index in page.words.0..page.words.1 {
        let Some(word) = pages.words.get(index as usize) else {
            break;
        };
        if word.hidden {
            pending |= word.line_break;
            continue;
        }
        let before = (word.text.0 as usize).wrapping_sub(1);
        let space_before = !first && pages.text.as_bytes().get(before) == Some(&b' ');
        first = false;
        each(Drawn {
            index,
            word,
            text: pages.word_text(page, index),
            space_before,
            forced: word.line_break || std::mem::take(&mut pending),
        });
    }
}

/// Three letters or digits or fewer: never alone on a line.
pub fn is_short(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphanumeric()).count() <= 3
}

/// Clean's bottom-heavy lines: a style that keeps punctuation and casing as said, with no boxes and no word limit.
pub fn pyramid(style: &CaptionStyleDef) -> bool {
    style.punctuation == Punctuation::Keep
        && style.case == TextCase::AsSaid
        && style.boxes.is_none()
        && style.pages.max_words.is_none()
}

/// The most lines a page can be broken into: a style's 4, and one more when nothing fits (Loud's rule).
pub const MAX_LINES: usize = 5;

pub struct LineInput<'a> {
    /// Each word's width, px.
    pub widths: &'a [f32],
    /// The width of the space between two words on a line, px.
    pub space: f32,
    pub max_width: f32,
    pub max_lines: usize,
    /// A forced break before word i (the person's "New line here").
    pub forced: &'a [bool],
    /// Word i has three letters or fewer.
    pub short: &'a [bool],
    pub pyramid: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Breaks {
    /// The first word of each line.
    pub starts: ArrayVec<u8, MAX_LINES>,
    /// The widest line, px.
    pub widest: f32,
}

/// Whether a line `width` wide fits `max_width`: a relative tolerance (a thousandth), so a borderline line breaks the
/// same at every resolution (the preview's 720 px and the export's 1,080).
pub fn fits(width: f32, max_width: f32) -> bool {
    width <= max_width * (1.0 + 1e-3)
}

fn line_width(w: &[f32], space: f32, from: usize, to: usize) -> f32 {
    w[from..to].iter().sum::<f32>() + space * (to - from).saturating_sub(1) as f32
}

/// Whether the lines starting at `starts` fit, and their cost: the widest line, plus a penalty for a short word alone
/// on a line and, for a pyramid, for each line narrower than the one above it.
fn score(input: &LineInput, starts: &[u8]) -> (bool, f32) {
    let n = input.widths.len();
    let mut widest = 0f32;
    let mut penalty = 0f32;
    let mut prev_w = None;
    for (k, &s) in starts.iter().enumerate() {
        let from = s as usize;
        let to = starts.get(k + 1).map_or(n, |&e| e as usize);
        let w = line_width(input.widths, input.space, from, to);
        widest = widest.max(w);
        if to - from == 1 && input.short[from] && n > 1 {
            penalty += input.max_width * 0.5;
        }
        if input.pyramid {
            if let Some(p) = prev_w {
                if p > w {
                    penalty += (p - w) * 0.15;
                }
            }
        }
        prev_w = Some(w);
    }
    (fits(widest, input.max_width), widest + penalty)
}

/// Calls `f` with the line starts of every split that adds `extra` free break positions (positions not already
/// forced) to the forced ones.
fn each_split(input: &LineInput, extra: usize, f: &mut impl FnMut(&[u8])) {
    let n = input.widths.len();
    let mut starts: ArrayVec<u8, MAX_LINES> = ArrayVec::new();
    fn go(
        input: &LineInput,
        n: usize,
        from: usize,
        left: usize,
        starts: &mut ArrayVec<u8, MAX_LINES>,
        f: &mut impl FnMut(&[u8]),
    ) {
        if left == 0 {
            // The forced breaks and the chosen ones, in order.
            let mut all: ArrayVec<u8, MAX_LINES> = ArrayVec::new();
            let mut chosen = starts.iter().copied().peekable();
            for (i, &forced) in input.forced.iter().enumerate().take(n) {
                let is_chosen = chosen.peek() == Some(&(i as u8));
                if (i == 0 || forced || is_chosen) && all.try_push(i as u8).is_err() {
                    return;
                }
                if is_chosen {
                    chosen.next();
                }
            }
            f(&all);
            return;
        }
        for (i, &forced) in input.forced.iter().enumerate().take(n).skip(from) {
            if forced {
                continue;
            }
            if starts.try_push(i as u8).is_err() {
                return;
            }
            go(input, n, i + 1, left - 1, starts, f);
            starts.pop();
        }
    }
    go(input, n, 1, extra, &mut starts, f);
}

/// The lines of a page whose words are `input.widths` wide: the fewest that fit `max_width`, the most balanced of
/// those; when none fits at `max_lines`, the narrowest arrangement at the most lines (the caller autofits it).
pub fn break_lines(input: &LineInput) -> Breaks {
    let n = input.widths.len().min(32);
    if n == 0 {
        return Breaks {
            starts: ArrayVec::new(),
            widest: 0.0,
        };
    }
    let forced = input
        .forced
        .iter()
        .skip(1)
        .take(n - 1)
        .filter(|&&f| f)
        .count();
    // Never more lines than words: a page of two words in a three-line style still breaks them when they don't fit.
    let most = input.max_lines.max(forced + 1).min(MAX_LINES).min(n);
    let mut fallback: Option<(f32, ArrayVec<u8, MAX_LINES>)> = None;
    for lines in (forced + 1)..=most {
        let mut best: Option<(f32, ArrayVec<u8, MAX_LINES>)> = None;
        each_split(input, lines - 1 - forced, &mut |starts| {
            let (fits, cost) = score(input, starts);
            let keep = |b: &Option<(f32, ArrayVec<u8, MAX_LINES>)>| {
                b.as_ref().is_none_or(|(c, _)| cost < *c)
            };
            if fits && keep(&best) {
                best = Some((cost, starts.iter().copied().collect()));
            }
            if lines == most && keep(&fallback) {
                fallback = Some((cost, starts.iter().copied().collect()));
            }
        });
        if let Some((_, starts)) = best {
            let widest = widest_of(input, &starts);
            return Breaks { starts, widest };
        }
    }
    let starts = fallback
        .map(|(_, s)| s)
        .unwrap_or_else(|| std::iter::once(0).collect());
    let widest = widest_of(input, &starts);
    Breaks { starts, widest }
}

fn widest_of(input: &LineInput, starts: &[u8]) -> f32 {
    let n = input.widths.len();
    (0..starts.len())
        .map(|k| {
            line_width(
                input.widths,
                input.space,
                starts[k] as usize,
                starts.get(k + 1).map_or(n, |&e| e as usize),
            )
        })
        .fold(0.0, f32::max)
}
