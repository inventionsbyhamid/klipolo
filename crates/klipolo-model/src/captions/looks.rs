//! The person's look for caption words, in each file's own time like word fixes, so it follows the
//! words through cuts, speed changes and undo, and stays when the style changes. Looks never overlap: setting a range
//! replaces what was inside it.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::style::hex_ok;
use crate::error::{invalid, Result};
use crate::ids::Id;
use crate::messages::code;
use crate::project::Project;
use crate::time::{RangeUs, Us, MAX_TIME_US};

/// The size steps a look may take, each ×1.15: −2 is about ×0.76 and 3 about ×1.52.
pub const LOOK_SIZE_STEPS: RangeInclusive<i8> = -2..=3;

/// A word's look over its style's. Every part is optional: unset, the style decides. Of the switches, only `emphasis`
/// has a `false` of its own ("never a keyword"); no style highlights, hides or breaks a line at a word by itself, so
/// for those `false` reads as unset. Named `CaptionLook` for the UI and the tool schemas, where a clip's colour look is
/// already `Look`. An unknown key is refused: a misspelled one read as an empty look would clear the range.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename = "CaptionLook")]
#[schemars(rename = "CaptionLook")]
pub struct Look {
    /// The word's fill, `#RRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub color: Option<String>,
    /// Always a keyword (`true`) or never one (`false`); unset: the style's picker decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub emphasis: Option<bool>,
    /// Size steps of ×1.15, −2 to 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub size: Option<i8>,
    /// The style's word box behind it (or one in the accent colour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub highlight: Option<bool>,
    /// Off screen and out of caption files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hidden: Option<bool>,
    /// Starts a new line (a new page at the style's last line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line_break: Option<bool>,
}

impl Look {
    pub fn is_empty(&self) -> bool {
        *self == Look::default()
    }
}

/// The look of a word without one: every part unset.
pub static PLAIN_LOOK: Look = Look {
    color: None,
    emphasis: None,
    size: None,
    highlight: None,
    hidden: None,
    line_break: None,
};

/// One look over `[start_us, end_us)` of a file's own time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WordLook {
    pub asset_id: Id,
    pub start_us: Us,
    pub end_us: Us,
    pub look: Look,
}

/// `ranges` sorted by start, with those that overlap or touch as one: what [`set_word_looks`] takes. O(r log r).
pub fn sorted_ranges(ranges: &[RangeUs]) -> Vec<RangeUs> {
    let mut sorted = ranges.to_vec();
    sorted.sort_unstable_by_key(|r| r.start);
    let mut out: Vec<RangeUs> = Vec::with_capacity(sorted.len());
    for r in sorted {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// `look` over each of `ranges` (sorted, apart: [`sorted_ranges`]) of `asset`, replacing the looks inside them: those
/// they overlap keep only their parts outside them. An empty look only clears. The list stays sorted by (asset, start).
///
/// Sorted and never overlapping, a file's looks have their ends in order too, so the looks the ranges reach are one
/// window found by two binary searches. The window is written again in one pass, its looks' remaining parts and the new
/// looks in order, and goes back with one splice: O(log looks + window + ranges), one shift of the list's tail
/// whatever the number of ranges (one shift per word would be O(words × looks)).
pub fn set_word_looks(looks: &mut Vec<WordLook>, asset: &str, ranges: &[RangeUs], look: &Look) {
    let (Some(first), Some(last)) = (ranges.first(), ranges.last()) else {
        return;
    };
    let from = looks.partition_point(|l| (l.asset_id.as_str(), l.end_us) <= (asset, first.start));
    // `max`: a list that breaks the order (never a validated one) still can't make a backwards window.
    let to = looks
        .partition_point(|l| (l.asset_id.as_str(), l.start_us) < (asset, last.end))
        .max(from);
    let mut out = Vec::with_capacity(to - from + 2 * ranges.len());
    // `next`: the first range not written yet; `cut`: the first that may still reach the look being cut.
    let (mut next, mut cut) = (0, 0);
    let mut put = |out: &mut Vec<WordLook>, kept: WordLook| {
        // The new looks that start before the part kept end before it: they go first.
        while next < ranges.len() && ranges[next].start < kept.start_us {
            put_set(out, asset, ranges[next], look);
            next += 1;
        }
        out.push(kept);
    };
    for l in &looks[from..to] {
        while cut < ranges.len() && ranges[cut].end <= l.start_us {
            cut += 1;
        }
        let mut at = l.start_us;
        for r in ranges[cut..].iter().take_while(|r| r.start < l.end_us) {
            if r.start > at {
                put(&mut out, piece(l, at, r.start));
            }
            at = at.max(r.end);
        }
        if at < l.end_us {
            put(&mut out, piece(l, at, l.end_us));
        }
    }
    for &r in &ranges[next..] {
        put_set(&mut out, asset, r, look);
    }
    looks.splice(from..to, out);
}

/// Look `l` cut to `[start, end)`.
fn piece(l: &WordLook, start: Us, end: Us) -> WordLook {
    WordLook {
        start_us: start,
        end_us: end,
        ..l.clone()
    }
}

/// `look` over `r`, unless it is empty (it only clears).
fn put_set(out: &mut Vec<WordLook>, asset: &str, r: RangeUs, look: &Look) {
    if !look.is_empty() {
        out.push(WordLook {
            asset_id: asset.to_owned(),
            start_us: r.start,
            end_us: r.end,
            look: look.clone(),
        });
    }
}

/// Lookups for one build of the pages: the slice is sorted by (asset, start) and never overlaps, so a word's look is a
/// binary search. No allocation.
pub struct LookIndex<'a> {
    looks: &'a [WordLook],
}

impl<'a> LookIndex<'a> {
    pub fn new(looks: &'a [WordLook]) -> Self {
        Self { looks }
    }

    /// The look covering `mid_us` (a word's middle) in `asset`'s own time.
    pub fn at(&self, asset: &str, mid_us: Us) -> Option<&'a Look> {
        self.index_at(asset, mid_us).map(|i| &self.looks[i].look)
    }

    /// `at`'s place in the slice.
    pub fn index_at(&self, asset: &str, mid_us: Us) -> Option<usize> {
        let i = self
            .looks
            .partition_point(|l| (l.asset_id.as_str(), l.start_us) <= (asset, mid_us))
            .checked_sub(1)?;
        let l = &self.looks[i];
        (l.asset_id == asset && mid_us < l.end_us).then_some(i)
    }
}

/// The looks of the words one walk brings, span by span (one file each, its words in their own time's order): the file's
/// looks are found once per span by binary search, then a cursor only moves forward, comparing times. O(1) amortised
/// per word, with no string compares.
pub(crate) struct LookCursor {
    span: u32,
    /// The file's looks, `from..to`, and the first of them that may cover the next word.
    to: usize,
    at: usize,
    last_mid: Us,
}

impl LookCursor {
    pub(crate) fn new() -> Self {
        Self {
            span: u32::MAX,
            to: 0,
            at: 0,
            last_mid: Us::MIN,
        }
    }

    /// The place in `looks` (sorted by file and start, never overlapping: `validate_word_looks`) of the look covering
    /// `mid_us` (a word's middle) in `asset`'s own time, for a word of span `span`.
    pub(crate) fn at(
        &mut self,
        looks: &[WordLook],
        span: u32,
        asset: &str,
        mid_us: Us,
    ) -> Option<usize> {
        if span != self.span || mid_us < self.last_mid {
            let from = looks.partition_point(|l| l.asset_id.as_str() < asset);
            self.to = from + looks[from..].partition_point(|l| l.asset_id == asset);
            self.at = from + looks[from..self.to].partition_point(|l| l.end_us <= mid_us);
            self.span = span;
        }
        self.last_mid = mid_us;
        // A file's looks never overlap, so their ends rise with their starts.
        while self.at < self.to && looks[self.at].end_us <= mid_us {
            self.at += 1;
        }
        (self.at < self.to && looks[self.at].start_us <= mid_us).then_some(self.at)
    }
}

fn check(ok: bool, code: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid(code))
    }
}

/// `0 ≤ start < end ≤ MAX_TIME_US`.
pub(crate) fn range_ok(start: Us, end: Us) -> bool {
    0 <= start && start < end && end <= MAX_TIME_US
}

/// A look's own rules: a `#RRGGBB` colour and a size step from −2 to 3.
pub(crate) fn check_look(look: &Look) -> Result<()> {
    check(
        look.color.as_deref().is_none_or(hex_ok),
        code!("model.wordLook.color"),
    )?;
    check(
        look.size.is_none_or(|s| LOOK_SIZE_STEPS.contains(&s)),
        code!("model.wordLook.size"),
    )
}

/// The project's looks (`validate`): each on a file of the project, over valid times, with a valid look; sorted by file
/// and start and never overlapping on one file (`model.wordLook.overlap`: a file edited by hand), as `set_word_looks`
/// and `LookIndex` need. One media lookup per file: O(looks + files × media).
pub(crate) fn validate_word_looks(p: &Project) -> Result<()> {
    let mut last: Option<&WordLook> = None;
    for l in p.word_looks.iter() {
        let same_file = last.is_some_and(|x| x.asset_id == l.asset_id);
        check(
            same_file || p.asset(&l.asset_id).is_some(),
            code!("model.wordLook.asset"),
        )?;
        check(
            range_ok(l.start_us, l.end_us),
            code!("model.wordLook.range"),
        )?;
        check_look(&l.look)?;
        check(
            last.is_none_or(|x| {
                if same_file {
                    x.end_us <= l.start_us
                } else {
                    x.asset_id < l.asset_id
                }
            }),
            code!("model.wordLook.overlap"),
        )?;
        last = Some(l);
    }
    Ok(())
}
