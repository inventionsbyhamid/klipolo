//! The transcript as an app reads it (`getTranscript`, `klipolo://transcript`): every line at full detail
//! (`klipolo_chat::full_transcript`), the whole text when it fits in `PAGE_BYTES`, else a list of pages, each a time
//! range of whole lines that reads whole. A range an app asks for takes every line it overlaps; past `PAGE_BYTES` it is
//! cut at a whole line and names where to go on. The notes (clips not transcribed yet) come last on every read.
//!
//! Times are timeline seconds, written exactly (`312.34`, `75.5`), so a page edge is a line's start to the
//! microsecond: the line before it ends at or before it and never shows on two pages. Cost: one pass over the lines.

use std::fmt::Write as _;

use klipolo_chat::{FullTranscript, TranscriptLine};
use klipolo_model::time::{MAX_TIME_US, Us};

use crate::words::LINES_ARE;

/// The most a read of the transcript says (about 10,000 tokens).
pub const PAGE_BYTES: usize = 40 * 1024;
/// The page template's start: `klipolo://transcript/{from}-{to}`.
pub const TEMPLATE: &str = "klipolo://transcript/";
/// Room a listed page keeps for its head and the line after its lines, so it reads whole.
const PAGE_HEAD: usize = 1024;
/// Room a range read keeps for where to go on.
const MORE_ROOM: usize = 256;

/// What `t` says from `from` to `to` (timeline microseconds). With neither, the whole text when it fits, else the list
/// of its pages; with either, the lines that range overlaps (from the start, or to the end, for the one not given).
pub fn page(t: &FullTranscript, from: Option<Us>, to: Option<Us>) -> String {
    if from.is_none() && to.is_none() {
        whole_or_index(t)
    } else {
        range(t, from, to)
    }
}

/// A time as an app writes it: seconds (`75.5`), `m:ss` (`1:05`) or `h:mm:ss` (`1:02:03.25`); `:` may come as
/// `%3A` from a URI. None for anything else: a sign, an exponent, a minute or second of 60 or more after a colon.
pub fn parse_time(s: &str) -> Option<Us> {
    let s = s.trim().replace("%3A", ":").replace("%3a", ":");
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let (seconds, clock) = parts.split_last()?;
    let mut minutes: u64 = 0;
    for (i, part) in clock.iter().enumerate() {
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let value: u64 = part.parse().ok()?;
        // The minutes of `h:mm:ss`.
        if i > 0 && value >= 60 {
            return None;
        }
        minutes = minutes.checked_mul(60)?.checked_add(value)?;
    }
    if !seconds.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    let seconds: f64 = seconds.parse().ok()?;
    if !clock.is_empty() && seconds >= 60.0 {
        return None;
    }
    let us = (minutes as f64 * 60.0 + seconds) * 1e6;
    (us.is_finite() && us <= MAX_TIME_US as f64).then(|| us.round() as Us)
}

/// Seconds, exactly, without trailing zeros: `312.34`, `75.5`, `0`.
fn stamp(us: Us) -> String {
    let us = us.max(0);
    let (whole, micros) = (us / 1_000_000, us % 1_000_000);
    if micros == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{micros:06}")
            .trim_end_matches('0')
            .to_owned()
    }
}

/// What `lines` take, one line break each.
fn bytes(lines: &[TranscriptLine]) -> usize {
    lines.iter().map(|l| l.text.len() + 1).sum()
}

/// Just past the last line, at a whole 10 ms: where the last page ends.
fn end_of(t: &FullTranscript) -> Us {
    t.lines.last().map_or(0, |l| {
        let end = l.range.end.max(l.range.start + 1);
        (end.max(0) + 9_999) / 10_000 * 10_000
    })
}

fn whole_or_index(t: &FullTranscript) -> String {
    let head = format!("Transcript. {LINES_ARE}\n");
    if head.len() + bytes(&t.lines) + t.notes.len() <= PAGE_BYTES {
        let mut out = head;
        for l in &t.lines {
            out.push_str(&l.text);
            out.push('\n');
        }
        out.push_str(&t.notes);
        return out;
    }
    let pages = split(t);
    let mut out = format!(
        "Transcript, in {} pages as it is long. {LINES_ARE} Read a page with getTranscript and the times listed for it, \
         or as {TEMPLATE}<from>-<to>.\n",
        pages.len()
    );
    let end = end_of(t);
    for (k, lines) in pages.iter().enumerate() {
        let from = if k == 0 { 0 } else { lines[0].range.start };
        let to = pages.get(k + 1).map_or(end, |next| next[0].range.start);
        let (from, to) = (stamp(from), stamp(to));
        let _ = writeln!(
            out,
            "fromSec {from} toSec {to}: {} lines, {} KB ({TEMPLATE}{from}-{to})",
            lines.len(),
            bytes(lines).div_ceil(1024)
        );
    }
    out.push_str(&t.notes);
    out
}

/// The lines in pages that each read whole: with a head and the notes, within `PAGE_BYTES`. At least one line a page.
fn split(t: &FullTranscript) -> Vec<&[TranscriptLine]> {
    let room = PAGE_BYTES.saturating_sub(PAGE_HEAD + t.notes.len());
    let mut pages = Vec::new();
    let (mut from, mut used) = (0, 0);
    for (i, l) in t.lines.iter().enumerate() {
        let size = l.text.len() + 1;
        if i > from && used + size > room {
            pages.push(&t.lines[from..i]);
            (from, used) = (i, 0);
        }
        used += size;
    }
    if from < t.lines.len() {
        pages.push(&t.lines[from..]);
    }
    pages
}

/// The lines `from..to` overlaps, at most `PAGE_BYTES` with where to go on when they don't all fit.
fn range(t: &FullTranscript, from: Option<Us>, to: Option<Us>) -> String {
    let start = from.unwrap_or(0).max(0);
    let end = to.unwrap_or_else(|| end_of(t));
    let (a, b) = (stamp(start), stamp(end));
    let mut out = format!(
        "Transcript from {a} to {b} s, in whole lines (the first may start before {a}). {LINES_ARE}\n"
    );
    if start >= end {
        out.push_str("Nothing to read: the start must be before the end.\n");
        out.push_str(&t.notes);
        return out;
    }
    // A line overlaps when it ends after the start and starts before the end; without a start, every line from the
    // first does.
    let first = match from {
        Some(_) => t.lines.partition_point(|l| l.range.end <= start),
        None => 0,
    };
    let mut shown = 0;
    for l in t.lines[first..].iter().take_while(|l| l.range.start < end) {
        if shown > 0 && out.len() + l.text.len() + 1 + t.notes.len() + MORE_ROOM > PAGE_BYTES {
            let next = stamp(l.range.start);
            let _ = writeln!(
                out,
                "More follows: call getTranscript with fromSec {next} and toSec {b}, or read {TEMPLATE}{next}-{b}."
            );
            break;
        }
        out.push_str(&l.text);
        out.push('\n');
        shown += 1;
    }
    if shown == 0 {
        let _ = writeln!(out, "Nothing is said from {a} to {b} s.");
    }
    out.push_str(&t.notes);
    out
}
