//! The shapes the Captions panel reads (`captions_pages`, `captions_find`).

use klipolo_model::captions::Keyword;
use klipolo_model::captions::looks::Look;
use klipolo_model::ids::Id;
use klipolo_model::project::FormatKind;
use klipolo_model::time::Us;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A window of one format's pages (`captions_pages`).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CaptionPagesView {
    /// The revision they were made from.
    #[ts(type = "number")]
    pub revision: u64,
    pub format: FormatKind,
    /// The format's pages in all (none while captions are off).
    pub total: u32,
    /// `pages[0]`'s place among them.
    pub first: u32,
    /// The query asked for more pages than one answer holds (`MAX_PAGES`): the window goes on past the last one here.
    pub capped: bool,
    pub pages: Vec<PageView>,
}

/// One page as drawn: when it shows on the timeline, and its words in order (hidden ones included, marked by their
/// look).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PageView {
    pub index: u32,
    pub start_us: Us,
    pub end_us: Us,
    pub words: Vec<WordView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WordView {
    /// What it shows: the style's case and marks applied, a dropped final period left out.
    pub text: String,
    /// What was said, or the person's fix, when it reads otherwise than `text` (an upper-case style): what a fix starts
    /// from, so a fix never stores the style's capitals.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub said: Option<String>,
    /// It follows the word before without a space (Chinese and Japanese): a fix joins them so, and keeps no space the
    /// captions never had.
    pub joined: bool,
    /// When it is heard on the timeline.
    pub start_us: Us,
    pub end_us: Us,
    /// Its file, and where in that file's sound time (what `fixWords` and `setWordLook` take): a correction's whole
    /// range when it reads one, so fixing it again replaces all of it, the pieces a split or a cut left after it
    /// included.
    pub asset_id: Id,
    pub source_start_us: Us,
    pub source_end_us: Us,
    /// The person's look for it (plain when they set none).
    pub look: Look,
    pub keyword: Option<Keyword>,
    /// `Some(true)`: a cut lies between it and the page word before it (another clip, or another file of a clip), so
    /// the file time between them may play elsewhere: a fix stops there.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cut_before: Option<bool>,
    /// Where in its file the pages read its look, when that isn't the middle of `source_start_us..source_end_us` (the
    /// piece of a correction it shows): the word list reads the person's look there.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub look_us: Option<Us>,
    /// `Some(true)`: the transcript heard a filler here ("um", "uh"); the word list draws it dashed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub filler: Option<bool>,
    /// What the transcript heard over a correction's range, when the word reads the person's fix: the word list says
    /// what it was fixed from.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub heard: Option<String>,
}

/// Which pages: those overlapping a stretch of the timeline, or a run of them by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export, rename = "CaptionPagesQuery")]
pub enum PagesQuery {
    Range { from_us: Us, to_us: Us },
    Index { from: u32, count: u32 },
}
