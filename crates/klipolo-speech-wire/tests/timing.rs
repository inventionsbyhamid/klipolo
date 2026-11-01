//! `timing`: word starts scored against labels, and the word error rate, plain or normalised.

use klipolo_speech_wire::timing::{score, wer, wer_normalised, Label};
use klipolo_speech_wire::words::RawWord;

fn us(secs: f64) -> i64 {
    (secs * 1e6).round() as i64
}

fn label(text: &str, start: f64, end: f64) -> Label {
    Label {
        text: text.into(),
        start_us: us(start),
        end_us: us(end),
    }
}

fn raw(text: &str, start: f64, end: f64) -> RawWord {
    RawWord {
        text: text.into(),
        start_us: us(start),
        end_us: us(end),
        confidence: 1.0,
    }
}

#[test]
fn a_perfect_transcript_scores_zero_and_a_missed_word_is_left_out() {
    let labels = [
        label("hello", 0.32, 0.61),
        label("everyone", 0.66, 1.20),
        label("today", 2.0, 2.4),
    ];
    let words = [raw("Hello,", 0.32, 0.61), raw("everyone.", 0.66, 1.20)];
    let s = score(&labels, &words);
    assert_eq!((s.matched, s.median_ms, s.p90_ms), (2, 0, 0));
}

#[test]
fn start_errors_are_absolute_and_the_median_is_of_the_matched_words() {
    let s = score(
        &[
            label("a", 1.0, 1.2),
            label("b", 2.0, 2.2),
            label("c", 3.0, 3.2),
        ],
        &[
            raw("a", 1.04, 1.2),
            raw("b", 1.97, 2.2),
            raw("c", 3.10, 3.2),
        ],
    );
    assert_eq!((s.matched, s.median_ms, s.p90_ms), (3, 40, 100));
}

/// The signed median says which way a recognizer leans (one on 80 ms steps starts late more often than early), and a
/// word heard wrongly or added is never matched.
#[test]
fn the_bias_is_signed_and_an_added_or_wrong_word_is_never_matched() {
    let s = score(
        &[
            label("the", 0.0, 0.1),
            label("budget", 0.2, 0.6),
            label("shows", 0.7, 1.0),
            label("today", 1.2, 1.5),
        ],
        &[
            raw("the", 0.05, 0.1),
            raw("uh", 0.12, 0.18),
            raw("budgie", 0.25, 0.6),
            raw("shows", 0.68, 1.0),
            raw("today", 1.26, 1.5),
        ],
    );
    assert_eq!(
        (s.labels, s.matched, s.median_ms, s.bias_ms),
        (4, 3, 50, 50)
    );
}

#[test]
fn fillers_and_spelled_numbers_count_only_in_the_plain_word_error_rate() {
    let reference = "Um, the total is twenty four dollars and one thousand two hundred cents.";
    let heard = "The total is 24 dollars and 1,200 cents";
    assert!(wer(reference, heard) > 0.3, "{}", wer(reference, heard));
    assert_eq!(wer_normalised(reference, heard, "en"), 0.0);
    assert_eq!(wer_normalised("I don't know", "i dont know uh", "en"), 0.0);
    // Digits read one by one stay separate numbers.
    assert_eq!(
        wer_normalised("code one two three", "code 1 2 3", "en"),
        0.0
    );
    assert_eq!(
        wer_normalised("the budget shows", "the budgets shows", "en"),
        1.0 / 3.0
    );
}

/// German "um" (around, in order to) and "er" (he), and Dutch "er" (there), are words. Only each
/// language's own hesitations are dropped.
#[test]
fn fillers_are_each_languages_own_and_never_a_word_of_another() {
    assert_eq!(wer_normalised("er kommt um acht", "kommt acht", "de"), 0.5);
    assert_eq!(
        wer_normalised("Äh, er kommt um acht", "er kommt um acht", "de"),
        0.0
    );
    assert_eq!(
        wer_normalised("er is een probleem", "is een probleem", "nl"),
        0.25
    );
    assert_eq!(
        wer_normalised("euh, je pense que oui", "je pense que oui", "fr"),
        0.0
    );
    assert_eq!(wer_normalised("uh, I think so", "I think so", "en-US"), 0.0);
}

/// English number words become digits, years and ordinals included; other languages compare digits as
/// written, and their number words stay words.
#[test]
fn numbers_are_normalised_in_english_and_compared_as_written_elsewhere() {
    assert_eq!(
        wer_normalised("in nineteen eighty four", "in 1984", "en"),
        0.0
    );
    assert_eq!(wer_normalised("twenty twenty four", "2024", "en"), 0.0);
    assert_eq!(wer_normalised("a hundred rows", "100 rows", "en"), 0.0);
    assert_eq!(
        wer_normalised("the twenty first row", "the 21st row", "en"),
        0.0
    );
    assert_eq!(wer_normalised("zwei Zeilen", "2 Zeilen", "de"), 0.5);
    assert_eq!(wer_normalised("1.200 Zeilen", "1.200 Zeilen", "de"), 0.0);
}

/// A year starts only at 13–20 and never right after an ordinal, so a clock time and a date's day stay
/// their own numbers.
#[test]
fn a_clock_time_or_a_days_ordinal_is_never_read_as_a_year() {
    assert_eq!(wer_normalised("at eleven thirty", "at 11 30", "en"), 0.0);
    assert_eq!(wer_normalised("at twelve fifteen", "at 12 15", "en"), 0.0);
    assert_eq!(
        wer_normalised("the twelfth twenty twenty four", "the 12th 2024", "en"),
        0.0
    );
    assert_eq!(
        wer_normalised("in nineteen eighty four", "in 1984", "en"),
        0.0
    );
}
