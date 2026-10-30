//! When caption pages show: held into silence, read at the style's speed, found by
//! binary search, and never more words than the renderer lays out.

use klipolo_model::captions::pages::{captions, page_at, Pages, MAX_PAGE_WORDS};
use klipolo_model::project::FormatKind;
use klipolo_model::time::secs_to_us as s;

mod common;
use common::speech::{project_with_words, words_at};

fn texts(pages: &Pages) -> Vec<String> {
    pages
        .pages
        .iter()
        .map(|p| pages.shown_text(p).replace('\n', " "))
        .collect()
}

#[test]
fn a_short_page_holds_into_silence_and_a_fast_one_reads_at_twenty_characters_a_second() {
    let (p, sp) = project_with_words("caption.clean", &[("Okay.", 0.0, 0.2), ("Next", 3.0, 3.3)]);
    let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
    assert!(pages.pages[0].end_us >= s(0.833), "held to minPageMs");
    assert!(pages.pages[0].end_us <= pages.pages[1].start_us);
    // 61 columns at 20 a second: held 3.05 s, though its words end at 1.75 s and minPageMs is 0.833 s.
    let fast = "This line is said much faster than anyone could read it here!";
    let words: Vec<(&str, f64, f64)> = fast
        .split_whitespace()
        .enumerate()
        .map(|(i, t)| (t, i as f64 * 0.15, i as f64 * 0.15 + 0.1))
        .chain([("Next", 9.0, 9.3)])
        .collect();
    let (p, sp) = project_with_words("caption.clean", &words);
    let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
    assert_eq!(texts(pages)[0], fast, "one page over two lines");
    assert_eq!(pages.pages[0].end_us, s(61.0 / 20.0));
}

#[test]
fn a_frame_finds_its_page_by_binary_search() {
    let (p, sp) = project_with_words(
        "caption.bold",
        &words_at(0.0, "one two three four five six seven"),
    );
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    assert_eq!(page_at(pages, s(0.05)), Some(0));
    assert_eq!(page_at(pages, s(1.0)), Some(1));
    assert_eq!(page_at(pages, s(100.0)), None);
}

/// Clean has no `maxWords`: a run of one-letter words fills its two lines with up to 42 words, but a page stops at
/// `MAX_PAGE_WORDS`, which the renderer's fixed-capacity layout holds.
#[test]
fn a_page_holds_at_most_thirty_two_words() {
    let letters = vec!["a"; 40].join(" ");
    let words: Vec<(&str, f64, f64)> = letters
        .split(' ')
        .enumerate()
        .map(|(i, t)| (t, i as f64 * 0.1, i as f64 * 0.1 + 0.08))
        .collect();
    let (p, sp) = project_with_words("caption.clean", &words);
    let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
    let sizes: Vec<u32> = pages.pages.iter().map(|p| p.words.1 - p.words.0).collect();
    assert_eq!(sizes, [MAX_PAGE_WORDS, 8]);
}
