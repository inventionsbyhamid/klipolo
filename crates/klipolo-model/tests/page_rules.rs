//! Caption page rules a person sees in exports: "I'm" is never a keyword, a page never hangs
//! over the next shot, and punctuation goes only at a word's edges.

use klipolo_model::captions::pages::{captions, Keyword, Pages};
use klipolo_model::project::FormatKind;
use klipolo_model::time::secs_to_us as s;

mod common;
use common::speech::{clips_from, project_with_words, two_clips, w, words_at};

fn keyed(pages: &Pages) -> Vec<(String, Keyword)> {
    pages
        .pages
        .iter()
        .flat_map(|p| p.words.0..p.words.1)
        .filter_map(|i| {
            let w = &pages.words[i as usize];
            pages.keyword(i).map(|k| (pages.text_of(w).to_owned(), k))
        })
        .collect()
}

/// The pronoun and its contractions ("I'm", "I'll", "I've", "I'd") aren't proper nouns, so they never take a page's
/// keyword from the style's own word.
#[test]
fn i_m_is_never_a_keyword() {
    for (style, sentence, word, class) in [
        ("caption.bold", "so I'm never late", "NEVER", Keyword::Loss),
        ("caption.loud", "so I'm never late", "NEVER", Keyword::Loss),
        ("caption.arcade", "so I’ll be dead", "DEAD", Keyword::Loss),
        (
            "caption.bold",
            "then I've won and I'd say",
            "WON",
            Keyword::Gain,
        ),
    ] {
        let (p, sp) = project_with_words(style, &words_at(0.0, sentence));
        let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
        assert_eq!(
            keyed(pages),
            [(word.to_owned(), class)],
            "{style}: {sentence}"
        );
    }
}

/// A page's hold stops where its clip's words end on the timeline, unless the next page starts within the style's
/// `lingerMs` after the cut, when it holds to it (a jump cut never blinks). Clean, two clips: c1 0–20 s, c2 from 20 s
/// playing the file from 30 s.
#[test]
fn a_page_ends_at_the_cut_unless_the_next_one_is_close() {
    // "Done." at 19.3–19.5 s holds to 20.133 s (Clean's 833 ms) by itself.
    let ends = |next_at: f64| {
        let (p, sp) = two_clips(
            "caption.clean",
            vec![
                w("Done.", 19.3, 19.5),
                w("Next", 30.0 + next_at, 30.3 + next_at),
            ],
        );
        let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
        assert_eq!(pages.pages.len(), 2);
        (pages.pages[0].end_us, pages.pages[1].start_us)
    };
    assert_eq!(
        ends(2.0),
        (s(20.0), s(22.0)),
        "words 2 s into the next shot: it ends at the cut"
    );
    assert_eq!(
        ends(0.18),
        (s(20.18), s(20.18)),
        "words 0.18 s after the cut: it holds to them"
    );
}

/// Styles that show only `?` and `!` drop marks only at a word's edges, so times, ratios, names and money stay as
/// said, a fix included.
#[test]
fn punctuation_goes_only_at_a_words_edges() {
    for style in [
        "caption.bold",
        "caption.loud",
        "caption.highlight",
        "caption.arcade",
    ] {
        let (p, sp) = project_with_words(
            style,
            &words_at(0.0, "24/7 1:30 C++ R&D #1 “$5,000.” really?!"),
        );
        let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
        let shown: Vec<&str> = pages.words.iter().map(|w| pages.text_of(w)).collect();
        assert_eq!(
            shown,
            ["24/7", "1:30", "C++", "R&D", "#1", "$5,000", "REALLY?!"],
            "{style}"
        );
    }
}

/// A dry run that moves words pages only the previewed format and keeps the committed revision's others, shared.
#[test]
fn a_dry_run_pages_only_its_format_and_keeps_the_others() {
    use std::sync::Arc;

    let (p, sp) = project_with_words("caption.bold", &words_at(0.0, "one two three four"));
    let committed = captions(&p, &sp);
    let mut trimmed = p.clone();
    trimmed.timeline.main[0].source_in_us = s(0.5);
    let dry = klipolo_model::captions::previewed(&trimmed, &sp, FormatKind::Portrait, &committed);
    for f in [FormatKind::Landscape, FormatKind::Square] {
        assert!(
            Arc::ptr_eq(&dry.pages[&f], &committed.pages[&f]),
            "{f:?} kept"
        );
    }
    assert!(!Arc::ptr_eq(
        &dry.pages[&FormatKind::Portrait],
        &committed.pages[&FormatKind::Portrait]
    ));
    assert_eq!(
        dry.pages[&FormatKind::Portrait].pages[0].start_us,
        s(0.1),
        "\"three\" from 0.6 s, now at 0.1 s"
    );
}

/// `dropFinalPeriod` (Boxed) shows each page's last word without its final period; a period inside a word stays.
#[test]
fn boxed_drops_a_pages_final_period() {
    let (p, sp) = project_with_words(
        "caption.boxed",
        &words_at(0.0, "It works. v1.5 ships today."),
    );
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    let shown: Vec<String> = pages.pages.iter().map(|pg| pages.shown_text(pg)).collect();
    assert_eq!(shown, ["It works", "v1.5 ships today"]);
}

/// Hindi's danda and Chinese's "。" end a sentence and a page like a period, and Boxed drops them the same way; a danda
/// alone is no word to drop it from.
#[test]
fn boxed_drops_a_danda_or_a_chinese_full_stop() {
    let (p, sp) = project_with_words(
        "caption.boxed",
        &words_at(0.0, "यह काम करता है। अब देखो। 今 天 好。"),
    );
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    let shown: Vec<String> = pages.pages.iter().map(|pg| pages.shown_text(pg)).collect();
    assert_eq!(shown, ["यह काम करता है", "अब देखो", "今天好"]);
}

/// Only a real cut ends a hold. A split left in place (c2 goes on from source 20 s, at any speed) is
/// no cut, so "Done." holds its 833 ms, though the next words come 0.6 s of source after the split.
#[test]
fn a_split_left_in_place_is_no_cut() {
    for (speed, next) in [(1.0, 20.6), (2.0, 20.3)] {
        let (p, sp) = clips_from(
            "caption.clean",
            vec![w("Done.", 19.3, 19.5), w("Next", 20.6, 20.9)],
            20.0,
            speed,
        );
        let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
        assert_eq!(
            (pages.pages[0].end_us, pages.pages[1].start_us),
            (s(20.133), s(next)),
            "at {speed}×"
        );
    }
}

/// A split left in place never ends a page, at any speed, nor does a hidden word said there; a
/// real cut still does. Clean (no word limit), c2 from source 20 s (in place) or 30 s (a cut).
#[test]
fn a_split_left_in_place_never_ends_a_page() {
    use klipolo_model::captions::looks::{Look, WordLook};

    let shown = |pages: &Pages| -> Vec<String> {
        pages.pages.iter().map(|pg| pages.shown_text(pg)).collect()
    };
    for speed in [1.0, 2.0] {
        let (mut p, sp) = clips_from(
            "caption.clean",
            vec![
                w("We", 19.2, 19.4),
                w("go", 19.5, 19.7),
                w("um", 19.85, 19.95),
                w("on", 20.0, 20.15),
                w("together", 20.2, 20.4),
            ],
            20.0,
            speed,
        );
        // "um", said right before the split, hidden.
        p.word_looks = vec![WordLook {
            asset_id: "v".into(),
            start_us: s(19.85),
            end_us: s(19.95),
            look: Look {
                hidden: Some(true),
                ..Look::default()
            },
        }]
        .into();
        let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
        assert_eq!(shown(pages), ["We go on together"], "at {speed}×");
    }
    let (p, sp) = clips_from(
        "caption.clean",
        vec![
            w("We", 19.2, 19.4),
            w("go", 19.5, 19.7),
            w("on", 30.0, 30.2),
        ],
        30.0,
        1.0,
    );
    let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
    assert_eq!(shown(pages), ["We go", "on"], "a real cut");
}

/// A minus, a decimal point or an apostrophe before a digit stays at a word's edge.
#[test]
fn a_sign_or_point_before_a_digit_stays() {
    let (p, sp) = project_with_words("caption.bold", &words_at(0.0, "-5 −3 -20% .5 '90s “-7.”"));
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    let shown: Vec<&str> = pages.words.iter().map(|w| pages.text_of(w)).collect();
    assert_eq!(shown, ["-5", "−3", "-20%", ".5", "'90S", "-7"]);
}

/// A `captions.json` that marks a page's period as dropped where there is none (damaged or edited by
/// hand) shows the word as it is, never panics.
#[test]
fn a_dropped_period_that_isnt_there_shows_the_word() {
    let mut pages = Pages::one_page("v", 0, s(1.0), &[("Hi", 0, s(0.5)), ("é", s(0.5), s(0.9))]);
    pages.pages[0].period_dropped = true;
    assert_eq!(pages.shown_text(&pages.pages[0]), "Hi é");
    assert_eq!(pages.word_text(&pages.pages[0], 1), "é");
}
