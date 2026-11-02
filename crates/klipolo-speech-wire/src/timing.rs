//! How close a transcript is to its reference, for measuring the timing rules (only `fillers`, each language's
//! hesitations, runs in the app, where analysis finds filler sounds with it):
//! - `score`: word starts against labels (a forced alignment, or a synthetic set's construction). Words are matched to
//!   labels by the longest common subsequence of their normalised texts (lowercased letters and digits), so a word
//!   heard wrongly, added or dropped is left out rather than shifting every match after it. Only starts are scored,
//!   because a caption's highlight starts on them; ends are measured too, to be printed.
//! - `wer` and `wer_normalised`: the word error rate, plain or with fillers dropped and numbers written one way, which
//!   is the one to compare timings or languages by.

use klipolo_model::languages::base_language;
use klipolo_model::time::Us;

use crate::words::RawWord;

/// One reference word: its text and where its sound starts and ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub text: String,
    pub start_us: Us,
    pub end_us: Us,
}

/// Matched words' timing against their labels. Errors are in ms, rounded; percentiles are nearest-rank.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimingScore {
    /// Labels scored against.
    pub labels: usize,
    /// Labels a word was matched to.
    pub matched: usize,
    /// Median and 90th percentile of |word start − label start|.
    pub median_ms: i64,
    pub p90_ms: i64,
    /// Median of word start − label start: below 0, the words start early.
    pub bias_ms: i64,
    /// Median and 90th percentile of |word end − label end| (printed, never gated).
    pub end_median_ms: i64,
    pub end_p90_ms: i64,
}

impl TimingScore {
    /// The share of labels matched (1 with no labels).
    pub fn matched_share(&self) -> f64 {
        if self.labels == 0 {
            1.0
        } else {
            self.matched as f64 / self.labels as f64
        }
    }
}

/// Lowercased letters and digits only: "Hello," and "hello" match, "don't" is "dont".
pub fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The (label, word) index pairs of the longest common subsequence of the two normalised texts, in order. A word or
/// label with no letters or digits never matches. O(labels × words) time and memory (a bench's utterance or window,
/// never a job's).
pub fn pairs(labels: &[Label], words: &[RawWord]) -> Vec<(usize, usize)> {
    let l: Vec<String> = labels.iter().map(|l| normalise(&l.text)).collect();
    let w: Vec<String> = words.iter().map(|w| normalise(&w.text)).collect();
    let (n, m) = (l.len(), w.len());
    // table[at(i, j)]: the LCS length of l[i..] and w[j..].
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if !l[i].is_empty() && l[i] == w[j] {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if !l[i].is_empty() && l[i] == w[j] {
            out.push((i, j));
            (i, j) = (i + 1, j + 1);
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// `words`' starts (and ends) against `labels`, over the words matched to them (`pairs`).
pub fn score(labels: &[Label], words: &[RawWord]) -> TimingScore {
    let matched = pairs(labels, words);
    let mut signed: Vec<Us> = matched
        .iter()
        .map(|&(l, w)| words[w].start_us - labels[l].start_us)
        .collect();
    let mut ends: Vec<Us> = matched
        .iter()
        .map(|&(l, w)| (words[w].end_us - labels[l].end_us).abs())
        .collect();
    let mut abs: Vec<Us> = signed.iter().map(|e| e.abs()).collect();
    TimingScore {
        labels: labels.len(),
        matched: matched.len(),
        median_ms: ms(rank(&mut abs, 0.5)),
        p90_ms: ms(rank(&mut abs, 0.9)),
        bias_ms: ms(rank(&mut signed, 0.5)),
        end_median_ms: ms(rank(&mut ends, 0.5)),
        end_p90_ms: ms(rank(&mut ends, 0.9)),
    }
}

/// The nearest-rank `p` quantile (0 for none).
fn rank(v: &mut [Us], p: f64) -> Us {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let k = ((p * v.len() as f64).ceil() as usize).clamp(1, v.len());
    v[k - 1]
}

fn ms(us: Us) -> i64 {
    (us as f64 / 1_000.0).round() as i64
}

/// The lowercased letter-and-digit runs of `text`.
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.extend(c.to_lowercase());
        } else if !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// Substitutions, deletions and insertions over the reference's word count. Words are runs of letters and digits
/// (`char::is_alphanumeric`, so Cyrillic, Vietnamese and CJK count), lowercased; punctuation and case never count. An
/// empty reference scores 0 against an empty hypothesis and 1 against any other. O(reference × hypothesis) time,
/// O(hypothesis) memory.
pub fn wer(reference: &str, hypothesis: &str) -> f64 {
    edit_rate(&words(reference), &words(hypothesis))
}

/// `wer` after both texts are normalised the same way, for `language` (a language code such as `en`; a tag such as
/// `en-US` counts as its base language):
/// - that language's own hesitations are dropped (`fillers`), never a word of another language: "um" and "er" are
///   German words, "er" a Dutch one;
/// - an apostrophe inside a word is dropped ("don't" is "dont");
/// - English only: digits grouped with commas join ("1,200"), a digit's ordinal ending goes ("21st" is
///   21), and number words become digits: "twenty four" is 24, "one thousand two hundred" 1200, "a hundred" 100,
///   "nineteen eighty four" 1984 (a year starts only at 13–20, never right after an ordinal), "twenty first" 21;
///   digits read one by one ("one two three") and clock times ("eleven thirty") stay separate numbers.
///   Every other language compares digits as written (its grouping and decimal marks differ) and keeps its number
///   words. No language reads decimals: "3.5" is 3 and 5 on both sides.
pub fn wer_normalised(reference: &str, hypothesis: &str, language: &str) -> f64 {
    let language = base_language(language);
    edit_rate(
        &normalised(reference, language),
        &normalised(hypothesis, language),
    )
}

/// `language`'s hesitations (its base language's), which no reference or recognizer writes the same way. A language
/// not listed has only wordless hums.
pub fn fillers(language: &str) -> &'static [&'static str] {
    match base_language(language) {
        "en" => &[
            "um", "umm", "uh", "uhh", "uhm", "er", "erm", "ah", "hm", "hmm", "mm", "mhm",
        ],
        "de" => &["äh", "ähm", "öh", "öhm", "hm", "hmm", "mhm"],
        "nl" => &["uh", "uhm", "eh", "ehm", "hm", "hmm"],
        "fr" => &["euh", "heu", "hum", "hm", "hmm"],
        "es" => &["eh", "ehm", "mmm", "hm"],
        "it" => &["ehm", "eh", "uhm", "mmm", "hm"],
        "pt" => &["ahn", "hum", "hm"],
        // Hindi as Voice Transcribe writes it (in Devanagari, English hums and all) and as Hinglish is typed. Never
        // "हम" ("we") or "अ" alone.
        "hi" => &[
            "um",
            "umm",
            "uh",
            "uhh",
            "uhm",
            "hm",
            "hmm",
            "mm",
            "mhm",
            "उम",
            "उम्म",
            "अं",
            "अम्म",
            "हम्म",
        ],
        _ => &["hm", "hmm", "mm", "mhm"],
    }
}

fn normalised(text: &str, language: &str) -> Vec<String> {
    let english = language == "en";
    let chars: Vec<char> = text.chars().collect();
    let between = |i: usize, f: fn(&char) -> bool| {
        i > 0 && i + 1 < chars.len() && f(&chars[i - 1]) && f(&chars[i + 1])
    };
    let cleaned: String = chars
        .iter()
        .enumerate()
        .filter(|&(i, c)| {
            let apostrophe = matches!(*c, '\'' | '’') && between(i, |c| c.is_alphabetic());
            let grouping = english && *c == ',' && between(i, char::is_ascii_digit);
            !apostrophe && !grouping
        })
        .map(|(_, c)| c)
        .collect();
    let fillers = fillers(language);
    let ws: Vec<String> = words(&cleaned)
        .into_iter()
        .filter(|w| !fillers.contains(&w.as_str()))
        .map(|w| if english { ordinal_digits(w) } else { w })
        .collect();
    if !english {
        return ws;
    }
    let mut out: Vec<String> = Vec::with_capacity(ws.len());
    let mut i = 0;
    while i < ws.len() {
        match number_at(&ws[i..]) {
            Some((n, used, ordinal)) => {
                // A year read in two halves ("nineteen eighty four", "twenty twenty four"): 13–20, then 10–99. Never
                // from 11 or 12 (a clock time: "eleven thirty") nor right after an ordinal ("the twelfth twenty twenty
                // four").
                let year = (!ordinal && (13..=20).contains(&n))
                    .then(|| number_at(&ws[i + used..]))
                    .flatten()
                    .filter(|&(m, _, _)| (10..=99).contains(&m));
                match year {
                    Some((m, more, _)) => {
                        out.push((n * 100 + m).to_string());
                        i += used + more;
                    }
                    None => {
                        out.push(n.to_string());
                        i += used;
                    }
                }
            }
            None => {
                out.push(ws[i].clone());
                i += 1;
            }
        }
    }
    out
}

/// "21st", "2nd", "3rd", "4th" as their digits.
fn ordinal_digits(w: String) -> String {
    let digits = w.trim_end_matches(char::is_alphabetic);
    let ending = &w[digits.len()..];
    if !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && ["st", "nd", "rd", "th"].contains(&ending)
    {
        digits.to_owned()
    } else {
        w
    }
}

/// The English number spelled at the start of `ws`, how many words it takes, and whether it ends in an ordinal, if
/// one is.
fn number_at(ws: &[String]) -> Option<(u64, usize, bool)> {
    #[derive(Clone, Copy, PartialEq)]
    enum Last {
        Nothing,
        Unit,
        Ten,
        Scale,
    }
    let (mut total, mut current, mut last, mut used) = (0u64, 0u64, Last::Nothing, 0);
    let mut ordinal_end = false;
    for (i, w) in ws.iter().map(String::as_str).enumerate() {
        if let Some(v) = ordinal(w) {
            // "twenty first", "third": an ordinal ends its number.
            let fits = if v < 10 {
                last != Last::Unit
            } else {
                !matches!(last, Last::Unit | Last::Ten)
            };
            if fits {
                current += v;
                used = i + 1;
                ordinal_end = true;
            }
            break;
        }
        if w == "a"
            && last == Last::Nothing
            && ws
                .get(i + 1)
                .is_some_and(|n| n == "hundred" || scale(n).is_some())
        {
            // "a hundred", "a thousand".
            current = 1;
            last = Last::Unit;
        } else if let Some(v) = unit(w) {
            // A unit after a unit or a teen ("one two"), or a teen after a ten, starts another number.
            if last == Last::Unit || (last == Last::Ten && v >= 10) {
                break;
            }
            current += v;
            last = Last::Unit;
        } else if let Some(v) = ten(w) {
            if matches!(last, Last::Unit | Last::Ten) {
                break;
            }
            current += v;
            last = Last::Ten;
        } else if w == "hundred" && current > 0 {
            current *= 100;
            last = Last::Scale;
        } else if let Some(scale) = scale(w).filter(|_| last != Last::Nothing) {
            total += current.max(1) * scale;
            current = 0;
            last = Last::Scale;
        } else if w == "and"
            && last == Last::Scale
            && ws
                .get(i + 1)
                .is_some_and(|n| unit(n).is_some() || ten(n).is_some())
        {
            // "two hundred and five": part of the number.
        } else {
            break;
        }
        used = i + 1;
    }
    (used > 0).then_some((total + current, used, ordinal_end))
}

fn unit(w: &str) -> Option<u64> {
    const UNITS: [&str; 20] = [
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
    ];
    UNITS.iter().position(|u| *u == w).map(|v| v as u64)
}

fn ten(w: &str) -> Option<u64> {
    const TENS: [&str; 8] = [
        "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    TENS.iter()
        .position(|t| *t == w)
        .map(|v| 20 + 10 * v as u64)
}

/// English ordinals to ninetieth.
fn ordinal(w: &str) -> Option<u64> {
    const ORDINALS: [&str; 20] = [
        "",
        "first",
        "second",
        "third",
        "fourth",
        "fifth",
        "sixth",
        "seventh",
        "eighth",
        "ninth",
        "tenth",
        "eleventh",
        "twelfth",
        "thirteenth",
        "fourteenth",
        "fifteenth",
        "sixteenth",
        "seventeenth",
        "eighteenth",
        "nineteenth",
    ];
    const TENTHS: [&str; 8] = [
        "twentieth",
        "thirtieth",
        "fortieth",
        "fiftieth",
        "sixtieth",
        "seventieth",
        "eightieth",
        "ninetieth",
    ];
    ORDINALS[1..]
        .iter()
        .position(|o| *o == w)
        .map(|v| v as u64 + 1)
        .or_else(|| {
            TENTHS
                .iter()
                .position(|t| *t == w)
                .map(|v| 20 + 10 * v as u64)
        })
}

fn scale(w: &str) -> Option<u64> {
    match w {
        "thousand" => Some(1_000),
        "million" => Some(1_000_000),
        "billion" => Some(1_000_000_000),
        _ => None,
    }
}

fn edit_rate(r: &[String], h: &[String]) -> f64 {
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    // One row of the edit-distance table at a time: `row[j]` is the distance between the reference so far and h[..j].
    let mut row: Vec<usize> = (0..=h.len()).collect();
    for rw in r {
        let mut diagonal = row[0];
        row[0] += 1;
        for (j, hw) in h.iter().enumerate() {
            let substitute = diagonal + usize::from(rw != hw);
            diagonal = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[h.len()] as f64 / r.len() as f64
}
