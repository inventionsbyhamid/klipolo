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
//! A moment the media tools place something at (`moment`: a sound, a title, a sticker) stands on the same evidence,
//! but lands on a change of shot or topic within the tolerance before anything heard or seen there (a whoosh "at every
//! topic change" plays on the change, not on the word beside it); what the person pointed at still comes first.
//!
//! Times are the snapshot's timeline, as the context showed them; the compiler maps them into its working copy after.
//! Building costs O(words + pauses + scene items + clips) once per message; a lookup is a binary search.

use std::collections::HashMap;

use klipolo_model::Project;
use klipolo_model::query::{clip_starts, main_track_duration};
use klipolo_model::speech::silence::timeline_pauses;
use klipolo_model::speech::{PAUSE_US, Speech, Spoken, spoken, word_cut};
use klipolo_model::time::{MAX_TIME_US, RangeUs, US_PER_SEC, Us, secs_to_us};

use crate::context::{SceneLog, View, scene_changes, scene_spans, within_video};

/// How far a model's time may lie from what it was shown.
pub const TOLERANCE_US: Us = 300_000;

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

/// Every edge the model may name in one message's snapshot.
pub(crate) struct Evidence {
    /// Sorted by time, then by `Source`.
    edges: Vec<(Us, Source)>,
    /// Where a change of shot or topic starts (`context::scene_changes`), sorted.
    changes: Vec<Us>,
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
        Self {
            edges,
            changes,
            spoken,
            fps: p.settings.fps,
        }
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

    /// Where something placed at `t` lands: the nearest of what the person pointed at (the selection's ends, a time
    /// they typed, the playhead), else the nearest change of shot or topic, else the nearest edge (`snap`), each within
    /// `TOLERANCE_US`; None when nothing is near.
    pub(crate) fn moment(&self, t: Us) -> Option<Us> {
        let pointed = (self.near(t))
            .filter(|(_, source)| *source <= Source::Playhead)
            .min_by_key(|&&(at, source)| (at.abs_diff(t), source))
            .map(|&(at, _)| at);
        let from = (self.changes).partition_point(|&at| at < t.saturating_sub(TOLERANCE_US));
        let change = (self.changes[from..].iter())
            .take_while(|&&at| at <= t.saturating_add(TOLERANCE_US))
            .min_by_key(|&&at| at.abs_diff(t))
            .copied();
        pointed
            .or(change)
            .or_else(|| self.snap(t).map(|(at, _)| at))
    }

    /// `t` on its nearest edge, or as given when none is near: for a time that needs no evidence (a split).
    pub(crate) fn time(&self, t: Us) -> Us {
        self.snap(t).map_or(t, |(at, _)| at)
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
