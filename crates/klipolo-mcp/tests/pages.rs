//! The transcript as agents read it: whole when it fits in `PAGE_BYTES`, else a list of pages by time, each read
//! whole; a range takes every line it overlaps and names where to go on when it is cut. Times read as clock times or
//! seconds.

use klipolo_chat::{FullTranscript, TranscriptLine};
use klipolo_mcp::pages::{PAGE_BYTES, page, parse_time};
use klipolo_model::time::{RangeUs, Us};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

/// A line from `start` to `end` seconds saying `words`.
fn line(start: f64, end: f64, words: &str) -> TranscriptLine {
    TranscriptLine {
        range: RangeUs {
            start: secs(start),
            end: secs(end),
        },
        text: format!("[{start:.1}-{end:.1}] A: {words}"),
    }
}

/// `n` lines of about `chars` characters, one every 4 s, each 3.5 s long.
fn talk(n: usize, chars: usize) -> FullTranscript {
    let lines = (0..n)
        .map(|i| {
            let start = 0.5 + 4.0 * i as f64;
            let words = format!("line {i} ") + &"talk ".repeat(chars / 5);
            line(start, start + 3.5, words.trim_end())
        })
        .collect();
    FullTranscript {
        lines,
        notes: "Not transcribed yet: c9.\n".into(),
    }
}

/// The transcript lines (`[a-b] ...`) a page holds.
fn lines_of(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.starts_with('[')).collect()
}

/// The `fromSec <a> ... toSec <b>` pairs a text names, in order.
fn ranges_named(text: &str) -> Vec<(Us, Us)> {
    // The number a text starts with, without the punctuation after it.
    let number = |s: &str| -> Option<Us> {
        let token = s.split_whitespace().next()?;
        parse_time(token.trim_end_matches([',', ':', ')']))
    };
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("fromSec ") {
        rest = &rest[at + "fromSec ".len()..];
        let Some(from) = number(rest) else { continue };
        let at = rest.find("toSec ").expect("a toSec after a fromSec");
        rest = &rest[at + "toSec ".len()..];
        out.push((from, number(rest).expect("a toSec number")));
    }
    out
}

#[test]
fn short_transcript_is_one_page() {
    let t = talk(20, 100);
    let text = page(&t, None, None);
    assert!(text.len() <= PAGE_BYTES);
    let shown = lines_of(&text);
    let all: Vec<&str> = t.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(shown, all, "every line, in order");
    assert!(text.ends_with("\nNot transcribed yet: c9.\n"), "{text}");
    assert!(!text.contains("fromSec"), "no pages: {text}");
    assert!(!text.contains(['\u{2013}', '\u{2014}']));

    // Nothing said yet: the notes say why.
    let empty = FullTranscript {
        lines: Vec::new(),
        notes: "No speech.\n".into(),
    };
    let text = page(&empty, None, None);
    assert!(lines_of(&text).is_empty());
    assert!(text.ends_with("\nNo speech.\n"), "{text}");
}

#[test]
fn long_transcript_lists_pages_under_40_kb() {
    // About 300 KB: two hours of a line every 4 s.
    let t = talk(1800, 160);
    let whole: usize = t.lines.iter().map(|l| l.text.len() + 1).sum();
    assert!(whole > 7 * PAGE_BYTES, "{whole}");
    let index = page(&t, None, None);
    assert!(index.len() <= PAGE_BYTES);
    assert!(lines_of(&index).is_empty(), "a list, not lines");
    assert!(index.contains("klipolo://transcript/"), "{index}");
    assert!(index.ends_with("\nNot transcribed yet: c9.\n"));
    let pages = ranges_named(&index);
    assert!(pages.len() >= 8, "{index}");
    assert_eq!(pages[0].0, 0);
    // Each page read whole, under the limit, and together every line once, in order.
    let mut read: Vec<String> = Vec::new();
    for (k, &(from, to)) in pages.iter().enumerate() {
        if k > 0 {
            assert_eq!(from, pages[k - 1].1, "pages meet");
        }
        let text = page(&t, Some(from), Some(to));
        assert!(text.len() <= PAGE_BYTES, "page {k}: {} bytes", text.len());
        assert!(!text.contains("More follows"), "page {k} is whole");
        read.extend(lines_of(&text).into_iter().map(str::to_owned));
    }
    let all: Vec<&str> = t.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(read, all);
}

#[test]
fn page_range_takes_overlapping_lines() {
    let t = FullTranscript {
        lines: vec![
            line(0.0, 5.0, "one"),
            line(5.0, 10.0, "two"),
            line(10.0, 15.0, "three"),
            line(15.0, 20.0, "four"),
        ],
        notes: String::new(),
    };
    let words = |from: f64, to: f64| -> Vec<String> {
        lines_of(&page(&t, Some(secs(from)), Some(secs(to))))
            .iter()
            .map(|l| l.rsplit(' ').next().unwrap().to_owned())
            .collect()
    };
    assert_eq!(words(7.0, 12.0), ["two", "three"]);
    // Lines that only touch an edge are outside.
    assert_eq!(words(10.0, 15.0), ["three"]);
    assert_eq!(words(9.9, 10.1), ["two", "three"]);
    assert_eq!(words(0.0, 100.0), ["one", "two", "three", "four"]);
    let none = page(&t, Some(secs(30.0)), Some(secs(40.0)));
    assert!(lines_of(&none).is_empty());
    assert!(none.contains("Nothing is said"), "{none}");
    // Only one end given: from there on, or up to there.
    let after = page(&t, Some(secs(12.0)), None);
    assert_eq!(lines_of(&after).len(), 2);
    let before = page(&t, None, Some(secs(7.0)));
    assert_eq!(lines_of(&before).len(), 2);
    // An end before its start reads nothing.
    let backwards = page(&t, Some(secs(12.0)), Some(secs(7.0)));
    assert!(lines_of(&backwards).is_empty());

    // A range longer than a page is cut at a whole line and names where to go on, which repeats no line.
    let long = talk(1800, 160);
    let first = page(&long, Some(0), Some(secs(7200.0)));
    assert!(first.len() <= PAGE_BYTES);
    let shown = lines_of(&first);
    assert!(shown.len() > 10 && shown.len() < 1800);
    let more = &first[first.find("More follows").expect("a next part")..];
    let next = ranges_named(more);
    assert_eq!(next.len(), 1, "{more}");
    let (from, to) = next[0];
    assert_eq!(to, secs(7200.0));
    assert_eq!(from, long.lines[shown.len()].range.start);
    let second = page(&long, Some(from), Some(to));
    assert_eq!(lines_of(&second)[0], long.lines[shown.len()].text);
}

#[test]
fn a_line_said_in_no_time_at_a_page_edge_is_read_once() {
    // A one-word sentence the recognizer gave no length, between two lines.
    let mut t = FullTranscript {
        lines: vec![
            line(0.0, 5.0, "one"),
            line(5.0, 5.0, "okay"),
            line(5.5, 10.0, "two"),
        ],
        notes: String::new(),
    };
    let words = |t: &FullTranscript, from: Option<f64>, to: Option<f64>| -> Vec<String> {
        lines_of(&page(t, from.map(secs), to.map(secs)))
            .iter()
            .map(|l| l.rsplit(' ').next().unwrap().to_owned())
            .collect()
    };
    // Read in two parts at its start: the line before ends there, it starts there.
    assert_eq!(words(&t, Some(0.0), Some(5.0)), ["one"]);
    assert_eq!(words(&t, Some(5.0), Some(20.0)), ["okay", "two"]);
    assert_eq!(words(&t, Some(5.0), None), ["okay", "two"]);
    // A page or a cut range that starts there, as a long transcript lists them, shows it.
    let long = talk(1800, 160);
    let index = page(&long, None, None);
    let edge = ranges_named(&index)[1].0;
    let at = long
        .lines
        .iter()
        .position(|l| l.range.start == edge)
        .unwrap();
    t.lines = long.lines.clone();
    t.lines[at].range.end = t.lines[at].range.start;
    let mut read = Vec::new();
    for (from, to) in ranges_named(&page(&t, None, None)) {
        read.extend(
            lines_of(&page(&t, Some(from), Some(to)))
                .into_iter()
                .map(str::to_owned),
        );
    }
    let all: Vec<&str> = t.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(read, all, "every line once");
}

#[test]
fn times_parse_clock_and_seconds() {
    let cases: [(&str, f64); 10] = [
        ("75.5", 75.5),
        ("0", 0.0),
        (" 12 ", 12.0),
        ("1:05", 65.0),
        ("0:00", 0.0),
        ("1:02:03", 3723.0),
        ("1:02:03.25", 3723.25),
        ("10:00.5", 600.5),
        ("1%3A05", 65.0),
        ("312.345678", 312.345678),
    ];
    for (text, expected) in cases {
        assert_eq!(parse_time(text), Some(secs(expected)), "{text}");
    }
    for bad in [
        "",
        "-1",
        "1:60",
        "1:75:00",
        "1:2:3:4",
        "abc",
        "1e3",
        "inf",
        "NaN",
        ":5",
        "5:",
        "1::2",
        "+3",
        "99999999999999",
    ] {
        assert_eq!(parse_time(bad), None, "{bad:?}");
    }
}
