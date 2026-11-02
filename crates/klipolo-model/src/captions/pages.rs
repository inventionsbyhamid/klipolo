//! Caption pages: the words shown together, made from the style's rules, the person's
//! looks and fixes. Built once per revision and format (the engine's feed; a dry run only for the previewed format),
//! frozen in an export's job (`captions.json`), and found per frame by binary search (`page_at`). Membership and timing
//! are decided here from `charEm` width estimates; the engine shapes the exact lines and autofits.
//!
//! Cost: one speech map and one walk over its words for every format (`paging`), O(words × formats) time. The words,
//! their text, files and looks are made once and shared by every format's pages; a format adds only
//! its pages and one keyword byte per word.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use super::keywords::Keyword;
use super::looks::{Look, WordLook, PLAIN_LOOK};
pub use super::paging::{captions, captions_in, previewed};
use crate::contrib::ContribId;
use crate::ids::Id;
use crate::project::FormatKind;
use crate::time::Us;

/// A page holds at most this many words, hidden ones included: the renderer lays a page out in fixed-capacity arrays
/// Only a run of very short words in a style without `maxWords` comes near it.
pub const MAX_PAGE_WORDS: u32 = 32;

/// Every format's pages, and the style they were made with (Clean when the project's isn't installed). Empty when
/// captions are off. Each format's pages are shared, so a dry run that makes its previewed format's again keeps the
/// others'.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Captions {
    pub style: ContribId,
    pub pages: BTreeMap<FormatKind, Arc<Pages>>,
}

impl Default for Captions {
    fn default() -> Self {
        Self {
            style: ContribId(String::new()),
            pages: BTreeMap::new(),
        }
    }
}

/// One format's pages, over the words of the build that made them. The words, their text, files and looks are shared
/// (`Arc`) by every format built together; in `captions.json` each format writes them out in full.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pages {
    /// On the timeline, in order, never overlapping. A page of hidden words only takes no time.
    pub pages: Vec<Page>,
    /// Per word (in `words`' order): its keyword on this format's pages (`keyword`).
    pub keywords: Vec<Option<Keyword>>,
    pub words: Arc<Vec<PageWord>>,
    /// Every word's shown text, a space before each but the first unless their script joins them (a page's first word
    /// draws without it).
    pub text: Arc<String>,
    /// The files the words come from (a handful).
    pub assets: Arc<Vec<Id>>,
    /// The person's looks the words point into (`PageWord::look`): the project's own list, shared, never copied.
    pub looks: Arc<Vec<WordLook>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    /// From its first visible word's start until it ends (held, lingered, never past the next page's start nor, unless
    /// the next page follows close behind it, its cut).
    pub start_us: Us,
    pub end_us: Us,
    /// `Pages::words[words.0..words.1]`.
    pub words: (u32, u32),
    /// Its place in `Pages::pages` (random effects are seeded by it, so preview and export match).
    pub index: u32,
    /// Some word on it is a keyword.
    pub keyword: bool,
    /// Its last visible word shows without its final period (`dropFinalPeriod`: `Pages::word_text`).
    #[serde(default, skip_serializing_if = "not")]
    pub period_dropped: bool,
}

/// `PageWord::look` of a word without a look.
pub const NO_LOOK: u32 = u32::MAX;

/// A sentence's full stops: the period, Hindi's danda, the Chinese and Japanese full stop and Urdu's.
const FULL_STOPS: [char; 4] = ['.', '।', '。', '۔'];

/// `text` without its final full stop (`FULL_STOPS`), when it ends with one and shows more than it.
pub(crate) fn without_final_stop(text: &str) -> Option<&str> {
    text.strip_suffix(FULL_STOPS)
        .filter(|rest| !rest.is_empty())
}

fn no_look() -> u32 {
    NO_LOOK
}

fn is_no_look(i: &u32) -> bool {
    *i == NO_LOOK
}

fn not(b: &bool) -> bool {
    !*b
}

/// One word of the pages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageWord {
    /// Its bytes in `Pages::text`: what it shows, cased and with its style's punctuation.
    pub text: (u32, u32),
    /// When it is heard on the timeline.
    pub start_us: Us,
    pub end_us: Us,
    /// `Pages::assets[asset]`, and where in that file's own time (the word list and caption files work from these).
    pub asset: u32,
    pub source_start_us: Us,
    pub source_end_us: Us,
    /// The person's look for it, whatever the style: `Pages::looks[look]`, or `NO_LOOK` (`Pages::look`).
    #[serde(default = "no_look", skip_serializing_if = "is_no_look")]
    pub look: u32,
    /// Its look starts a line here. It does, unless the word starts its page; a line the width makes is never marked
    /// (the engine breaks those itself).
    #[serde(default, skip_serializing_if = "not")]
    pub line_break: bool,
    /// Its look hides it: it stays, so the word list can show and unhide it, but never draws and takes no part in
    /// the page's width, words, time or keyword.
    #[serde(default, skip_serializing_if = "not")]
    pub hidden: bool,
    /// A cut lies between it and the page word before it: they come from different spans (another clip, or another
    /// file a clip plays), so the file time between them may play elsewhere, and one fix (one range of one file) must
    /// not cover both ("Fix the words"). Never set on the first word.
    #[serde(default, skip_serializing_if = "not")]
    pub cut_before: bool,
}

impl Pages {
    /// `w`'s look: the person's, or a plain one (every part unset).
    pub fn look(&self, w: &PageWord) -> &Look {
        self.looks
            .get(w.look as usize)
            .map_or(&PLAIN_LOOK, |l| &l.look)
    }

    /// Word `i`'s keyword on this format's pages.
    pub fn keyword(&self, i: u32) -> Option<Keyword> {
        self.keywords.get(i as usize).copied().flatten()
    }

    /// What `w` shows (`word_text` for a page's last word in a `dropFinalPeriod` style).
    pub fn text_of(&self, w: &PageWord) -> &str {
        self.text
            .get(w.text.0 as usize..w.text.1 as usize)
            .unwrap_or_default()
    }

    /// The last word of `page` that isn't hidden.
    fn last_visible(&self, page: &Page) -> Option<u32> {
        (page.words.0..page.words.1)
            .rev()
            .find(|&i| self.words.get(i as usize).is_some_and(|w| !w.hidden))
    }

    /// What word `i` of `page` shows there: its text, without its final period (or danda, or "。") when it is the page's
    /// last visible word and the page drops it.
    pub fn word_text(&self, page: &Page, i: u32) -> &str {
        let text = self.words.get(i as usize).map_or("", |w| self.text_of(w));
        if page.period_dropped && self.last_visible(page) == Some(i) {
            without_final_stop(text).unwrap_or(text)
        } else {
            text
        }
    }

    /// What `page` draws, appended to `out`: its words that aren't hidden, a space between two where the text has one,
    /// a `\n` before one whose look starts a line. `each` gets every word drawn with its bytes in `out`.
    pub fn write_shown(
        &self,
        page: &Page,
        out: &mut String,
        mut each: impl FnMut(&PageWord, u32, u32),
    ) {
        let last = page
            .period_dropped
            .then(|| self.last_visible(page))
            .flatten();
        let mut first = true;
        for (i, w) in (page.words.0..).zip(
            self.words
                .get(page.words.0 as usize..page.words.1 as usize)
                .unwrap_or_default(),
        ) {
            if w.hidden {
                continue;
            }
            if !first {
                if w.line_break {
                    out.push('\n');
                } else if self
                    .text
                    .as_bytes()
                    .get((w.text.0 as usize).wrapping_sub(1))
                    == Some(&b' ')
                {
                    out.push(' ');
                }
            }
            first = false;
            let from = out.len() as u32;
            let text = self.text_of(w);
            // `strip_suffix`: a damaged `captions.json` that marks a period where there is none shows the word.
            out.push_str(if last == Some(i) {
                without_final_stop(text).unwrap_or(text)
            } else {
                text
            });
            each(w, from, out.len() as u32);
        }
    }

    /// `write_shown` as a new string.
    pub fn shown_text(&self, page: &Page) -> String {
        let mut out = String::new();
        self.write_shown(page, &mut out, |_, _, _| {});
        out
    }

    /// One page from `start_us` to `end_us` of plain words (text, heard start and end) from file `asset`, the source
    /// times their heard times: for tests and fixtures that need pages without a transcript.
    pub fn one_page(asset: &str, start_us: Us, end_us: Us, words: &[(&str, Us, Us)]) -> Pages {
        let mut text = String::new();
        let mut out = Vec::with_capacity(words.len());
        for &(word, start, end) in words {
            if !text.is_empty() {
                text.push(' ');
            }
            let from = text.len() as u32;
            text.push_str(word);
            out.push(PageWord {
                text: (from, text.len() as u32),
                start_us: start,
                end_us: end,
                asset: 0,
                source_start_us: start,
                source_end_us: end,
                look: NO_LOOK,
                line_break: false,
                hidden: false,
                cut_before: false,
            });
        }
        Pages {
            pages: vec![Page {
                start_us,
                end_us,
                words: (0, out.len() as u32),
                index: 0,
                keyword: false,
                period_dropped: false,
            }],
            keywords: vec![None; out.len()],
            words: Arc::new(out),
            text: Arc::new(text),
            assets: Arc::new(vec![asset.to_owned()]),
            looks: Arc::default(),
        }
    }
}

/// The page on screen at timeline time `t` (binary search; allocates nothing). A page of hidden words only never is:
/// it takes no time, and sits no earlier than the page before it ends.
pub fn page_at(pages: &Pages, t: Us) -> Option<usize> {
    let i = pages.pages.partition_point(|p| p.start_us <= t);
    i.checked_sub(1).filter(|&i| t < pages.pages[i].end_us)
}
