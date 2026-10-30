//! Caption files for long-form exports: SRT or WebVTT beside a landscape
//! video, from Clean's pages (its lines and line length: 2 of at most 42 columns, Netflix timing) whatever style is
//! Hidden words stay in the pages and out of the file.
//!
//! One source of truth with what is drawn:
//! - the cues are pages, never paged again differently: the very pages an export freezes in `captions.json` when it
//!   burns Clean in, else Clean's pages of the same project and speech (`sidecar_pages`), made once when the job is;
//! - a page's words are walked as the renderer walks them (`lines::drawn_words`: hidden words out, a dropped final
//!   period, a hidden word's line break kept), and its lines balanced by the same breaker (`lines::break_lines`), in
//!   columns here instead of pixels.
//!
//! Words are untrusted text: both writers strip control and direction-override characters and keep each
//! line on one line, so no word can end a cue with a blank line; WebVTT also escapes `&`, `<` and `>`, so a word can't
//! open a tag, an entity or a timing (`-->`).
//!
//! Cost: O(words) per file plus `break_lines`' search per page; the per-page buffers are reused.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::lines::{break_lines, drawn_words, is_short, pyramid, LineInput};
use super::pages::{Captions, Pages};
use super::paging::captions_with;
use super::said::columns;
pub use super::sidecar_write::{srt, vtt};
use super::style::CaptionStyleDef;
use super::styles::{CaptionStyles, FALLBACK_STYLE};
use crate::contrib::ContribId;
use crate::project::{FormatKind, Project};
use crate::speech::Speech;
use crate::time::Us;

/// The caption file an export saves beside each landscape video (the export popover's choice, remembered per person).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CaptionFile {
    /// SubRip (`.srt`).
    Srt,
    /// WebVTT (`.vtt`).
    Vtt,
}

impl CaptionFile {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }

    /// `cues` as this kind of file.
    pub fn write(self, cues: &[SideCue]) -> String {
        match self {
            Self::Srt => srt(cues),
            Self::Vtt => vtt(cues),
        }
    }
}

/// One cue: a page's time on the video and its lines.
#[derive(Clone, Debug, PartialEq)]
pub struct SideCue {
    pub start_us: Us,
    pub end_us: Us,
    pub lines: Vec<String>,
}

/// A caption file's cues for `p` and `speech`: Clean's landscape pages, made even with captions off in the video.
pub fn sidecar_cues(p: &Project, speech: &Speech) -> Vec<SideCue> {
    sidecar_pages(p, speech, &Captions::default()).map_or_else(Vec::new, |pages| cues_of(&pages))
}

/// The pages a caption file is made from: `burned`'s landscape pages themselves when they are Clean's (the export
/// burns Clean in: one source of truth), else Clean's landscape pages of `p` and `speech`, whatever style and switch
/// the project has. None without a transcript to page.
pub fn sidecar_pages(p: &Project, speech: &Speech, burned: &Captions) -> Option<Arc<Pages>> {
    if burned.style.0 == FALLBACK_STYLE {
        if let Some(pages) = burned.pages.get(&FormatKind::Landscape) {
            return Some(pages.clone());
        }
    }
    let mut clean = p.captions.clone();
    clean.enabled = true;
    clean.style = Some(ContribId::from(FALLBACK_STYLE));
    // A look's variant and accent only paint: Clean's pages are the same with or without them.
    (clean.variant, clean.accent) = (None, None);
    let mut made = captions_with(p, &clean, speech, &[FormatKind::Landscape]);
    made.pages.remove(&FormatKind::Landscape)
}

/// Clean, whose lines a caption file keeps.
fn clean_style() -> &'static Arc<CaptionStyleDef> {
    CaptionStyles::builtin()
        .get(&ContribId::from(FALLBACK_STYLE))
        .expect("Clean is built in")
}

/// The cues of `pages`: one per page that draws a word, its lines as Clean breaks them (`max_lines` lines of at most
/// `max_chars` columns). Pages of hidden words only take no time and make no cue.
pub fn cues_of(pages: &Pages) -> Vec<SideCue> {
    let clean = clean_style();
    let rules = Rules {
        lines: clean.pages.max_lines.max(1) as usize,
        columns: clean
            .pages
            .max_chars
            .as_ref()
            .map_or(42, |m| m.get(FormatKind::Landscape)) as usize,
        pyramid: pyramid(clean),
    };
    let mut cues = Vec::with_capacity(pages.pages.len());
    let mut words = Words::default();
    for page in &pages.pages {
        words.clear();
        drawn_words(pages, page, |d| {
            words.texts.push(d.text);
            words.widths.push(columns(d.text) as f32);
            words.spaced.push(d.space_before);
            words.forced.push(d.forced);
            words.short.push(is_short(d.text));
        });
        if words.texts.is_empty() || page.end_us <= page.start_us {
            continue;
        }
        cues.push(SideCue {
            start_us: page.start_us,
            end_us: page.end_us,
            lines: words.lines(&rules),
        });
    }
    cues
}

struct Rules {
    lines: usize,
    columns: usize,
    pyramid: bool,
}

/// One page's drawn words, kept from page to page.
#[derive(Default)]
struct Words<'a> {
    texts: Vec<&'a str>,
    widths: Vec<f32>,
    /// A space before it on its line (none where the script joins its words).
    spaced: Vec<bool>,
    forced: Vec<bool>,
    short: Vec<bool>,
}

impl Words<'_> {
    fn clear(&mut self) {
        self.texts.clear();
        self.widths.clear();
        self.spaced.clear();
        self.forced.clear();
        self.short.clear();
    }

    /// At most `rules.lines` lines through the drawn captions' breaker: the fewest that fit, then the most balanced,
    /// the person's line breaks kept. A single word longer than the line keeps its own line (never cut). The person's
    /// breaks can ask for more lines than that (`break_lines` allows one per break); a caption file never shows them,
    /// so the rest joins the last line.
    fn lines(&self, rules: &Rules) -> Vec<String> {
        let n = self.texts.len();
        let space = if self.spaced.iter().skip(1).any(|&s| s) {
            1.0
        } else {
            0.0
        };
        let breaks = break_lines(&LineInput {
            widths: &self.widths,
            space,
            max_width: rules.columns as f32,
            max_lines: rules.lines,
            forced: &self.forced,
            short: &self.short,
            pyramid: rules.pyramid,
        });
        let mut starts: Vec<usize> = breaks.starts.iter().map(|&s| usize::from(s)).collect();
        if starts.first() != Some(&0) {
            starts.insert(0, 0);
        }
        starts.truncate(rules.lines);
        (0..starts.len())
            .map(|k| {
                let end = starts.get(k + 1).copied().unwrap_or(n);
                let mut line = String::new();
                for i in starts[k]..end {
                    if i > starts[k] && self.spaced[i] {
                        line.push(' ');
                    }
                    line.push_str(self.texts[i]);
                }
                line
            })
            .collect()
    }
}
