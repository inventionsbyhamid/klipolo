//! Making pages: one speech map and one walk over the shown words (`said`) feed `Builders`, which
//! makes the words once for every format (their shown text, width in columns, spacing, file, look, keyword candidate,
//! and a fix's carry) and hands each to one `PageBuilder` per format, which only pages them. Nothing is allocated per
//! word beyond the words' own growth: the shown text goes into a kept buffer, a look is an index into the project's
//! list, and the keyword lists are read without copying the word.

use std::sync::Arc;

use super::casing::shown_into;
use super::keywords::candidate;
use super::looks::{LookCursor, WordLook, PLAIN_LOOK};
use super::page_builder::{Cand, PageBuilder, Shown};
use super::pages::{without_final_stop, Captions, PageWord, Pages, NO_LOOK};
use super::said::{columns, ends_joining, starts_joining, walk, Said, Sink, BREAK_GAP_US};
use super::style::{CaptionStyleDef, KeywordWords, Punctuation, TextCase};
use super::styles::CaptionStyles;
use crate::project::{CaptionSettings, FormatKind, Project};
use crate::speech::{spoken, Speech};
use crate::time::Us;

/// The pages of every format the project lists (the preview's), or none when captions are off.
pub fn captions(p: &Project, speech: &Speech) -> Captions {
    captions_in(p, speech, &p.settings.formats)
}

/// The pages of `formats` (an export's files, or a dry run's previewed format), or none when captions are off. One
/// speech map and one walk over its words feed every format, and the formats share the words.
pub fn captions_in(p: &Project, speech: &Speech, formats: &[FormatKind]) -> Captions {
    captions_with(p, &p.captions, speech, formats)
}

/// `captions_in` with `settings` in place of the project's own (a caption file pages with Clean, whatever the project
/// burns in), without copying the project.
pub(super) fn captions_with(
    p: &Project,
    settings: &CaptionSettings,
    speech: &Speech,
    formats: &[FormatKind],
) -> Captions {
    if !settings.enabled || formats.is_empty() {
        return Captions::default();
    }
    let mut formats = formats.to_vec();
    formats.sort();
    formats.dedup();
    let (style, _) = CaptionStyles::builtin().for_settings(settings, &p.settings.formats);
    let spoken = spoken(p, speech);
    let mut sink = Builders::new(&style, p.word_looks.clone(), &formats, spoken.words.len());
    walk(&spoken, p, speech, &mut sink);
    Captions {
        style: style.id.clone(),
        pages: formats
            .into_iter()
            .zip(sink.finish().into_iter().map(Arc::new))
            .collect(),
    }
}

/// A dry run's pages: `format`'s made for `p`, the other formats' kept from `committed` (only the previewed
/// format draws, and keeping them costs a pointer each). With captions off there are none.
pub fn previewed(
    p: &Project,
    speech: &Speech,
    format: FormatKind,
    committed: &Captions,
) -> Captions {
    let fresh = captions_in(p, speech, &[format]);
    if fresh.pages.is_empty() {
        return fresh;
    }
    let mut pages = committed.pages.clone();
    pages.extend(fresh.pages);
    Captions {
        style: fresh.style,
        pages,
    }
}

/// The words of one build, made once, and every format's builder.
pub(super) struct Builders<'s> {
    words: Vec<PageWord>,
    text: String,
    /// The files the words come from, and the current span's (`PageWord::asset`).
    assets: Vec<String>,
    span_asset: Option<(u32, u32)>,
    /// The span of the last word added (`PageWord::cut_before`).
    last_span: Option<u32>,
    /// The project's looks: the words point into this one list.
    table: Arc<Vec<WordLook>>,
    cursor: LookCursor,
    case: TextCase,
    punct: Punctuation,
    keywords: Option<KeywordWords>,
    /// The word as the style shows it (kept from word to word).
    shown: String,
    /// The last word ends in a script written without spaces (`ends_joining`).
    prev_joins: bool,
    /// The fix the last word reads and where its piece ends on the timeline, and whether it was a piece of pauses only
    /// (`Sink::carry_on`).
    fix: Option<(u32, Us)>,
    unheard: bool,
    formats: Vec<PageBuilder<'s>>,
}

impl<'s> Builders<'s> {
    /// Builders for `formats` (each once) in `style`, reading `looks`; `words`: how many words the walk may bring.
    pub(super) fn new(
        style: &'s CaptionStyleDef,
        looks: Arc<Vec<WordLook>>,
        formats: &[FormatKind],
        words: usize,
    ) -> Self {
        Self {
            words: Vec::with_capacity(words),
            text: String::with_capacity(words * 7),
            assets: Vec::new(),
            span_asset: None,
            last_span: None,
            table: looks,
            cursor: LookCursor::new(),
            case: style.case,
            punct: style.punctuation,
            keywords: style.keyword.as_ref().map(|k| k.words),
            shown: String::with_capacity(64),
            prev_joins: false,
            fix: None,
            unheard: false,
            formats: formats
                .iter()
                .map(|&f| PageBuilder::new(style, f, words))
                .collect(),
        }
    }

    /// `w`'s file as `PageWord::asset`, found once per span.
    fn asset(&mut self, w: &Said) -> u32 {
        if let Some((_, i)) = self.span_asset.filter(|&(span, _)| span == w.span) {
            return i;
        }
        let i = match self.assets.iter().rposition(|a| a == w.asset) {
            Some(i) => i,
            None => {
                self.assets.push(w.asset.to_owned());
                self.assets.len() - 1
            }
        } as u32;
        self.span_asset = Some((w.span, i));
        i
    }

    /// Each format's pages, in the order of `formats`, over the words they share.
    pub(super) fn finish(self) -> Vec<Pages> {
        let words = Arc::new(self.words);
        let text = Arc::new(self.text);
        let assets = Arc::new(self.assets);
        self.formats
            .into_iter()
            .map(|b| {
                let (pages, keywords) = b.finish();
                Pages {
                    pages,
                    keywords,
                    words: words.clone(),
                    text: text.clone(),
                    assets: assets.clone(),
                    looks: self.table.clone(),
                }
            })
            .collect()
    }
}

impl Sink for Builders<'_> {
    fn add(&mut self, w: &Said) {
        self.shown.clear();
        shown_into(w.text, self.case, self.punct, &mut self.shown);
        if self.shown.is_empty() {
            // Punctuation only ("…" in an upper style): nothing to show, but a sentence or clause still ends.
            for b in &mut self.formats {
                b.punctuate(w);
            }
            return;
        }
        let asset = self.asset(w);
        // A word's look is the one covering its middle.
        let mid = w.source_start_us + (w.source_end_us - w.source_start_us) / 2;
        let index = self.cursor.at(&self.table, w.span, w.asset, mid);
        let look = index.map_or(&PLAIN_LOOK, |i| &self.table[i].look);
        let hidden = look.hidden == Some(true);
        let cols = columns(&self.shown);
        let joined = self.prev_joins && starts_joining(&self.shown);
        self.prev_joins = ends_joining(&self.shown);
        let s = Shown {
            cols,
            scaled: cols as f64 * 1.15f64.powi(i32::from(look.size.unwrap_or(0))),
            joined,
            hidden,
            look_break: look.line_break == Some(true),
            cand: Cand {
                best: self
                    .keywords
                    .filter(|_| !hidden)
                    .and_then(|k| candidate(w.text, w.sentence_start, k)),
                emphasis: look.emphasis,
                hidden,
                start_us: w.start_us,
                period: without_final_stop(&self.shown).is_some(),
            },
        };
        let word = self.words.len() as u32;
        for b in &mut self.formats {
            b.add(w, &s, word);
        }
        if !self.words.is_empty() && !joined {
            self.text.push(' ');
        }
        let from = self.text.len() as u32;
        self.text.push_str(&self.shown);
        let cut_before = self.last_span.is_some_and(|span| span != w.span);
        self.last_span = Some(w.span);
        self.words.push(PageWord {
            text: (from, self.text.len() as u32),
            start_us: w.start_us,
            end_us: w.end_us,
            asset,
            source_start_us: w.source_start_us,
            source_end_us: w.source_end_us,
            look: index.map_or(NO_LOOK, |i| i as u32),
            line_break: s.look_break,
            hidden,
            cut_before,
        });
        (self.fix, self.unheard) = (w.fix, w.unheard);
    }

    fn carry_on(&mut self, fix: u32, edges: (Us, Us), cut_us: Us, heard: Option<(Us, Us)>) {
        let Some((f, piece_end)) = self.fix else {
            return;
        };
        let Some(last) = self.words.last_mut() else {
            return;
        };
        if f != fix || edges.0 - piece_end >= BREAK_GAP_US {
            return;
        }
        // The last word goes on to the end of what the piece says; a word of pauses only is heard from here, never from
        // the pauses before (the carry decision).
        let mut heard_from = None;
        if let Some((start_us, end_us)) = heard {
            if self.unheard {
                last.start_us = start_us;
                heard_from = Some(start_us);
                self.unheard = false;
            }
            last.end_us = last.end_us.max(end_us);
        }
        let end_us = heard.map(|_| last.end_us);
        for b in &mut self.formats {
            b.carry_on(cut_us, heard_from, end_us);
        }
        self.fix = Some((fix, edges.1));
    }
}
