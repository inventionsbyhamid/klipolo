//! The shown-word stream captions are made from: every word the speech map shows, in
//! timeline order, with what it shows, when it is heard and where it comes from. One walk feeds every format's pages
//! (`Sink`). O(clips + words) time; it allocates only its table of shot ends (`shot_ends`), one entry per clip.
//!
//! It reads the speech map's text (`Spoken::text`), never a fix's text directly: a fix reads once per use of its words,
//! and its later pieces show nothing. A fixed piece is heard from its first word heard to its last, not from a
//! pause it also covers: after a cut, its piece can start with the pause. A later piece of the same use
//! right after it (a pause removed or a split inside the fix) carries the word on to its last word (`Sink::carry_on`).

use std::collections::HashMap;

use crate::project::Project;
use crate::speech::{
    is_wide, joins_unspaced, Speech, Spoken, SpokenWord, Transcript, Word, WordFix, WordKind,
};
use crate::time::Us;

/// A fix's later piece this far or further from the word it continues doesn't carry it on.
pub const BREAK_GAP_US: Us = 500_000;

/// A word's width in columns: wide characters (Han, kana, Hangul, full-width forms) take two.
pub(crate) fn columns(s: &str) -> usize {
    if s.is_ascii() {
        return s.len();
    }
    s.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

/// `text` without the closing quotes and brackets after its last mark.
fn unquoted(text: &str) -> &str {
    text.trim_end_matches(['"', '\'', '”', '’', '»', ')', '」', '』', '）'])
}

/// Whether `text` ends a sentence (closing quotes and brackets aside), and whether it ends a clause: a comma, semicolon
/// or colon (a page breaks after one once it has two words). A word ending in an ASCII letter or digit, the usual
/// case, ends neither.
fn ends(text: &str) -> (bool, bool) {
    if text
        .as_bytes()
        .last()
        .is_some_and(u8::is_ascii_alphanumeric)
    {
        return (false, false);
    }
    let t = unquoted(text);
    (
        t.ends_with(['.', '?', '!', '…', '。', '？', '！']),
        t.ends_with([',', ';', ':', '，', '、', '；', '：']),
    )
}

/// Whether `s` ends with a character of the scripts written without spaces between words (Han and kana: Chinese and
/// Japanese); a word that starts with one (`starts_joining`) then follows it without a space. Korean keeps its spaces.
pub(crate) fn ends_joining(s: &str) -> bool {
    // Every such character is outside ASCII.
    !s.as_bytes().last().is_none_or(u8::is_ascii) && s.ends_with(joins_unspaced)
}

pub(crate) fn starts_joining(s: &str) -> bool {
    !s.as_bytes().first().is_none_or(u8::is_ascii) && s.starts_with(joins_unspaced)
}

/// When `w` is heard on the timeline, and where it comes from in its file's own time (looks are found there), in one
/// pass over the words a fix's piece covers (O(words it covers)); `t` is its span's transcript, looked up once per span.
/// A fix's piece is heard from the first word it covers that isn't a pause to the last one (None when it covers only
/// pauses) and comes from the first start to the last end of the words it covers in its span; any other word is heard
/// over its own times and comes from its transcript word.
fn heard_and_source(
    spoken: &Spoken,
    w: &SpokenWord,
    t: Option<&Transcript>,
    fix: Option<&WordFix>,
) -> (Option<(Us, Us)>, (Us, Us)) {
    let own = Some((w.start_us, w.end_us));
    let Some(first) = t.and_then(|t| t.words.get(w.word as usize)) else {
        return (own, (0, 0));
    };
    let (Some(t), Some(fix)) = (t, fix) else {
        return (own, (first.start_us, first.end_us));
    };
    let span = &spoken.spans[w.span as usize];
    let end = (span.end as usize).min(t.words.len());
    let (mut last, mut said) = (first, None);
    for x in t.words.get(w.word as usize..end).unwrap_or_default() {
        if x.start_us + (x.end_us - x.start_us) / 2 >= fix.end_us {
            break;
        }
        last = x;
        if x.kind != WordKind::Pause {
            said = Some(said.map_or((x, x), |(a, _)| (a, x)));
        }
    }
    let heard = said.map(|(a, b): (&Word, &Word)| {
        (
            span.at(a.start_us).max(w.start_us),
            span.at(b.end_us).min(w.end_us),
        )
    });
    (heard, (first.start_us, last.end_us))
}

/// Where spoken word `w` comes from in its file's own time, as the pages find its look (by its middle): its
/// transcript word, or the words a correction's piece covers in its span (`heard_and_source`). None without one, so a
/// tool sets a word's look exactly where the captions read it.
pub fn word_source(
    spoken: &Spoken,
    w: &SpokenWord,
    p: &Project,
    speech: &Speech,
) -> Option<(Us, Us)> {
    let t = speech.transcripts.get(spoken.asset(w)).map(|t| &**t)?;
    if w.word as usize >= t.words.len() {
        return None;
    }
    let fix = w.fix.and_then(|i| p.word_fixes.get(i as usize));
    Some(heard_and_source(spoken, w, Some(t), fix).1)
}

/// Where fix piece `w` starts and ends on the timeline for telling whether two pieces of a use meet: its span's edge
/// where the fix goes on past it (the pieces of a split or a cut inside the fix then meet exactly, whatever silence of
/// the fix's own lies between their words), else its own times. O(1).
fn piece_edges(spoken: &Spoken, w: &SpokenWord, fix: &WordFix) -> (Us, Us) {
    let span = &spoken.spans[w.span as usize];
    let start = if fix.start_us < span.asset_in_us {
        span.from_us()
    } else {
        w.start_us
    };
    let end = if fix.end_us > span.asset_out_us {
        span.end_us()
    } else {
        w.end_us
    };
    (start, end)
}

/// One word as captions take it.
pub(crate) struct Said<'a> {
    /// What it shows, as heard or fixed (trimmed; never empty).
    pub text: &'a str,
    pub span: u32,
    /// When it is heard on the timeline.
    pub start_us: Us,
    pub end_us: Us,
    /// The file it comes from, and where in that file's own time.
    pub asset: &'a str,
    pub source_start_us: Us,
    pub source_end_us: Us,
    /// The fix it reads, and where its piece ends on the timeline (`piece_edges`).
    pub fix: Option<(u32, Us)>,
    /// A fix's piece of pauses only: its times are the piece's own until a continued piece says when it is heard.
    pub unheard: bool,
    pub ends_sentence: bool,
    pub ends_clause: bool,
    /// The first word, or the first after one that ended a sentence (keywords: a capital there isn't a proper noun).
    pub sentence_start: bool,
    /// Where its shot ends on the timeline (`shot_ends`): a cut, which a page's hold doesn't run past and no page
    /// spans. Words of one shot share it, a split left in place included, so it names the shot.
    pub cut_us: Us,
}

/// What the walk feeds: every word shown, and every later piece of a fix's use.
pub(crate) trait Sink {
    fn add(&mut self, w: &Said);

    /// A later piece of fix `fix`'s use (`continued`: no text of its own) between `edges` on the timeline
    /// (`piece_edges`), heard over `heard` (None: only pauses). When the open page's last word reads that fix
    /// and the piece comes right after it (a removed pause or a split inside the fix: their spans meet, less than
    /// `BREAK_GAP_US` apart), the word goes on to the end of what the piece says, so the caption and Karaoke's colour
    /// cover the whole phrase. A piece placed elsewhere stays without a caption (never the text twice). A word of pauses
    /// only takes the piece's heard times. `cut_us`: where the piece's shot ends on the timeline (`shot_ends`).
    fn carry_on(&mut self, fix: u32, edges: (Us, Us), cut_us: Us, heard: Option<(Us, Us)>);
}

/// Each main-track clip's shot end: where on the timeline the shot it is part of ends, a real cut.
/// Clip `k + 1` goes on from clip `k` (no cut) when `k` has no transition out and `k + 1` plays the same source from
/// where `k` stops, at any speed: a split left in place. Made from the main track, not from the spans (`spans` leaves
/// out the ones without words). O(clips).
fn shot_ends(p: &Project) -> HashMap<&str, Us> {
    let main = &p.timeline.main;
    let starts = crate::query::clip_starts(main);
    let mut ends = HashMap::with_capacity(main.len());
    let mut shot_end: Us = 0;
    for (k, clip) in main.iter().enumerate().rev() {
        let goes_on = main.get(k + 1).is_some_and(|next| {
            clip.transition_out.is_none()
                && next.source == clip.source
                && next.source_in_us == clip.source_out_us
        });
        if !goes_on {
            shot_end = starts[k] + clip.duration_us();
        }
        ends.insert(clip.id.as_str(), shot_end);
    }
    ends
}

/// Feeds `sink` every word `spoken` shows, in timeline order (pauses and empty words skipped), each span's transcript
/// and cut looked up once.
pub(crate) fn walk(spoken: &Spoken, p: &Project, speech: &Speech, sink: &mut impl Sink) {
    let shots = shot_ends(p);
    let cut_of = |span: u32| {
        let s = &spoken.spans[span as usize];
        shots
            .get(s.clip.as_str())
            .copied()
            .unwrap_or_else(|| s.end_us())
    };
    // The current span's transcript and cut, looked up once per span.
    let mut transcript: Option<(u32, Option<&Transcript>, Us)> = None;
    let mut sentence_start = true;
    for (w, text) in spoken.shown(p, speech) {
        if w.kind == WordKind::Pause {
            continue;
        }
        let (t, cut_us) = match transcript {
            Some((span, t, cut)) if span == w.span => (t, cut),
            _ => {
                let t = speech.transcripts.get(spoken.asset(w)).map(|t| &**t);
                let cut = cut_of(w.span);
                transcript = Some((w.span, t, cut));
                (t, cut)
            }
        };
        let fixed = w
            .fix
            .and_then(|i| p.word_fixes.get(i as usize).map(|f| (i, f)));
        if let (Some((i, fix)), true) = (fixed, w.continued) {
            let (heard, _) = heard_and_source(spoken, w, t, Some(fix));
            sink.carry_on(i, piece_edges(spoken, w, fix), cut_us, heard);
            continue;
        }
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let (heard, (source_start_us, source_end_us)) =
            heard_and_source(spoken, w, t, fixed.map(|(_, f)| f));
        let (start_us, end_us) = heard.unwrap_or((w.start_us, w.end_us));
        let (ends_sentence, ends_clause) = ends(text);
        let said = Said {
            text,
            span: w.span,
            start_us,
            end_us,
            asset: spoken.asset(w),
            source_start_us,
            source_end_us,
            fix: fixed.map(|(i, fix)| (i, piece_edges(spoken, w, fix).1)),
            unheard: heard.is_none(),
            ends_sentence,
            ends_clause,
            sentence_start,
            cut_us,
        };
        sentence_start = said.ends_sentence;
        sink.add(&said);
    }
}
