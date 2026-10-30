//! A shaped scene text cut into the pieces it draws and the units it moves by: grapheme clusters
//! (`unicode-segmentation`, so a ZWJ emoji, a flag or a letter with its marks is one), words (runs between
//! whitespace) or lines (the layout's). A piece is one grain of one look on one line, in the node's box; it moves with
//! its split unit and appears with its typewriter step. Never per frame.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use super::text::NO_UNIT;
use super::text_shape::Lines;

/// One thing a text draws: a grain (a grapheme, a word or a line) of one look on one line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    /// Its layout box in the node's box, pixels: left, top, right, bottom (its line's top and bottom).
    pub rect: [f32; 4],
    pub baseline: f32,
    /// The split unit it moves with (`NO_UNIT` without), and its typewriter step (`NO_UNIT` without).
    pub unit: u16,
    pub reveal: u16,
    /// Its look: 0 the node's own, `k + 1` span `k`'s.
    pub look: u8,
    /// Its line, and how many of the line's glyphs come before it (an animated letter spacing moves it that many).
    pub line: u16,
    pub col: u16,
    /// Its glyphs in `TextUnits::glyphs`, its bytes in the laid-out text, and the characters before it there.
    pub glyphs: [u32; 2],
    pub bytes: [u32; 2],
    pub chars_at: u32,
}

/// How finely a text is cut: by grapheme, by word or by line (the coarsest).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Grain {
    Chars,
    Words,
    Lines,
}

/// A text's graphemes: where each starts, the characters before it, its index among the ones that aren't whitespace
/// (`NO_UNIT` for whitespace) and its word's index.
pub(super) struct Graphemes {
    starts: Vec<usize>,
    chars_before: Vec<u32>,
    shown: Vec<u16>,
    words: Vec<u16>,
    /// Graphemes that aren't whitespace, and words.
    pub count: usize,
    pub word_count: usize,
}

impl Graphemes {
    pub(super) fn of(text: &str) -> Self {
        let mut g = Self {
            starts: Vec::new(),
            chars_before: Vec::new(),
            shown: Vec::new(),
            words: Vec::new(),
            count: 0,
            word_count: 0,
        };
        let (mut chars, mut in_word) = (0u32, false);
        for (at, cluster) in text.grapheme_indices(true) {
            let blank = cluster.chars().all(char::is_whitespace);
            g.starts.push(at);
            g.chars_before.push(chars);
            chars += cluster.chars().count() as u32;
            if blank {
                g.shown.push(NO_UNIT);
                g.words.push(NO_UNIT);
                in_word = false;
                continue;
            }
            if !in_word {
                g.word_count += 1;
                in_word = true;
            }
            g.shown.push(g.count.min(usize::from(NO_UNIT) - 1) as u16);
            g.words
                .push((g.word_count - 1).min(usize::from(NO_UNIT) - 1) as u16);
            g.count += 1;
        }
        g
    }

    /// The grapheme holding byte `at`.
    pub(super) fn at(&self, at: usize) -> usize {
        self.starts.partition_point(|&s| s <= at).saturating_sub(1)
    }

    /// Grapheme `i`'s index among those that aren't whitespace, or `NO_UNIT`.
    pub(super) fn shown(&self, i: usize) -> u16 {
        self.shown.get(i).copied().unwrap_or(NO_UNIT)
    }

    fn unit(&self, grain: Option<Grain>, i: usize, line: u16) -> u16 {
        match grain {
            None => NO_UNIT,
            Some(Grain::Chars) => self.shown(i),
            Some(Grain::Words) => self.words.get(i).copied().unwrap_or(NO_UNIT),
            Some(Grain::Lines) => line,
        }
    }
}

/// What cutting a laid-out text gives: its pieces in reading order, its split units (how many, and each one's box,
/// what it moves about), its typewriter steps, and each line's box (its glyphs' edges, its top and bottom).
pub(super) struct Cut {
    pub pieces: Vec<Piece>,
    pub units: usize,
    pub unit_rects: Vec<[f32; 4]>,
    pub reveal_units: usize,
    pub lines: Vec<[f32; 4]>,
    /// Each line's glyphs.
    pub line_glyphs: Vec<u16>,
}

/// `lines` (shaped from `text`, already placed in the box) cut by `grain` and by look (`spans`), each piece with its
/// unit under `split` and its step under `reveal`. Whitespace makes no piece, except inside a line piece.
pub(super) fn cut(
    lines: &Lines,
    text: &str,
    spans: &[(Range<usize>, u8)],
    grain: Grain,
    split: Option<Grain>,
    reveal: Option<Grain>,
) -> Cut {
    let g = Graphemes::of(text);
    let look_at = |at: usize| {
        spans
            .iter()
            .find(|(r, _)| r.start <= at && at < r.end)
            .map_or(0, |(_, l)| *l)
    };
    let mut pieces: Vec<Piece> = Vec::new();
    let mut shown_line = 0u16;
    let mut boxes = Vec::with_capacity(lines.lines.len());
    let mut line_glyphs = Vec::with_capacity(lines.lines.len());
    for line in &lines.lines {
        let [from, to] = line.glyphs;
        let mut current: Option<(usize, u8)> = None;
        let mut any = false;
        for (k, glyph) in lines.glyphs[from..to].iter().enumerate() {
            let i = g.at(glyph.start);
            let blank = g.shown(i) == NO_UNIT;
            if blank && grain != Grain::Lines {
                current = None;
                continue;
            }
            any |= !blank;
            let look = look_at(glyph.start);
            let key = match grain {
                Grain::Chars => i,
                Grain::Words => usize::from(g.unit(Some(Grain::Words), i, 0)),
                Grain::Lines => 0,
            };
            let at = from + k;
            if current != Some((key, look)) || pieces.is_empty() {
                current = Some((key, look));
                pieces.push(Piece {
                    rect: [glyph.x, line.top, glyph.x + glyph.w, line.top + line.height],
                    baseline: line.baseline,
                    unit: g.unit(split, i, shown_line),
                    reveal: g.unit(reveal, i, shown_line),
                    look,
                    line: (boxes.len()).min(usize::from(u16::MAX)) as u16,
                    col: k.min(usize::from(u16::MAX)) as u16,
                    glyphs: [at as u32, at as u32 + 1],
                    bytes: [glyph.start as u32, glyph.end as u32],
                    chars_at: g.chars_before.get(i).copied().unwrap_or(0),
                });
                continue;
            }
            let p = pieces.last_mut().expect("a piece is open");
            p.rect[0] = p.rect[0].min(glyph.x);
            p.rect[2] = p.rect[2].max(glyph.x + glyph.w);
            p.glyphs[1] = at as u32 + 1;
            p.bytes = [
                p.bytes[0].min(glyph.start as u32),
                p.bytes[1].max(glyph.end as u32),
            ];
            if (glyph.start as u32) == p.bytes[0] {
                p.chars_at = g.chars_before.get(i).copied().unwrap_or(0);
            }
        }
        shown_line += u16::from(any);
        boxes.push([line.ink[0], line.top, line.ink[1], line.top + line.height]);
        line_glyphs.push((to - from).min(usize::from(u16::MAX)) as u16);
    }
    // A line piece of only whitespace draws nothing.
    pieces.retain(|p| {
        text.get(p.bytes[0] as usize..p.bytes[1] as usize)
            .is_some_and(|t| !t.chars().all(char::is_whitespace))
    });
    pieces.sort_by_key(|p| (p.bytes[0], p.line));
    let count = |grain: Option<Grain>| match grain {
        None => 0,
        Some(Grain::Chars) => g.count,
        Some(Grain::Words) => g.word_count,
        Some(Grain::Lines) => usize::from(shown_line),
    };
    let units = count(split);
    let mut unit_rects = vec![[f32::MAX, f32::MAX, f32::MIN, f32::MIN]; units];
    for p in &pieces {
        if let Some(r) = unit_rects.get_mut(usize::from(p.unit)) {
            *r = [
                r[0].min(p.rect[0]),
                r[1].min(p.rect[1]),
                r[2].max(p.rect[2]),
                r[3].max(p.rect[3]),
            ];
        }
    }
    for r in &mut unit_rects {
        if r[0] > r[2] {
            *r = [0.0; 4];
        }
    }
    Cut {
        pieces,
        units,
        unit_rects,
        reveal_units: count(reveal),
        lines: boxes,
        line_glyphs,
    }
}
