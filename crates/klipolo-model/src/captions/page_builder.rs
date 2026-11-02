//! One format's page builder: the rules that close a page, in order, and how a word joins one. It
//! pages the words `Builders` makes once for every format, keeping only its pages and their keywords. Closing and
//! finishing the pages are in `page_close`.

use super::keywords::Keyword;
use super::pages::{Page, MAX_PAGE_WORDS};
use super::said::Said;
use super::style::{CaptionStyleDef, PageRules, Punctuation};
use crate::project::FormatKind;
use crate::time::Us;

pub(super) fn ms(ms: u32) -> Us {
    Us::from(ms) * 1_000
}

/// The page a builder is filling. Its times are its visible words'.
pub(super) struct OpenPage {
    pub(super) start_us: Us,
    pub(super) end_us: Us,
    pub(super) first_word: u32,
    /// Where its shot ends on the timeline (`Said::cut_us`): its hold stops there, and a word from another shot starts
    /// a new page. A split left in place is no cut (its clips share one shot end), so it never ends a page.
    pub(super) cut_us: Us,
    /// Words that aren't hidden.
    pub(super) visible: u32,
    /// Lines so far, and the last one's estimated width (px) and columns.
    pub(super) lines: u32,
    pub(super) line_px: f64,
    pub(super) line_cols: usize,
    /// Every visible word's columns and the spaces between them (reading speed).
    pub(super) cols: usize,
    pub(super) ended_sentence: bool,
    pub(super) ended_clause: bool,
}

/// One format's pages while they are made.
pub(super) struct PageBuilder<'s> {
    pub(super) rules: &'s PageRules,
    /// One column's estimated width at the format's size: `charEm` × size, px at a 1080 short side.
    pub(super) col_px: f64,
    /// The line width in px at the format's size: `maxLineWidth` × the frame's width.
    pub(super) line_px: f64,
    pub(super) max_chars: Option<usize>,
    pub(super) keywords: bool,
    pub(super) drop_final_period: bool,
    pub(super) pages: Vec<Page>,
    /// Per word: its keyword on these pages (`Pages::keywords`).
    pub(super) keyword_of: Vec<Option<Keyword>>,
    pub(super) open: Option<OpenPage>,
    pub(super) last_keyword_us: Option<Us>,
    /// Per page made: the earliest it may end (`minPageMs`, `readingCps`; None for a page of hidden words only), and
    /// the cut its shot ends at.
    pub(super) holds: Vec<(Option<Us>, Us)>,
    /// The open page's words as the keyword picker and the period rule see them (kept from page to page).
    pub(super) cands: Vec<Cand>,
}

/// A word as every format's builder takes it, worked out once (`Builders`).
pub(super) struct Shown {
    /// Its width in columns, and that × its look's size step (×1.15 each): a format's px are this × its column width.
    pub(super) cols: usize,
    pub(super) scaled: f64,
    /// It follows the word before it without a space (Chinese and Japanese).
    pub(super) joined: bool,
    pub(super) hidden: bool,
    /// Its look starts a line.
    pub(super) look_break: bool,
    pub(super) cand: Cand,
}

/// One word of the open page as the keyword picker and the period rule see it.
#[derive(Clone, Copy)]
pub(super) struct Cand {
    /// Its priority and class as a candidate (`keywords::candidate`).
    pub(super) best: Option<(u8, Keyword)>,
    /// The person's emphasis for it (`Look::emphasis`).
    pub(super) emphasis: Option<bool>,
    pub(super) hidden: bool,
    pub(super) start_us: Us,
    /// It shows more than a final period (or danda, or "。"), and ends with one (`dropFinalPeriod`).
    pub(super) period: bool,
}

impl<'s> PageBuilder<'s> {
    pub(super) fn new(style: &'s CaptionStyleDef, f: FormatKind, words: usize) -> Self {
        let (w, h) = f.size();
        let short = f64::from(w.min(h));
        let size_px = style.size.size(f) * short / 1080.0;
        let r = &style.pages;
        Self {
            rules: r,
            col_px: style.char_em * size_px,
            line_px: r.max_line_width.get(f) * f64::from(w),
            max_chars: r.max_chars.as_ref().map(|c| c.get(f) as usize),
            keywords: style.keyword.is_some(),
            drop_final_period: style.punctuation == Punctuation::DropFinalPeriod,
            pages: Vec::with_capacity(words / 2),
            keyword_of: Vec::with_capacity(words),
            open: None,
            last_keyword_us: None,
            holds: Vec::with_capacity(words / 2),
            cands: Vec::with_capacity(32),
        }
    }

    /// Whether the open page holds `MAX_PAGE_WORDS` words already.
    fn full(&self, o: &OpenPage) -> bool {
        self.keyword_of.len() as u32 - o.first_word >= MAX_PAGE_WORDS
    }

    /// Whether a visible word can't join the open page (the rules in their order); `fits`: whether it fits on the
    /// page's last line.
    fn must_break(&self, o: &OpenPage, w: &Said, look_break: bool, fits: bool) -> bool {
        let r = self.rules;
        let n = o.visible;
        o.cut_us != w.cut_us
            || self.full(o)
            || o.ended_sentence
            || (o.ended_clause && n >= 2)
            || (n > 0
                && (w.start_us - o.end_us >= ms(r.break_silence_ms)
                    || r.max_words.is_some_and(|m| n >= m)
                    || w.end_us - o.start_us > ms(r.max_page_ms)
                    || ((look_break || !fits) && o.lines >= r.max_lines)))
    }

    /// Whether a hidden word can't join the open page: it takes no width, words or time, but a cut, a sentence or
    /// clause end and a silence after the page's words still close it (so it starts the page it is said in).
    fn must_break_hidden(&self, o: &OpenPage, w: &Said) -> bool {
        o.cut_us != w.cut_us
            || self.full(o)
            || o.ended_sentence
            || (o.ended_clause && o.visible >= 2)
            || (o.visible > 0 && w.start_us - o.end_us >= ms(self.rules.break_silence_ms))
    }

    /// The space before a visible word on the open page's last line (px, columns), and whether it fits there.
    fn place(&self, o: &OpenPage, s: &Shown) -> ((f64, usize), bool) {
        let space = if s.joined || o.visible == 0 {
            (0.0, 0)
        } else {
            (self.col_px, 1)
        };
        let fits = o.line_px + space.0 + s.scaled * self.col_px <= self.line_px
            && self
                .max_chars
                .is_none_or(|m| o.line_cols + space.1 + s.cols <= m);
        (space, fits)
    }

    /// Pages word `word` (`Builders`' index), said as `w` and shown as `s`.
    pub(super) fn add(&mut self, w: &Said, s: &Shown, word: u32) {
        let mut placed = None;
        if let Some(o) = &self.open {
            let close = if s.hidden {
                self.must_break_hidden(o, w)
            } else {
                let p = self.place(o, s);
                placed = Some(p);
                self.must_break(o, w, s.look_break, p.1)
            };
            if close {
                self.close();
                placed = None;
            }
        }
        let o = self.open.get_or_insert(OpenPage {
            start_us: w.start_us,
            end_us: w.end_us,
            first_word: word,
            cut_us: w.cut_us,
            visible: 0,
            lines: 0,
            line_px: 0.0,
            line_cols: 0,
            cols: 0,
            ended_sentence: false,
            ended_clause: false,
        });
        if !s.hidden {
            let width = s.scaled * self.col_px;
            let space = placed.map_or((0.0, 0), |p| p.0);
            if o.visible == 0 {
                (o.start_us, o.end_us, o.lines, o.line_px, o.line_cols) =
                    (w.start_us, w.end_us, 1, width, s.cols);
            } else {
                if s.look_break || !placed.is_some_and(|p| p.1) {
                    (o.lines, o.line_px, o.line_cols) = (o.lines + 1, width, s.cols);
                } else {
                    o.line_px += space.0 + width;
                    o.line_cols += space.1 + s.cols;
                }
                o.end_us = o.end_us.max(w.end_us);
            }
            o.visible += 1;
            o.cols += space.1 + s.cols;
        }
        o.ended_sentence = w.ends_sentence;
        o.ended_clause = w.ends_clause;
        o.cut_us = w.cut_us;
        self.keyword_of.push(None);
        self.cands.push(s.cand);
    }

    /// A word with nothing to show still ends a sentence or clause on the open page.
    pub(super) fn punctuate(&mut self, w: &Said) {
        if let Some(o) = &mut self.open {
            o.ended_sentence |= w.ends_sentence;
            o.ended_clause |= w.ends_clause;
        }
    }

    /// A fix's later piece carried the open page's last word on (`Sink::carry_on`) into a shot that ends at `cut_us`:
    /// it is heard from `start_us` now (when it was a piece of pauses only), and until `end_us` (when the piece is heard
    /// at all).
    pub(super) fn carry_on(&mut self, cut_us: Us, start_us: Option<Us>, end_us: Option<Us>) {
        let (Some(o), Some(last)) = (self.open.as_mut(), self.cands.last_mut()) else {
            return;
        };
        if let Some(start_us) = start_us {
            last.start_us = start_us;
            if !last.hidden && o.visible == 1 {
                o.start_us = start_us;
            }
        }
        if let Some(end_us) = end_us.filter(|_| !last.hidden) {
            o.end_us = o.end_us.max(end_us);
        }
        o.cut_us = cut_us;
    }
}
