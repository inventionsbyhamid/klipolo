//! Holding the model's times to what it was shown: it never invents a cut time.
//!
//! An edge of a range the model names must lie within `TOLERANCE_US` of something in its context or in the person's
//! message:
//! - a spoken word's start or end (pause words too);
//! - a pause in the sound (`timeline_pauses`, gaps of `PAUSE_US` or more);
//! - a scene item's start or end, as Scenes showed it. Scene cuts are items too, and only a lower bound: jump cuts are
//!   often missed, so they are one edge among the others, never required;
//! - a clip's start or end;
//! - the selection's ends, the playhead, the video's start (0) and the main track's end;
//! - a time the person typed (`typed_times`).
//!
//! The edge then snaps to the nearest of them. On a tie, `Source`'s order decides: the selection, then a typed time,
//! the playhead, the video's start or end, a clip's edge, a word, a pause, a scene item (what the person pointed at
//! before what was heard or seen). An edge on a word is padded as `word_cut` pads a cut of the words the range holds,
//! so a cut never clips a word it keeps or removes; no other edge is padded.
//!
//! A part to keep (`keep_range`) loses what trails after its last words, and nothing else: an end that snapped to a
//! word or a pause, and that lies a long pause (`TAIL_PAUSE_US`) after the last word said, ends at that word; and an
//! end that takes one stray word after such a pause (the next speaker's first word, said just before the rest of their
//! sentence) ends before the pause. A stray word that ends a sentence, or that its speech stops after, is the
//! speaker's own last word and stays. Starts never move, and an end never moves on: without the speakers' turns the
//! compiler can't tell a quick change of speaker from a phrase that runs on. An edge near a scene item, a clip's edge,
//! the video's ends or what the person pointed at or typed stays where it snapped.
//!
//! A moment the media tools place something at in seconds (`place`: a sound, a title, a sticker) needs no evidence: it
//! lands on what the person pointed at within the tolerance, else on the nearest word start or cut (a clip's start, a
//! change of shot or topic, the video's start) within `PLACE_US`, else where it was given. A moment given as the words
//! said there needs no snapping (`compile::quote`). A layer's new start or split (`moment`) stands on the evidence as
//! an edge does, landing on a change of shot or topic within the tolerance first.
//!
//! Times are the snapshot's timeline, as the context showed them; the compiler maps them into its working copy after.
//! Building costs O(words + pauses + scene items + clips) once per message; a lookup is a binary search.

use std::collections::HashMap;

use klipolo_model::Project;
use klipolo_model::query::{clip_starts, main_track_duration};
use klipolo_model::speech::silence::timeline_pauses;
use klipolo_model::speech::{PAUSE_US, Speech, Spoken, WordKind, spoken, word_cut};
use klipolo_model::time::{MAX_TIME_US, RangeUs, US_PER_SEC, Us, secs_to_us};

use crate::context::{
    BREATH_US, SceneLog, View, ends_sentence, scene_changes, scene_spans, within_video,
};

/// How far a model's time may lie from what it was shown.
pub const TOLERANCE_US: Us = 300_000;
/// How far a placed moment snaps to a word's start or a cut (`Evidence::place`).
pub const PLACE_US: Us = 500_000;
/// A pause this long or longer before a kept part's end trims it (`Evidence::keep_range`): a part that ends on it ends
/// at the word before, and a part that ends on one stray word after it ends before it.
pub const TAIL_PAUSE_US: Us = 1_000_000;

/// What an edge is. The order is the preference on a tie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Source {
    Selection,
    Typed,
    Playhead,
    Bound,
    Clip,
    Word,
    Pause,
    Scene,
}

/// Which edge of a range had nothing to stand on, or why the range is empty once snapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unfounded {
    Start,
    End,
    /// Both ends snap to the same edge (or cross): nothing lies between them. Holds where they meet.
    Empty(Us),
}

/// Whether `t` lies in one of `cut`'s ranges (each from its start, up to but not at its end).
pub(crate) fn in_cut(cut: &[RangeUs], t: Us) -> bool {
    cut.iter().any(|r| r.start <= t && t < r.end)
}

/// Every edge the model may name in one message's snapshot.
pub(crate) struct Evidence {
    /// Sorted by time, then by `Source`.
    edges: Vec<(Us, Source)>,
    /// Where a change of shot or topic starts (`context::scene_changes`), sorted.
    changes: Vec<Us>,
    /// Where something placed may land (`place`): each spoken word's start (pause words left out), each clip's
    /// start, each change of shot or topic (`context::scene_changes`) and the video's start; sorted, each once.
    starts: Vec<Us>,
    /// Each clip's start and the main track's end, sorted: a cut between clips is a break (`keep_range`).
    clip_edges: Vec<Us>,
    /// By spoken word: it ends a sentence, or is a filler, so a cut right after it is clean (`keep_range`).
    closes: Vec<bool>,
    spoken: Spoken,
    fps: u32,
}

impl Evidence {
    /// The edges in `p` (its words, pauses, clips and length) with its `speech` and `scenes` (by asset id), the
    /// person's `view` and the `message` they typed.
    pub(crate) fn new(
        p: &Project,
        speech: &Speech,
        scenes: &HashMap<String, SceneLog>,
        view: &View,
        message: &str,
    ) -> Self {
        let spoken = spoken(p, speech);
        let mut edges: Vec<(Us, Source)> = Vec::with_capacity(spoken.words.len() * 2);
        let mut both = |r: RangeUs, source: Source| {
            edges.push((r.start, source));
            edges.push((r.end, source));
        };
        for w in &spoken.words {
            both(
                RangeUs {
                    start: w.start_us,
                    end: w.end_us,
                },
                Source::Word,
            );
        }
        for r in timeline_pauses(p, speech, PAUSE_US, None) {
            both(r, Source::Pause);
        }
        for r in scene_spans(p, scenes) {
            both(r, Source::Scene);
        }
        let (playhead, selection) = within_video(p, view);
        if let Some(r) = selection {
            both(r, Source::Selection);
        }
        let main = &p.timeline.main;
        let duration = main_track_duration(main);
        let mut clip_edges = clip_starts(main);
        clip_edges.push(duration);
        let closes = (spoken.words.iter())
            .map(|w| w.kind == WordKind::Filler || ends_sentence(spoken.text(w, p, speech)))
            .collect();
        edges.extend(clip_starts(main).into_iter().map(|t| (t, Source::Clip)));
        edges.push((duration, Source::Clip));
        edges.push((playhead, Source::Playhead));
        edges.push((0, Source::Bound));
        edges.push((duration, Source::Bound));
        edges.extend((typed_times(message, duration).into_iter()).map(|t| (t, Source::Typed)));
        edges.sort_unstable();
        edges.dedup();
        let mut changes = scene_changes(p, scenes);
        changes.sort_unstable();
        changes.dedup();
        let mut starts = changes.clone();
        starts.extend(
            (spoken.words.iter())
                .filter(|w| w.kind != WordKind::Pause)
                .map(|w| w.start_us),
        );
        starts.extend(clip_starts(main));
        starts.push(0);
        starts.sort_unstable();
        starts.dedup();
        Self {
            edges,
            changes,
            starts,
            clip_edges,
            closes,
            spoken,
            fps: p.settings.fps,
        }
    }

    /// The snapshot's spoken words, in timeline order.
    pub(crate) fn spoken(&self) -> &Spoken {
        &self.spoken
    }

    /// The edges within `TOLERANCE_US` of `t`.
    fn near(&self, t: Us) -> impl Iterator<Item = &(Us, Source)> {
        let from = (self.edges).partition_point(|&(at, _)| at < t.saturating_sub(TOLERANCE_US));
        (self.edges[from..].iter())
            .take_while(move |&&(at, _)| at <= t.saturating_add(TOLERANCE_US))
    }

    /// The edge nearest `t` within `TOLERANCE_US`, if any.
    pub(crate) fn snap(&self, t: Us) -> Option<(Us, Source)> {
        (self.near(t))
            .min_by_key(|&&(at, source)| (at.abs_diff(t), source))
            .copied()
    }

    /// The nearest of what the person pointed at (the selection's ends, a time they typed, the playhead) within
    /// `TOLERANCE_US` of `t`, outside `cut`.
    fn pointed(&self, t: Us, cut: &[RangeUs]) -> Option<Us> {
        (self.near(t))
            .filter(|&&(at, source)| source <= Source::Playhead && !in_cut(cut, at))
            .min_by_key(|&&(at, source)| (at.abs_diff(t), source))
            .map(|&(at, _)| at)
    }

    /// Where a layer's new start or split at `t` lands (`setLayer`): what the person pointed at (`pointed`), else the
    /// nearest change of shot or topic, else the nearest edge (`snap`), each within `TOLERANCE_US`; None when nothing
    /// is near.
    pub(crate) fn moment(&self, t: Us) -> Option<Us> {
        let from = (self.changes).partition_point(|&at| at < t.saturating_sub(TOLERANCE_US));
        let change = (self.changes[from..].iter())
            .take_while(|&&at| at <= t.saturating_add(TOLERANCE_US))
            .min_by_key(|&&at| at.abs_diff(t))
            .copied();
        (self.pointed(t, &[]))
            .or(change)
            .or_else(|| self.snap(t).map(|(at, _)| at))
    }

    /// Where something placed at `t` lands: what the person pointed at (`pointed`), else the nearest word start or cut
    /// within `PLACE_US` (the earlier on a tie), else `t` as given; neither in `cut`, the footage the answer's removals
    /// take out (where a time left as given may still lie).
    pub(crate) fn place(&self, t: Us, cut: &[RangeUs]) -> Us {
        let pointed = self.pointed(t, cut);
        let from = (self.starts).partition_point(|&at| at < t.saturating_sub(PLACE_US));
        let start = (self.starts[from..].iter())
            .take_while(|&&at| at <= t.saturating_add(PLACE_US))
            .filter(|&&at| !in_cut(cut, at))
            .min_by_key(|&&at| at.abs_diff(t))
            .copied();
        pointed.or(start).unwrap_or(t)
    }

    /// `t` on its nearest edge, or as given when none is near: for a time that needs no evidence (a split).
    pub(crate) fn time(&self, t: Us) -> Us {
        self.snap(t).map_or(t, |(at, _)| at)
    }

    /// `r` as `range` snaps it, for a part to keep (`keepOnly`), its end trimmed of what trails after its last words
    /// (the module says how).
    pub(crate) fn keep_range(&self, r: RangeUs) -> Result<RangeUs, Unfounded> {
        let snapped = self.range(r)?;
        // Only an end the model put on a word or a pause, with nothing else of note near it.
        let by_speech = self
            .snap(r.end)
            .is_some_and(|(_, source)| matches!(source, Source::Word | Source::Pause))
            && self
                .near(r.end)
                .all(|&(_, source)| matches!(source, Source::Word | Source::Pause));
        if !by_speech {
            return Ok(snapped);
        }
        let words = &self.spoken.words;
        let said: Vec<usize> = (0..words.len())
            .filter(|&i| words[i].kind != WordKind::Pause)
            .collect();
        let Some(first) = said.iter().position(|&i| words[i].end_us > snapped.start) else {
            return Ok(snapped);
        };
        let Some(mut last) = said.iter().rposition(|&i| words[i].start_us < snapped.end) else {
            return Ok(snapped);
        };
        if last <= first {
            return Ok(snapped);
        }
        let gap = |k: usize| words[said[k]].start_us - words[said[k - 1]].end_us;
        // Whether speech runs on from the `k`-th word said without a break (`context::BREATH_US`, a sentence's end, a
        // filler, or a clip's edge between).
        let runs_on = |k: usize| {
            k + 1 < said.len() && gap(k + 1) < BREATH_US && !self.closes[said[k]] && {
                let (a, b) = (words[said[k]].end_us, words[said[k + 1]].start_us);
                let at = self.clip_edges.partition_point(|&t| t < a);
                self.clip_edges.get(at).is_none_or(|&t| t > b)
            }
        };
        let mut trimmed = false;
        // One stray word after a long pause, said into the next sentence: it opens what comes next.
        if gap(last) >= TAIL_PAUSE_US && runs_on(last) {
            last -= 1;
            trimmed = true;
        }
        // A long pause the part ends on.
        if trimmed || snapped.end - words[said[last]].end_us >= TAIL_PAUSE_US {
            let padded = word_cut(&self.spoken, said[first], said[last], self.fps);
            return Ok(RangeUs {
                start: snapped.start,
                end: padded.end.min(snapped.end),
            });
        }
        Ok(snapped)
    }

    /// `r` with both edges snapped. An edge padded is one that snapped to a word's own edge on that side: the start of
    /// the range's first word, the end of its last (`word_cut` over the words it holds). Any other edge stays exactly
    /// where it snapped, so padding never carries it to a word further off (in another clip, across a pause).
    pub(crate) fn range(&self, r: RangeUs) -> Result<RangeUs, Unfounded> {
        let (start, from_source) = self.snap(r.start).ok_or(Unfounded::Start)?;
        let (end, to_source) = self.snap(r.end).ok_or(Unfounded::End)?;
        if end <= start {
            return Err(Unfounded::Empty(start));
        }
        let mut out = RangeUs { start, end };
        let words = &self.spoken.words;
        let first = words.partition_point(|w| w.start_us < start);
        let past = words.partition_point(|w| w.end_us <= end);
        if first < past {
            let pad_start = from_source == Source::Word && words[first].start_us == start;
            let pad_end = to_source == Source::Word && words[past - 1].end_us == end;
            if pad_start || pad_end {
                let padded = word_cut(&self.spoken, first, past - 1, self.fps);
                if pad_start {
                    out.start = padded.start;
                }
                if pad_end {
                    out.end = padded.end;
                }
            }
        }
        Ok(out)
    }
}

/// The times the person typed in `message`, sorted, on a main track `duration_us` long. Read leniently but never
/// invented: a message without a time gives none, and neither do counts, refs, speeds or shapes.
/// - A number with a seconds or minutes word is that time ("12.5 s", "12.5s", "2 min", "1.5 minutes"). "m" is not
///   minutes: "5m" may as well be metres or a size. A number word needs its unit ("five seconds").
/// - A bare number is seconds when a time word comes right before it (`TIME_WORDS`: "at", "from", "after" ...: "at 12
///   please", "after 45 because"), when it follows a time just read ("12-15 please", "12 and 15"), or when only a
///   joining word (`JOINING`: "to", "and", "then" ...) or the phrase's end comes next ("12 to 15", "at 12, then").
///   Otherwise, before another word, it is a count ("3 pauses", "3 pauses and 2 ums", "2 clips").
/// - A clock is minutes and two-digit seconds ("1:05", "1:10.5"), or hours too ("1:02:05"). "1:5" and the aspect
///   ratios are none ("16:9" fails the two digits, "9:16" is in `RATIOS`); "16:10" stays a clock.
/// - "first N" gives 0 and N, "last N" the duration less N and the duration, when N has a unit or is a clock ("the
///   first 5 seconds", "the last 10 s", "the last minute"); "the first 3" is a count.
/// - A number touching a letter is none, unless the letters are its unit ("12.5s"): a ref, an ordinal or a speed
///   ("c3", "2nd", "0.5x").
pub fn typed_times(message: &str, duration_us: Us) -> Vec<Us> {
    let tokens = tokens(message);
    let mut out = Vec::new();
    // Which tokens were read as times, so a number right after one is a time too ("12-15", "12 and 15").
    let mut read = vec![false; tokens.len()];
    for (k, token) in tokens.iter().enumerate() {
        let before = k.checked_sub(1).map(|j| &tokens[j]);
        let after = tokens.get(k + 1);
        let side = before.filter(|_| !token.joined).and_then(Token::side);
        let unit = after.and_then(Token::unit);
        let amount = match &token.kind {
            Kind::Number(_) | Kind::Clock(_) if touches_letters(token, before, after) => continue,
            Kind::Number(n) => match unit {
                Some(unit) => n * unit,
                None if side.is_some() => continue,
                None if introduced(k, &tokens, &read) => *n,
                None if before_a_word(token, after) => continue,
                None => *n,
            },
            Kind::Clock(t) => *t,
            Kind::NotTime => continue,
            Kind::Word(w) => match (number_word(w), unit) {
                (Some(n), Some(unit)) => n * unit,
                (Some(_), None) => continue,
                // "the last minute": first or last right before the unit.
                (None, _) => match token.unit() {
                    Some(unit) if side.is_some() => unit,
                    _ => continue,
                },
            },
        };
        if !amount.is_finite() || amount < 0.0 {
            continue;
        }
        read[k] = true;
        let amount_us = secs_to_us(amount.min((MAX_TIME_US / US_PER_SEC) as f64));
        match side {
            Some(Side::First) => out.extend([0, amount_us]),
            Some(Side::Last) => {
                out.extend([duration_us.saturating_sub(amount_us).max(0), duration_us])
            }
            None => out.push(amount_us),
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Words that bring in a time, so a bare number right after one is a time whatever follows it. Not "and" or "or":
/// "3 pauses and 2 ums" are counts.
const TIME_WORDS: [&str; 12] = [
    "at", "from", "to", "until", "till", "through", "thru", "after", "before", "between", "around",
    "since",
];

/// Words that join a time to the next, so a bare number before one is still a time.
const JOINING: [&str; 8] = [
    "to", "and", "or", "until", "till", "through", "thru", "then",
];

/// Aspect ratios that pass for a clock (two-digit seconds): only 9:16. The others are no clock already ("16:9",
/// "4:3", "21:9", "1:1" have one-digit seconds). "16:10" stays a clock, 16 minutes 10 seconds, as people write times.
const RATIOS: [(&str, &str); 1] = [("9", "16")];

/// Characters that end a phrase: a bare number before one is a time whatever word comes next.
const PHRASE_ENDS: [char; 6] = ['.', ',', ';', ':', '!', '?'];

/// A number or clock with letters right before it ("c3") or right after that aren't its unit ("2nd", "0.5x").
fn touches_letters(token: &Token, before: Option<&Token>, after: Option<&Token>) -> bool {
    let glued_before = token.joined && before.is_some();
    let glued_after = after.is_some_and(|a| a.joined && a.unit().is_none());
    glued_before || glued_after
}

/// Whether the bare number at `k` is brought in as a time: by a time word right before it (with a space between), or by
/// a time read right before it, directly ("12-15") or across "and" or "or" ("12 and 15").
fn introduced(k: usize, tokens: &[Token], read: &[bool]) -> bool {
    let word_is = |j: usize, words: &[&str]| matches!(&tokens[j].kind, Kind::Word(w) if words.contains(&w.as_str()));
    let Some(j) = k.checked_sub(1) else {
        return false;
    };
    (!tokens[k].joined && word_is(j, &TIME_WORDS))
        || read[j]
        || (word_is(j, &["and", "or"]) && j.checked_sub(1).is_some_and(|i| read[i]))
}

/// A bare number with a word after it, in the same phrase, that neither joins times nor is a unit: a count.
fn before_a_word(token: &Token, after: Option<&Token>) -> bool {
    !token.ends_phrase
        && after.is_some_and(|a| match &a.kind {
            Kind::Word(w) => !JOINING.contains(&w.as_str()) && a.unit().is_none(),
            _ => false,
        })
}

/// One piece of a message.
#[derive(Clone, Debug, PartialEq)]
struct Token {
    kind: Kind,
    /// Nothing between it and the piece before ("c" and "3" in "c3", "12.5" and "s" in "12.5s").
    joined: bool,
    /// A phrase ends right after it (`PHRASE_ENDS`).
    ends_phrase: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// Digits, with a decimal part or not.
    Number(f64),
    /// "1:05" in seconds.
    Clock(f64),
    /// Digits and colons that are no clock: "9:16", "1:5".
    NotTime,
    /// Letters, lowercase.
    Word(String),
}

#[derive(Clone, Copy)]
enum Side {
    First,
    Last,
}

impl Token {
    /// Seconds a unit word stands for.
    fn unit(&self) -> Option<f64> {
        let Kind::Word(w) = &self.kind else {
            return None;
        };
        match w.as_str() {
            "s" | "sec" | "secs" | "second" | "seconds" => Some(1.0),
            "min" | "mins" | "minute" | "minutes" => Some(60.0),
            _ => None,
        }
    }

    fn side(&self) -> Option<Side> {
        match &self.kind {
            Kind::Word(w) if w == "first" => Some(Side::First),
            Kind::Word(w) if w == "last" => Some(Side::Last),
            _ => None,
        }
    }
}

fn number_word(w: &str) -> Option<f64> {
    const WORDS: [&str; 21] = [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    const TENS: [&str; 4] = ["thirty", "forty", "fifty", "sixty"];
    if let Some(n) = WORDS.iter().position(|&x| x == w) {
        return Some(n as f64);
    }
    TENS.iter()
        .position(|&x| x == w)
        .map(|n| (n as f64 + 3.0) * 10.0)
}

/// Digit groups joined by ':' as a number, a clock (two-digit groups after the first, not an aspect ratio) or neither.
fn number_or_clock(groups: &[String]) -> Kind {
    let Some(values) = groups
        .iter()
        .map(|g| g.parse().ok())
        .collect::<Option<Vec<f64>>>()
    else {
        return Kind::NotTime;
    };
    let two_digits =
        (groups.iter().skip(1)).all(|g| g.split('.').next().is_some_and(|d| d.len() == 2));
    let ratio = |a: &str, b: &str| RATIOS.contains(&(a, b));
    match values.as_slice() {
        [n] => Kind::Number(*n),
        _ if !two_digits => Kind::NotTime,
        [_, _] if ratio(&groups[0], &groups[1]) => Kind::NotTime,
        [m, s] => Kind::Clock(m * 60.0 + s),
        [h, m, s] => Kind::Clock(h * 3600.0 + m * 60.0 + s),
        _ => Kind::NotTime,
    }
}

/// `message` as numbers, clocks and words; everything else separates them.
fn tokens(message: &str) -> Vec<Token> {
    let chars: Vec<char> = message.to_lowercase().chars().collect();
    let mut out: Vec<Token> = Vec::new();
    // Where the last piece ended.
    let mut last_end = None;
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let c = chars[i];
        let kind = if c.is_ascii_digit() {
            // Groups of digits joined by ':' (a clock), the last with a decimal part.
            let mut groups: Vec<String> = Vec::new();
            let mut group = String::new();
            while i < chars.len() {
                let c = chars[i];
                let next_digit = chars.get(i + 1).is_some_and(char::is_ascii_digit);
                if c.is_ascii_digit() {
                    group.push(c);
                } else if c == ':' && next_digit && !group.contains('.') {
                    groups.push(std::mem::take(&mut group));
                } else if c == '.' && next_digit && !group.contains('.') {
                    group.push(c);
                } else {
                    break;
                }
                i += 1;
            }
            groups.push(group);
            number_or_clock(&groups)
        } else if c.is_alphabetic() {
            let mut word = String::new();
            while i < chars.len() && chars[i].is_alphabetic() {
                word.push(chars[i]);
                i += 1;
            }
            Kind::Word(word)
        } else {
            if PHRASE_ENDS.contains(&c)
                && let Some(last) = out.last_mut()
            {
                last.ends_phrase = true;
            }
            i += 1;
            continue;
        };
        out.push(Token {
            kind,
            joined: last_end == Some(start),
            ends_phrase: false,
        });
        last_end = Some(i);
    }
    out
}
