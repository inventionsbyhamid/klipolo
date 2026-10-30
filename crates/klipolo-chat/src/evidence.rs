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
//! Times are the snapshot's timeline, as the context showed them; the compiler maps them into its working copy after.
//! Building costs O(words + pauses + scene items + clips) once per message; a lookup is a binary search.

use klipolo_model::Project;
use klipolo_model::query::{clip_starts, main_track_duration};
use klipolo_model::speech::silence::timeline_pauses;
use klipolo_model::speech::{PAUSE_US, Spoken, spoken, word_cut};
use klipolo_model::time::{MAX_TIME_US, RangeUs, US_PER_SEC, Us, secs_to_us};

use crate::compile::Inputs;
use crate::context::{scene_spans, within_video};

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
    spoken: Spoken,
    fps: u32,
}

impl Evidence {
    pub(crate) fn new(p: &Project, inputs: &Inputs) -> Self {
        let speech = &inputs.speech;
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
        for r in scene_spans(p, &inputs.scenes) {
            both(r, Source::Scene);
        }
        let (playhead, selection) = within_video(p, &inputs.view);
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
        edges.extend(
            (typed_times(&inputs.message, duration).into_iter()).map(|t| (t, Source::Typed)),
        );
        edges.sort_unstable();
        edges.dedup();
        Self {
            edges,
            spoken,
            fps: p.settings.fps,
        }
    }

    /// The edge nearest `t` within `TOLERANCE_US`, if any.
    pub(crate) fn snap(&self, t: Us) -> Option<(Us, Source)> {
        let from = (self.edges).partition_point(|&(at, _)| at < t.saturating_sub(TOLERANCE_US));
        (self.edges[from..].iter())
            .take_while(|&&(at, _)| at <= t.saturating_add(TOLERANCE_US))
            .min_by_key(|&&(at, source)| (at.abs_diff(t), source))
            .copied()
    }

    /// `t` on its nearest edge, or as given when none is near: for a time that needs no evidence (a split).
    pub(crate) fn time(&self, t: Us) -> Us {
        self.snap(t).map_or(t, |(at, _)| at)
    }

    /// `r` with both edges snapped, and each one on a word padded (`word_cut` over the words it holds).
    pub(crate) fn range(&self, r: RangeUs) -> Result<RangeUs, Unfounded> {
        let (start, from_source) = self.snap(r.start).ok_or(Unfounded::Start)?;
        let (end, to_source) = self.snap(r.end).ok_or(Unfounded::End)?;
        if end <= start {
            return Err(Unfounded::Empty(start));
        }
        let mut out = RangeUs { start, end };
        if from_source == Source::Word || to_source == Source::Word {
            let words = &self.spoken.words;
            let first = words.partition_point(|w| w.start_us < start);
            let past = words.partition_point(|w| w.end_us <= end);
            if first < past {
                let padded = word_cut(&self.spoken, first, past - 1, self.fps);
                if from_source == Source::Word {
                    out.start = padded.start;
                }
                if to_source == Source::Word {
                    out.end = padded.end;
                }
            }
        }
        Ok(out)
    }
}

/// The times the person typed in `message`, sorted, on a main track `duration_us` long. Read leniently, never
/// invented: a message without a time gives none.
/// - A number is seconds ("12", "12.5 s", "12.5s"), or minutes with a unit ("2 min", "1.5 minutes").
/// - A clock is minutes and seconds ("1:05", "1:10.5"), or hours too ("1:02:05").
/// - "first N" gives 0 and N, "last N" the duration less N and the duration ("the last 10 s"); "the last minute" is
///   one. N may be a word from one to twenty, or thirty to sixty by tens, when a unit or first/last goes with it.
pub fn typed_times(message: &str, duration_us: Us) -> Vec<Us> {
    let tokens = tokens(message);
    let mut out = Vec::new();
    for (k, token) in tokens.iter().enumerate() {
        let next_unit = tokens.get(k + 1).and_then(Token::unit);
        let before = k.checked_sub(1).map(|j| &tokens[j]);
        let side = before.and_then(Token::side);
        let amount = match token {
            Token::Number(n) => Some(n * next_unit.unwrap_or(1.0)),
            Token::Clock(n) => Some(*n),
            Token::Word(w) => match (number_word(w), next_unit) {
                (Some(n), Some(unit)) => Some(n * unit),
                (Some(n), None) if side.is_some() => Some(n),
                // "the last minute": first or last right before the unit.
                (None, _) => token.unit().filter(|_| side.is_some()),
                (Some(_), None) => None,
            },
        };
        let Some(amount) = amount.filter(|a| a.is_finite() && *a >= 0.0) else {
            continue;
        };
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

#[derive(Clone, Debug, PartialEq)]
enum Token {
    /// Digits, with a decimal part or not.
    Number(f64),
    /// "1:05" in seconds.
    Clock(f64),
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
        let Token::Word(w) = self else { return None };
        match w.as_str() {
            "s" | "sec" | "secs" | "second" | "seconds" => Some(1.0),
            "m" | "min" | "mins" | "minute" | "minutes" => Some(60.0),
            _ => None,
        }
    }

    fn side(&self) -> Option<Side> {
        match self {
            Token::Word(w) if w == "first" => Some(Side::First),
            Token::Word(w) if w == "last" => Some(Side::Last),
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

/// `message` as numbers, clocks and words; everything else separates them.
fn tokens(message: &str) -> Vec<Token> {
    let chars: Vec<char> = message.to_lowercase().chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
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
            let values: Vec<f64> = groups.iter().filter_map(|g| g.parse().ok()).collect();
            match values.as_slice() {
                [n] => out.push(Token::Number(*n)),
                [m, s] => out.push(Token::Clock(m * 60.0 + s)),
                [h, m, s] => out.push(Token::Clock(h * 3600.0 + m * 60.0 + s)),
                _ => {}
            }
        } else if c.is_alphabetic() {
            let mut word = String::new();
            while i < chars.len() && chars[i].is_alphabetic() {
                word.push(chars[i]);
                i += 1;
            }
            out.push(Token::Word(word));
        } else {
            i += 1;
        }
    }
    out
}
