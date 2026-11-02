//! Moments named by what is said there (`atWords`): a quote of a few words in a row, as Transcript shows them, found in
//! the snapshot's spoken words (fixes applied, pause words left out). The moment is where the quote's first word
//! starts on the snapshot's timeline; the compiler maps it into its working copy as it maps any time (`map`), so a cut
//! or a split earlier in the answer never moves it off its word.
//!
//! Words compare bare (`bare`: letters and digits only, in lower case), so the case, the punctuation and a pause mark
//! the model copies with them don't count. A quote no run of words matches is tried again letter by letter across
//! whole words, for scripts written without spaces.
//!
//! A quote said in one place is that place. One said in several is refused back with each place and the words after
//! it, for the model to quote more; one never said, with the places said most like it. Places the answer's own
//! removals take out (`Compiler::cut`) don't count while another place is left.
//!
//! Costs: O(words) to build once per message, when a call first quotes; O(words x quoted words) per quote.

use klipolo_model::Project;
use klipolo_model::speech::{Speech, Spoken, WordKind};
use klipolo_model::time::{RangeUs, Us};

use super::{Refused, to_model};
use crate::context::secs;

/// The most places a refusal lists, then how many more there are.
const LISTED: usize = 3;
/// How many words after a place a refusal shows, so a longer quote can be copied from it.
const MORE_WORDS: usize = 2;

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

/// `text` with only its letters and digits, in lower case.
fn bare(text: &str) -> String {
    (text.chars())
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether `token` is a speaker label as Transcript writes one before a line's words (`A:`).
fn speaker_label(token: &str) -> bool {
    token.strip_suffix(':').is_some_and(|label| {
        (1..=2).contains(&label.chars().count()) && label.chars().all(|c| c.is_ascii_uppercase())
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

    /// Every place `q` (bare words) is said: its first word and how many words it covers. A run of words first; else the
    /// quote's letters over whole words in a row.
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
        let mut out = Vec::new();
        for i in 0..words.len() {
            let (mut rest, mut j) = (letters.as_str(), i);
            while j < words.len() && !rest.is_empty() && rest.starts_with(&words[j].bare) {
                rest = &rest[words[j].bare.len()..];
                j += 1;
            }
            if rest.is_empty() && j > i {
                out.push((i, j - i));
            }
        }
        out
    }

    /// Up to `LISTED` places said most like `q`, best first (the earlier on a tie), none overlapping another.
    fn nearest(&self, q: &[String]) -> Vec<usize> {
        let words = &self.words;
        let n = q.len().max(1);
        let mut scored: Vec<(u32, usize)> = (0..words.len())
            .map(|i| {
                let score = (q.iter().zip(&words[i..]))
                    .map(|(x, w)| alike(x, &w.bare))
                    .sum::<u32>();
                (score, i)
            })
            .filter(|&(score, _)| score >= 2)
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

    /// `count` words from word `i` as Transcript shows them, quoted, and where they start: `"we went flying" (10.3 s)`.
    fn excerpt(&self, i: usize, count: usize) -> String {
        let end = (i + count).min(self.words.len());
        let shown: Vec<&str> = self.words[i..end]
            .iter()
            .map(|w| w.shown.as_str())
            .collect();
        format!("{:?} ({} s)", shown.join(" "), secs(self.words[i].start_us))
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
    /// outside `cut` (the snapshot footage the answer's removals take out), or why not, for the model.
    pub(super) fn find(
        &self,
        cut: &[RangeUs],
        (tool, field): (&str, &str),
        quote: &str,
    ) -> Result<Us, Refused> {
        let q = quoted(quote);
        if q.is_empty() {
            return Err(to_model(format!(
                "{tool}: {field} {quote:?} holds no words. Quote 1 to 4 words exactly as Transcript shows them."
            )));
        }
        let places = self.places(&q);
        if places.is_empty() {
            let near = self.nearest(&q);
            let said = if near.is_empty() {
                String::new()
            } else {
                format!(" Said most like it: {}.", self.listed(&near, q.len()))
            };
            return Err(to_model(format!(
                "{tool}: {field} {quote:?} is not said in the video.{said} Quote 1 to 4 words exactly as Transcript \
                 shows them."
            )));
        }
        let taken = |&(i, _): &(usize, usize)| {
            let t = self.words[i].start_us;
            cut.iter().any(|r| r.start <= t && t < r.end)
        };
        let kept: Vec<(usize, usize)> = places.iter().copied().filter(|p| !taken(p)).collect();
        match kept.as_slice() {
            [(i, _)] => Ok(self.words[*i].start_us),
            [] => {
                let all: Vec<usize> = places.iter().map(|&(i, _)| i).collect();
                Err(to_model(format!(
                    "{tool}: {field} {quote:?} is said only where this answer cuts: {}. Quote words in the parts you \
                     keep.",
                    self.listed(&all, places[0].1)
                )))
            }
            several => {
                let at: Vec<usize> = several.iter().map(|&(i, _)| i).collect();
                Err(to_model(format!(
                    "{tool}: {field} {quote:?} is said {} times. Quote more of the words to say which: {}.",
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
        s.find(&[], ("addSound", "atWords"), quote)
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
        assert_eq!(found(&s, "home"), Ok(14_500_000));
    }

    #[test]
    fn a_quote_said_in_several_places_asks_for_more_words() {
        let reason = found(&flight(), "we went").unwrap_err();
        assert_eq!(
            reason,
            "addSound: atWords \"we went\" is said 2 times. Quote more of the words to say which: \"we went flying \
             today.\" (10.3 s), \"We went home.\" (14.0 s)."
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
        assert!(
            found(&flight(), "...")
                .unwrap_err()
                .contains("holds no words")
        );
    }

    #[test]
    fn a_place_the_answer_cuts_does_not_count() {
        let s = flight();
        let cut = [RangeUs {
            start: 13_500_000,
            end: 16_000_000,
        }];
        assert_eq!(
            s.find(&cut, ("addImage", "atWords"), "we went").ok(),
            Some(10_300_000)
        );
        let reason = s
            .find(&cut, ("addImage", "atWords"), "home")
            .unwrap_err()
            .reason;
        assert_eq!(
            reason,
            "addImage: atWords \"home\" is said only where this answer cuts: \"home.\" (14.5 s). Quote words in the \
             parts you keep."
        );
    }

    #[test]
    fn words_written_without_spaces_are_found_across_words() {
        let s = said(&[("今日は", 1.0), ("いい", 1.5), ("天気", 1.8), ("です", 2.2)]);
        assert_eq!(found(&s, "いい天気"), Ok(1_500_000));
        assert_eq!(found(&s, "今日はいい天気です"), Ok(1_000_000));
        // Never part of a word.
        assert!(found(&s, "日はいい").is_err());
    }
}
