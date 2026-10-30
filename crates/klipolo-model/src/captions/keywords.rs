//! Keyword emphasis: offline and deterministic. Best first: numbers and money, proper nouns
//! capitalised mid-sentence, then a short English list of power words (Arcade: gaming words). At most one per page and
//! one per 1.2 s; the person's own emphasis decides first.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::style::KeywordWords;

/// How a keyword is coloured: `gain` for money and gain words, `loss` for loss words, the keyword colour otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "CaptionKeyword")]
pub enum Keyword {
    Plain,
    Gain,
    Loss,
}

/// Keywords the picker chooses are at least this far apart.
pub const KEYWORD_GAP_US: crate::time::Us = 1_200_000;

/// A listed word, its priority and its class. Gain and loss words come second (after numbers, then proper nouns); the
/// power list's own words third. Sorted, so a word is one binary search (`keywords_are_sorted`).
type Listed = (&'static str, u8, Keyword);

/// The power list: gain words, loss words and power words.
const POWER: &[Listed] = &[
    ("always", 3, Keyword::Plain),
    ("bad", 2, Keyword::Loss),
    ("best", 2, Keyword::Gain),
    ("biggest", 3, Keyword::Plain),
    ("bonus", 2, Keyword::Gain),
    ("broke", 2, Keyword::Loss),
    ("can't", 2, Keyword::Loss),
    ("crazy", 3, Keyword::Plain),
    ("danger", 2, Keyword::Loss),
    ("dead", 2, Keyword::Loss),
    ("don't", 2, Keyword::Loss),
    ("double", 2, Keyword::Gain),
    ("easy", 2, Keyword::Gain),
    ("every", 3, Keyword::Plain),
    ("exactly", 3, Keyword::Plain),
    ("fail", 2, Keyword::Loss),
    ("failed", 2, Keyword::Loss),
    ("fast", 2, Keyword::Gain),
    ("first", 3, Keyword::Plain),
    ("free", 2, Keyword::Gain),
    ("growth", 2, Keyword::Gain),
    ("hate", 2, Keyword::Loss),
    ("huge", 3, Keyword::Plain),
    ("insane", 3, Keyword::Plain),
    ("last", 3, Keyword::Plain),
    ("literally", 3, Keyword::Plain),
    ("lose", 2, Keyword::Loss),
    ("lost", 2, Keyword::Loss),
    ("love", 2, Keyword::Gain),
    ("mistake", 2, Keyword::Loss),
    ("money", 2, Keyword::Gain),
    ("more", 2, Keyword::Gain),
    ("never", 2, Keyword::Loss),
    ("new", 2, Keyword::Gain),
    ("only", 3, Keyword::Plain),
    ("profit", 2, Keyword::Gain),
    ("quit", 2, Keyword::Loss),
    ("rich", 2, Keyword::Gain),
    ("save", 2, Keyword::Gain),
    ("saved", 2, Keyword::Gain),
    ("scam", 2, Keyword::Loss),
    ("secret", 2, Keyword::Gain),
    ("stop", 2, Keyword::Loss),
    ("success", 2, Keyword::Gain),
    ("win", 2, Keyword::Gain),
    ("won", 2, Keyword::Gain),
    ("worst", 2, Keyword::Loss),
    ("wrong", 2, Keyword::Loss),
];

/// Arcade's gaming list: gain and loss words.
const GAMING: &[Listed] = &[
    ("ace", 2, Keyword::Gain),
    ("clutch", 2, Keyword::Gain),
    ("crazy", 2, Keyword::Gain),
    ("dead", 2, Keyword::Loss),
    ("died", 2, Keyword::Loss),
    ("down", 2, Keyword::Loss),
    ("gg", 2, Keyword::Gain),
    ("go", 2, Keyword::Gain),
    ("headshot", 2, Keyword::Gain),
    ("insane", 2, Keyword::Gain),
    ("let's", 2, Keyword::Gain),
    ("lost", 2, Keyword::Loss),
    ("no", 2, Keyword::Loss),
    ("noob", 2, Keyword::Loss),
    ("pog", 2, Keyword::Gain),
    ("rip", 2, Keyword::Loss),
    ("victory", 2, Keyword::Gain),
    ("win", 2, Keyword::Gain),
];

/// The longest listed word, in bytes ("literally").
const LONGEST: usize = 9;

/// `word` without the marks at its edges (apostrophes stay): what the lists and the proper-noun rule read.
fn bare(word: &str) -> &str {
    word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’')
}

/// `bare` lower-cased into `buf`, `’` read as `'`, without allocating; None when it can't be a listed word (longer than
/// the longest, or not ASCII).
fn lowered<'b>(bare: &str, buf: &'b mut [u8; LONGEST]) -> Option<&'b [u8]> {
    let mut n = 0;
    for c in bare.chars() {
        let b = match c {
            '’' => b'\'',
            c if c.is_ascii() => (c as u8).to_ascii_lowercase(),
            _ => return None,
        };
        *buf.get_mut(n)? = b;
        n += 1;
    }
    Some(&buf[..n])
}

/// `word` (as heard, not cased) as a keyword candidate: its priority (0 best) and class. It allocates nothing.
pub fn candidate(word: &str, sentence_start: bool, words: KeywordWords) -> Option<(u8, Keyword)> {
    if word.bytes().any(|b| b.is_ascii_digit()) {
        let money = word.contains(['$', '€', '£']);
        return Some((0, if money { Keyword::Gain } else { Keyword::Plain }));
    }
    let b = bare(word);
    if b.is_empty() {
        return None;
    }
    let mut buf = [0; LONGEST];
    let lower = lowered(b, &mut buf);
    // Its first letter, past an opening quote or bracket.
    let capital = word
        .chars()
        .find(|c| c.is_alphanumeric())
        .is_some_and(char::is_uppercase);
    // "I" and its contractions ("I'm", "I'll", "I've", "I'd") are capitalised everywhere: never proper nouns.
    let pronoun = lower.is_some_and(|l| l == b"i" || l.starts_with(b"i'"));
    if !sentence_start && capital && !pronoun && b.len() >= 2 {
        return Some((1, Keyword::Plain));
    }
    let list = match words {
        KeywordWords::Power => POWER,
        KeywordWords::Gaming => GAMING,
    };
    let lower = lower?;
    list.binary_search_by(|(w, ..)| w.as_bytes().cmp(lower))
        .ok()
        .map(|i| (list[i].1, list[i].2))
}

#[cfg(test)]
mod tests {
    use super::{GAMING, LONGEST, POWER};

    #[test]
    fn keywords_are_sorted() {
        for list in [POWER, GAMING] {
            assert!(list.windows(2).all(|w| w[0].0 < w[1].0));
            assert!(list
                .iter()
                .all(|(w, ..)| w.len() <= LONGEST && w.is_ascii()));
        }
    }
}
