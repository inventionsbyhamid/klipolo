//! The caption file writers: SubRip and WebVTT from `sidecar`'s cues. A cue's words are untrusted text:
//! control and direction-override characters never reach the file, each line stays one line (no word
//! can end a cue with a blank line), and WebVTT escapes `&`, `<` and `>` (so a word can't open a tag, an entity or a
//! timing: `-->`).

use std::fmt::Write as _;

use super::sidecar::SideCue;
use crate::time::Us;

/// `us` as `HH:MM:SS<sep>mmm` (milliseconds floored; hours past 99 keep their digits).
fn stamp(out: &mut String, us: Us, sep: char) {
    let ms = us.max(0) / 1000;
    let _ = write!(
        out,
        "{:02}:{:02}:{:02}{sep}{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    );
}

/// A line separator, tab or other break inside a word: a space there, never a new line.
fn breaks_line(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Characters that never reach a file: controls, the direction overrides, embeddings and isolates (which can make
/// a line read as something else), and the byte-order mark.
fn dropped(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// Writes `line` cleaned to `out`: breaks become single spaces (none at either end), dropped characters go, and for
/// WebVTT `&`, `<` and `>` are escaped. False when nothing of it is left (it makes no line).
fn push_line(out: &mut String, line: &str, vtt: bool) -> bool {
    let from = out.len();
    let mut gap = false;
    for c in line.chars() {
        if breaks_line(c) {
            gap = out.len() > from;
            continue;
        }
        if dropped(c) {
            continue;
        }
        if std::mem::take(&mut gap) {
            out.push(' ');
        }
        match c {
            '&' if vtt => out.push_str("&amp;"),
            '<' if vtt => out.push_str("&lt;"),
            '>' if vtt => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    if out.len() == from {
        return false;
    }
    out.push('\n');
    true
}

/// The cue's lines, cleaned, as one block ending in `\n`; empty when nothing of them is left.
fn cue_text(cue: &SideCue, vtt: bool) -> String {
    let mut text = String::new();
    for line in &cue.lines {
        push_line(&mut text, line, vtt);
    }
    text
}

/// SubRip: numbered cues, `HH:MM:SS,mmm --> HH:MM:SS,mmm`, each ending in a blank line. A cue with nothing left to
/// show (or no time) is left out, and the numbers stay in order.
pub fn srt(cues: &[SideCue]) -> String {
    let mut out = String::with_capacity(cues.len() * 64);
    let mut n = 0;
    for c in cues.iter().filter(|c| c.end_us > c.start_us) {
        let text = cue_text(c, false);
        if text.is_empty() {
            continue;
        }
        n += 1;
        let _ = writeln!(out, "{n}");
        stamp(&mut out, c.start_us, ',');
        out.push_str(" --> ");
        stamp(&mut out, c.end_us, ',');
        out.push('\n');
        out.push_str(&text);
        out.push('\n');
    }
    out
}

/// WebVTT: the header, then `HH:MM:SS.mmm --> HH:MM:SS.mmm` cues, each ending in a blank line.
pub fn vtt(cues: &[SideCue]) -> String {
    let mut out = String::with_capacity(8 + cues.len() * 64);
    out.push_str("WEBVTT\n\n");
    for c in cues.iter().filter(|c| c.end_us > c.start_us) {
        let text = cue_text(c, true);
        if text.is_empty() {
            continue;
        }
        stamp(&mut out, c.start_us, '.');
        out.push_str(" --> ");
        stamp(&mut out, c.end_us, '.');
        out.push('\n');
        out.push_str(&text);
        out.push('\n');
    }
    out
}
