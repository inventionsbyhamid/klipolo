//! How a style shows a word: its case and punctuation.

use std::borrow::Cow;

use super::style::{Punctuation, TextCase};
use crate::speech::is_mark;

/// Whether `c` may stay at a word's edge in a style that shows only `?` and `!`: letters and digits, the marks that
/// belong to a letter (`is_mark`: Tamil's pulli, a Thai tone mark), those two marks (full-width and Arabic too), and the
/// symbols that belong to a word ("C++", "#1", "@name", "$5", "90%"). Everything between two of these stays too
/// ("24/7", "1:30", "R&D", "$5,000", "don't").
fn edge_kept(c: char) -> bool {
    c.is_alphanumeric()
        || is_mark(c)
        || matches!(
            c,
            '?' | '!'
                | '？'
                | '！'
                | '؟'
                | '+'
                | '#'
                | '@'
                | '&'
                | '$'
                | '%'
                | '€'
                | '£'
                | '¥'
                | '°'
        )
}

/// `text` without the marks at its edges (`edge_kept`), except a minus, a decimal point or an apostrophe right before a
/// digit ("-5", "−3", ".5", "'90s").
fn edges_trimmed(text: &str) -> &str {
    let rest = text.trim_start_matches(|c| !edge_kept(c));
    let start = text.len() - rest.len();
    let core = rest.trim_end_matches(|c| !edge_kept(c));
    let sign = text[..start]
        .chars()
        .next_back()
        .filter(|&c| matches!(c, '-' | '−' | '.' | '\'' | '’'))
        .filter(|_| core.starts_with(|c: char| c.is_ascii_digit()));
    match sign {
        Some(c) => &text[start - c.len_utf8()..start + core.len()],
        None => core,
    }
}

/// A word as a style shows it: upper-cased for upper styles, and its punctuation kept, cut to `?` and `!` at its edges
/// (the sentence's and clause's marks, quotes and brackets go; what lies inside the word stays), or (for the page's
/// last word, done by the page builder) its final period dropped.
pub fn shown(text: &str, case: TextCase, punct: Punctuation) -> Cow<'_, str> {
    match (case, punct) {
        (TextCase::AsSaid, Punctuation::Keep | Punctuation::DropFinalPeriod) => Cow::Borrowed(text),
        _ => {
            let mut out = String::with_capacity(text.len());
            shown_into(text, case, punct, &mut out);
            Cow::Owned(out)
        }
    }
}

/// `shown`, appended to `out` (the page builder's buffer, kept from word to word: no allocation per word). ASCII, the
/// usual case, is copied at once and upper-cased in place.
pub(crate) fn shown_into(text: &str, case: TextCase, punct: Punctuation, out: &mut String) {
    let text = match punct {
        Punctuation::QuestionAndExclamation => edges_trimmed(text),
        Punctuation::Keep | Punctuation::DropFinalPeriod => text,
    };
    let from = out.len();
    match case {
        TextCase::AsSaid => out.push_str(text),
        TextCase::Upper if text.is_ascii() => {
            out.push_str(text);
            out[from..].make_ascii_uppercase();
        }
        TextCase::Upper => out.extend(text.chars().flat_map(char::to_uppercase)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A style that shows only `?` and `!` trims the sentence's marks, never a mark that is part of the last letter.
    #[test]
    fn marks_of_a_letter_stay_at_a_words_edge() {
        let q = |t| shown(t, TextCase::AsSaid, Punctuation::QuestionAndExclamation).into_owned();
        assert_eq!(q("வணக்கம்,"), "வணக்கம்");
        assert_eq!(q("นี้"), "นี้");
        assert_eq!(q("साफ़।"), "साफ़");
        assert_eq!(q("हैं।"), "हैं");
        assert_eq!(q("كيف؟"), "كيف؟");
        assert_eq!(q("«سلام»"), "سلام");
        assert_eq!(q("שָׁלוֹם."), "שָׁלוֹם");
        assert_eq!(q("\"Hi,\""), "Hi");
    }
}
