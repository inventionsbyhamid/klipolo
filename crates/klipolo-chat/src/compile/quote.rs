//! Moments named by what is said there (`atWords`): a quote of a few words in a row, as Transcript shows them, found in
//! the snapshot's spoken words (fixes applied, pause words left out). The moment is where the quote's first word
//! starts on the snapshot's timeline; the compiler maps it into its working copy as it maps any time (`map`), so a cut
//! or a split earlier in the answer never moves it off its word.
//!
//! Words compare bare (`bare`: punctuation and spaces out, in lower case, a script's marks kept), so the case, the
//! punctuation, a pause mark and a speaker label the model copies with them don't count. A quote no run of words
//! matches is tried again on the words' letters joined: across whole words for scripts written with spaces, and from
//! inside a word for those written without (`unspaced`), placed where that word starts.
//!
//! A quote said in one place is that place. Said in several, it is the one nearest a time given beside it (`near`);
//! with none, it is refused back with each place, those nearest the playhead first, for the model to quote words said
//! once or give a time too. One never said is refused with the places said most like it. Places the answer's own
//! removals take out (`Compiler::cut`) don't count while another place is left. Refusals write the words as they are,
//! only a double quote escaped, so a script's marks come back as the model can copy them.
//!
//! Costs: O(words) to build once per message, when a call first quotes; O(words x quoted words) per quote.

use klipolo_model::Project;
use klipolo_model::speech::{Speech, Spoken, WordKind};
use klipolo_model::time::{RangeUs, Us};

use super::{Refused, to_model};
use crate::context::secs;
use crate::evidence::in_cut;

/// The most places a refusal lists, then how many more there are.
const LISTED: usize = 3;
/// How many words after a place a refusal shows, so the words around it can be seen.
const MORE_WORDS: usize = 2;
/// The most words a refusal shows of one place.
const EXCERPT_WORDS: usize = 6;
/// The most words and characters a refusal echoes of the model's quote, then `...`.
const ECHO_WORDS: usize = 8;
const ECHO_CHARS: usize = 60;
/// Punctuation outside ASCII and the general punctuation blocks, left out of a bare word.
const PUNCTUATION: [char; 10] = ['।', '॥', '«', '»', '¿', '¡', '·', '؟', '،', '؛'];

/// One word as Transcript shows it: a word whose text holds spaces (a fix of several words) is one per part.
struct Heard {
    bare: String,
    shown: String,
    start_us: Us,
}

/// The snapshot's spoken words, in timeline order, for finding quotes.
pub(super) struct Said {
    words: Vec<Heard>,
}

/// Whether `c` stays in a bare word: a letter or digit, or a script's mark (Hindi's vowel signs and virama, which are
/// neither), but no space or punctuation.
fn kept(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric();
    }
    c.is_alphanumeric()
        || !(c.is_whitespace()
            || ('\u{2000}'..='\u{206F}').contains(&c)
            || ('\u{3000}'..='\u{303F}').contains(&c)
            || ('\u{FF00}'..='\u{FF0F}').contains(&c)
            || ('\u{FF1A}'..='\u{FF20}').contains(&c)
            || PUNCTUATION.contains(&c))
}

/// `text` without its spaces and punctuation, in lower case.
fn bare(text: &str) -> String {
    (text.chars())
        .filter(|&c| kept(c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether `c` is of a script written without spaces between words (Chinese, Japanese, Thai, Lao, Khmer, Burmese).
fn unspaced(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30FF}' | '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}'
        | '\u{0E00}'..='\u{0EFF}' | '\u{1780}'..='\u{17FF}' | '\u{1000}'..='\u{109F}')
}

/// Whether `token` is a speaker label as Transcript writes one before a line's words: `A:`, `AB:`, `P1:`, `P12:`.
fn speaker_label(token: &str) -> bool {
    token.strip_suffix(':').is_some_and(|label| {
        let mut chars = label.chars();
        let first = chars.next().is_some_and(|c| c.is_ascii_uppercase());
        let rest: Vec<char> = chars.collect();
        first
            && rest.len() <= 2
            && ((rest.len() <= 1 && rest.iter().all(char::is_ascii_uppercase))
                || rest.iter().all(char::is_ascii_digit))
    })
}

/// A quote's words, bare: a note in brackets (`[pause 1.5 s]`) left out, a speaker label copied before them (`A:`), and
/// a word with no letter or digit.
fn quoted(quote: &str) -> Vec<String> {
    let mut text = String::with_capacity(quote.len());
    let mut depth = 0usize;
    for ch in quote.chars() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => text.push(ch),
            _ => {}
        }
    }
    let mut tokens = text.split_whitespace().peekable();
    tokens.next_if(|t| speaker_label(t));
    tokens.map(bare).filter(|w| !w.is_empty()).collect()
}

/// `text` in double quotes as a refusal writes it, on one line: only a double quote escaped, so the words come back as
/// written (`{:?}` would escape a script's marks, and the model would copy the escapes).
fn in_quotes(text: &str) -> String {
    let one_line: Vec<&str> = text.split_whitespace().collect();
    format!("\"{}\"", one_line.join(" ").replace('"', "\\\""))
}

/// The model's quote as a refusal echoes it: at most `ECHO_WORDS` words and `ECHO_CHARS` characters, then `...`.
pub(super) fn echo(quote: &str) -> String {
    let words: Vec<&str> = quote.split_whitespace().collect();
    let mut out = words[..words.len().min(ECHO_WORDS)].join(" ");
    let mut cut = words.len() > ECHO_WORDS;
    if out.chars().count() > ECHO_CHARS {
        out = out.chars().take(ECHO_CHARS).collect();
        cut = true;
    }
    if cut {
        out.push_str("...");
    }
    in_quotes(&out)
}

/// How alike two bare words are, for the places said most like a quote: the same word, one the start of the other, the
/// same first three letters, or not alike (0).
fn alike(a: &str, b: &str) -> u32 {
    let short = a.chars().count().min(b.chars().count());
    if a == b {
        3
    } else if short >= 3 && (a.starts_with(b) || b.starts_with(a)) {
        2
    } else if short >= 3 && a.chars().take(3).eq(b.chars().take(3)) {
        1
    } else {
        0
    }
}

impl Said {
    /// The words of `spoken`, the main track of `p` with `speech`, as Transcript shows them.
    pub(super) fn new(p: &Project, speech: &Speech, spoken: &Spoken) -> Self {
        let mut words = Vec::with_capacity(spoken.words.len());
        for (w, text) in spoken.shown(p, speech) {
            if w.kind == WordKind::Pause {
                continue;
            }
            for part in text.split_whitespace() {
                let b = bare(part);
                if !b.is_empty() {
                    words.push(Heard {
                        bare: b,
                        shown: part.to_owned(),
                        start_us: w.start_us,
                    });
                }
            }
        }
        Self { words }
    }

    /// Every place `q` (bare words) is said: its first word and how many words it covers. A run of words first; else
    /// the quote's letters in the words' letters joined, over whole words, or for a script written without spaces from
    /// anywhere in a word.
    fn places(&self, q: &[String]) -> Vec<(usize, usize)> {
        let (words, n) = (&self.words, q.len());
        if n == 0 || n > words.len() {
            return Vec::new();
        }
        let rows: Vec<(usize, usize)> = (0..=words.len() - n)
            .filter(|&i| (words[i..i + n].iter().zip(q)).all(|(w, x)| w.bare == *x))
            .map(|i| (i, n))
            .collect();
        if !rows.is_empty() {
            return rows;
        }
        let letters = q.concat();
        let inside = letters.chars().any(unspaced);
        let mut joined = String::new();
        let mut starts = Vec::with_capacity(words.len());
        for w in words {
            starts.push(joined.len());
            joined.push_str(&w.bare);
        }
        let mut out = Vec::new();
        for (at, _) in joined.match_indices(&letters) {
            let end = at + letters.len();
            let first = starts.partition_point(|&s| s <= at) - 1;
            let past = starts.partition_point(|&s| s < end);
            let whole = starts[first] == at && (past == words.len() || starts[past] == end);
            if whole || inside {
                out.push((first, past - first));
            }
        }
        out
    }

    /// Up to `LISTED` places said most like `q`, best first (the earlier on a tie), none overlapping another. A word on
    /// its own may be one letter off (`alike` 1).
    fn nearest(&self, q: &[String]) -> Vec<usize> {
        let words = &self.words;
        let n = q.len().max(1);
        let least = if q.len() == 1 { 1 } else { 2 };
        let mut scored: Vec<(u32, usize)> = (0..words.len())
            .map(|i| {
                let score = (q.iter().zip(&words[i..]))
                    .map(|(x, w)| alike(x, &w.bare))
                    .sum::<u32>();
                (score, i)
            })
            .filter(|&(score, _)| score >= least)
            .collect();
        scored.sort_by_key(|&(score, i)| (std::cmp::Reverse(score), i));
        let mut out: Vec<usize> = Vec::new();
        for (_, i) in scored {
            if out.iter().all(|&j| i.abs_diff(j) >= n) {
                out.push(i);
                if out.len() == LISTED {
                    break;
                }
            }
        }
        out
    }

    /// `count` words from word `i` (at most `EXCERPT_WORDS`) as Transcript shows them, quoted, and where they start:
    /// `"we went flying" (10.3 s)`.
    fn excerpt(&self, i: usize, count: usize) -> String {
        let end = (i + count.min(EXCERPT_WORDS)).min(self.words.len());
        let shown: Vec<&str> = (self.words[i..end].iter())
            .map(|w| w.shown.as_str())
            .collect();
        format!(
            "{} ({} s)",
            in_quotes(&shown.join(" ")),
            secs(self.words[i].start_us)
        )
    }

    /// Up to `LISTED` excerpts of `count` words, joined, with how many more there are.
    fn listed(&self, places: &[usize], count: usize) -> String {
        let mut out: Vec<String> = (places.iter().take(LISTED))
            .map(|&i| self.excerpt(i, count))
            .collect();
        if places.len() > LISTED {
            out.push(format!("and {} more", places.len() - LISTED));
        }
        out.join(", ")
    }

    /// Where `quote` is said on the snapshot's timeline (its first word's start), for `tool`'s `field`: its one place
    /// outside `cut` (the snapshot footage the answer's removals take out), or of several the one nearest a time in
    /// `near` (the times given beside it), or why not, for the model. A refusal lists several places nearest `playhead`
    /// first.
    pub(super) fn find(
        &self,
        cut: &[RangeUs],
        (tool, field): (&str, &str),
        quote: &str,
        (near, playhead): (&[Us], Us),
    ) -> Result<Us, Refused> {
        let q = quoted(quote);
        let said = echo(quote);
        if q.is_empty() {
            return Err(to_model(format!(
                "{tool}: {field} {said} holds no words. Quote 1 to 4 words exactly as Transcript shows them."
            )));
        }
        let places = self.places(&q);
        if places.is_empty() {
            let near = self.nearest(&q);
            let like = if near.is_empty() {
                String::new()
            } else {
                format!(" Said most like it: {}.", self.listed(&near, q.len()))
            };
            return Err(to_model(format!(
                "{tool}: {field} {said} is not said in the video.{like} Quote 1 to 4 words exactly as Transcript \
                 shows them."
            )));
        }
        let start = |&(i, _): &(usize, usize)| self.words[i].start_us;
        let kept: Vec<(usize, usize)> = (places.iter().copied())
            .filter(|p| !in_cut(cut, start(p)))
            .collect();
        let off = |p: &(usize, usize), times: &[Us]| {
            (times.iter().map(|&t| start(p).abs_diff(t)).min()).unwrap_or(0)
        };
        match kept.as_slice() {
            [one] => Ok(start(one)),
            [] => {
                let all: Vec<usize> = places.iter().map(|&(i, _)| i).collect();
                Err(to_model(format!(
                    "{tool}: {field} {said} is said only in parts this answer cuts: {}. Quote words in the parts you \
                     keep.",
                    self.listed(&all, places[0].1)
                )))
            }
            several if !near.is_empty() => {
                let best = (several.iter()).min_by_key(|p| (off(p, near), start(p)));
                Ok(best.map_or(0, start))
            }
            several => {
                let mut by_playhead = several.to_vec();
                by_playhead.sort_by_key(|p| (off(p, &[playhead]), start(p)));
                let at: Vec<usize> = by_playhead.iter().map(|&(i, _)| i).collect();
                Err(to_model(format!(
                    "{tool}: {field} {said} is said {} times: {}. Quote words said only once, or give atSec too to \
                     pick one.",
                    several.len(),
                    self.listed(&at, several[0].1 + MORE_WORDS)
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(words: &[(&str, f64)]) -> Said {
        Said {
            words: (words.iter())
                .map(|&(text, at)| Heard {
                    bare: bare(text),
                    shown: text.to_owned(),
                    start_us: (at * 1e6).round() as Us,
                })
                .collect(),
        }
    }

    /// "So we went flying today. We went home." with each word's start.
    fn flight() -> Said {
        said(&[
            ("So", 9.85),
            ("we", 10.3),
            ("went", 10.6),
            ("flying", 12.0),
            ("today.", 12.5),
            ("We", 14.0),
            ("went", 14.2),
            ("home.", 14.5),
        ])
    }

    fn found(s: &Said, quote: &str) -> Result<Us, String> {
        s.find(&[], ("addSound", "atWords"), quote, (&[], 0))
            .map_err(|r| r.reason)
    }

    #[test]
    fn a_quote_is_its_first_words_start_whatever_its_case_and_punctuation() {
        let s = flight();
        assert_eq!(found(&s, "went flying"), Ok(10_600_000));
        assert_eq!(found(&s, "Flying, TODAY"), Ok(12_000_000));
        assert_eq!(found(&s, "flying today."), Ok(12_000_000));
        // A pause mark or a speaker label copied with the words doesn't count.
        assert_eq!(found(&s, "flying [pause 1.5 s] today"), Ok(12_000_000));
        assert_eq!(found(&s, "B: went flying"), Ok(10_600_000));
        assert_eq!(found(&s, "P1: went flying"), Ok(10_600_000));
        assert_eq!(found(&s, "P12: went flying"), Ok(10_600_000));
        assert_eq!(found(&s, "home"), Ok(14_500_000));
        assert!(speaker_label("AB:") && !speaker_label("P123:") && !speaker_label("ABC:"));
    }

    #[test]
    fn a_quote_said_in_several_places_is_the_one_nearest_a_time_given_or_asks_for_one() {
        let s = flight();
        let at = |near: &[Us]| {
            s.find(&[], ("addText", "atWords"), "we went", (near, 0))
                .ok()
        };
        assert_eq!(at(&[13_000_000]), Some(14_000_000));
        assert_eq!(at(&[9_000_000]), Some(10_300_000));
        // With several times (a sound's list), the place nearest any of them.
        assert_eq!(at(&[2_000_000, 14_100_000]), Some(14_000_000));
        let reason = found(&s, "we went").unwrap_err();
        assert_eq!(
            reason,
            "addSound: atWords \"we went\" is said 2 times: \"we went flying today.\" (10.3 s), \"We went home.\" \
             (14.0 s). Quote words said only once, or give atSec too to pick one."
        );
        // The places nearest the playhead come first.
        let reason = (s.find(&[], ("addSound", "atWords"), "we went", (&[], 15_000_000)))
            .unwrap_err()
            .reason;
        assert!(
            reason.contains(
                "2 times: \"We went home.\" (14.0 s), \"we went flying today.\" (10.3 s)."
            ),
            "{reason}"
        );
    }

    #[test]
    fn a_quote_never_said_names_the_places_said_most_like_it() {
        let reason = found(&flight(), "went flyin").unwrap_err();
        assert_eq!(
            reason,
            "addSound: atWords \"went flyin\" is not said in the video. Said most like it: \"went flying\" (10.6 s), \
             \"went home.\" (14.2 s). Quote 1 to 4 words exactly as Transcript shows them."
        );
        let reason = found(&flight(), "zebra").unwrap_err();
        assert_eq!(
            reason,
            "addSound: atWords \"zebra\" is not said in the video. Quote 1 to 4 words exactly as Transcript shows them."
        );
        // A word on its own one letter off still gets a hint.
        let reason = found(&flight(), "flyng").unwrap_err();
        assert!(
            reason.contains("Said most like it: \"flying\" (12.0 s)."),
            "{reason}"
        );
        assert!(
            found(&flight(), "...")
                .unwrap_err()
                .contains("holds no words")
        );
    }

    #[test]
    fn a_refusal_stays_short() {
        let long = "so we went flying today and then we came home again in the evening after dinner with friends";
        let reason = found(&flight(), long).unwrap_err();
        assert!(
            reason.starts_with(
                "addSound: atWords \"so we went flying today and then we...\" is not said"
            ),
            "{reason}"
        );
        // A place shows at most six words.
        let many = said(&[
            ("one", 1.0),
            ("two", 2.0),
            ("three", 3.0),
            ("four", 4.0),
            ("five", 5.0),
            ("six", 6.0),
            ("seven", 7.0),
            ("one", 8.0),
            ("two", 9.0),
            ("three", 10.0),
            ("four", 11.0),
            ("five", 12.0),
            ("six", 13.0),
            ("eight", 14.0),
        ]);
        let reason = found(&many, "one two three four five").unwrap_err();
        assert!(
            reason.contains(
                "\"one two three four five six\" (1.0 s), \"one two three four five six\" (8.0 s)."
            ),
            "{reason}"
        );
    }

    #[test]
    fn a_place_the_answer_cuts_does_not_count() {
        let s = flight();
        let cut = [RangeUs {
            start: 13_500_000,
            end: 16_000_000,
        }];
        let at = |quote: &str| s.find(&cut, ("addImage", "atWords"), quote, (&[], 0));
        assert_eq!(at("we went").ok(), Some(10_300_000));
        assert_eq!(
            at("home").unwrap_err().reason,
            "addImage: atWords \"home\" is said only in parts this answer cuts: \"home.\" (14.5 s). Quote words in \
             the parts you keep."
        );
    }

    #[test]
    fn hindi_words_come_back_as_written() {
        // "नमस्ते दुनिया, आज हम उड़ेंगे।" ("Hello world, today we fly."): vowel signs and viramas are marks, kept in the
        // words and never escaped in a refusal.
        let s = said(&[
            ("नमस्ते", 1.0),
            ("दुनिया,", 1.6),
            ("आज", 2.4),
            ("हम", 2.8),
            ("उड़ेंगे।", 3.1),
        ]);
        assert_eq!(found(&s, "आज हम उड़ेंगे"), Ok(2_400_000));
        assert_eq!(found(&s, "दुनिया"), Ok(1_600_000));
        // A vowel sign apart is another word.
        assert!(found(&s, "दनिया").is_err());
        let reason = found(&s, "हम उड़ेगे").unwrap_err();
        assert!(!reason.contains("\\u"), "{reason}");
        assert!(reason.contains("\"हम उड़ेगे\""), "{reason}");
        assert!(
            reason.contains("Said most like it: \"हम उड़ेंगे।\" (2.8 s)."),
            "{reason}"
        );
    }

    #[test]
    fn words_written_without_spaces_are_found_across_and_inside_words() {
        let s = said(&[("今日は", 1.0), ("いい", 1.5), ("天気", 1.8), ("です", 2.2)]);
        assert_eq!(found(&s, "いい天気"), Ok(1_500_000));
        assert_eq!(found(&s, "今日はいい天気です"), Ok(1_000_000));
        // From inside a word: placed where that word starts.
        assert_eq!(found(&s, "日はいい"), Ok(1_000_000));
        assert_eq!(found(&s, "気で"), Ok(1_800_000));
        // Never from inside a word written with spaces.
        assert!(found(&flight(), "ying today").is_err());
    }
}
