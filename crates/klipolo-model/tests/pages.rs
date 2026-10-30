//! Caption pages: each style's paging rules, keywords, the person's looks and the
//! style's casing.

use klipolo_model::captions::looks::{Look, WordLook};
use klipolo_model::captions::pages::{captions, Keyword, Pages};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::secs_to_us as s;

mod common;
use common::speech::{project_with_words, words_at};

/// What each page shows: its words that aren't hidden (hidden words stay in the pages, marked, for the word list),
/// a space between two where the page's text has one (none between Japanese words), a line break read as a space.
fn texts(pages: &Pages) -> Vec<String> {
    pages
        .pages
        .iter()
        .map(|p| pages.shown_text(p).replace('\n', " "))
        .collect()
}

fn look(start_us: i64, end_us: i64, look: Look) -> WordLook {
    WordLook {
        asset_id: "v".into(),
        start_us,
        end_us,
        look,
    }
}

#[test]
fn bold_pages_hold_three_upper_case_words_and_break_at_sentences() {
    let (p, sp) = project_with_words(
        "caption.bold",
        &words_at(0.0, "so the budget has three columns. date and amount"),
    );
    let c = captions(&p, &sp);
    assert_eq!(
        texts(&c.pages[&FormatKind::Portrait]),
        ["SO THE BUDGET", "HAS THREE COLUMNS", "DATE AND AMOUNT"]
    );
}

#[test]
fn clean_keeps_case_and_punctuation_on_two_lines() {
    let (p, sp) = project_with_words(
        "caption.clean",
        &words_at(
            0.0,
            "So the budget has three columns: date, category and amount.",
        ),
    );
    let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
    assert_eq!(texts(pages)[0], "So the budget has three columns:");
}

#[test]
fn a_page_never_crosses_a_cut() {
    use klipolo_model::ops::{Edge, Op};

    let (p, sp) = project_with_words(
        "caption.karaoke",
        &words_at(0.0, "one two three four five six"),
    );
    // Split the clip between "three" (0.6–0.85 s) and "four" (0.9 s), then trim 10 ms off the second clip's start: the
    // footage jumps there, a real cut, so two pages.
    let split = klipolo_model::apply_ops(&p, &[Op::Split { at_us: s(0.87) }]).unwrap();
    let second = split.timeline.main[1].id.clone();
    let cut = klipolo_model::apply_ops(
        &split,
        &[Op::TrimClip {
            id: second,
            edge: Edge::Start,
            source_us: s(0.88),
        }],
    )
    .unwrap();
    let pages = &captions(&cut, &sp).pages[&FormatKind::Landscape];
    assert_eq!(texts(pages), ["one two three", "four five six"]);
    // The split left in place is no cut: the footage goes on, and so does the page.
    let pages = &captions(&split, &sp).pages[&FormatKind::Landscape];
    assert_eq!(texts(pages), ["one two three four five six"]);
}

#[test]
fn hidden_words_never_show_and_a_line_break_starts_a_line() {
    let (mut p, sp) = project_with_words(
        "caption.boxed",
        &words_at(0.0, "um so this is how I budget"),
    );
    let um = &sp.transcripts["v"].words[0];
    let so = &sp.transcripts["v"].words[1];
    let how = &sp.transcripts["v"].words[4];
    p.word_looks = vec![
        look(
            um.start_us,
            so.start_us,
            Look {
                hidden: Some(true),
                ..Look::default()
            },
        ),
        look(
            how.start_us,
            how.end_us,
            Look {
                line_break: Some(true),
                ..Look::default()
            },
        ),
    ]
    .into();
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    assert_eq!(texts(pages)[0], "so this is how I budget");
    assert_eq!(
        pages.shown_text(&pages.pages[0]),
        "so this is\nhow I budget"
    );
    let first = &pages.pages[0];
    let all: Vec<(bool, bool)> = (first.words.0..first.words.1)
        .map(|i| {
            (
                pages.words[i as usize].hidden,
                pages.words[i as usize].line_break,
            )
        })
        .collect();
    // "um" stays in the page, marked hidden, so the word list can show it struck through and unhide it.
    assert_eq!(
        all,
        [
            (true, false),
            (false, false),
            (false, false),
            (false, false),
            (false, true),
            (false, false),
            (false, false)
        ]
    );
}

#[test]
fn hidden_words_never_start_a_page_or_take_time() {
    let (mut p, sp) = project_with_words(
        "caption.boxed",
        &words_at(0.0, "um so this is how I budget"),
    );
    let words = &sp.transcripts["v"].words;
    let hide = |end_us| {
        look(
            words[0].start_us,
            end_us,
            Look {
                hidden: Some(true),
                ..Look::default()
            },
        )
    };
    p.word_looks = vec![hide(words[1].start_us)].into();
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    assert_eq!(
        pages.pages[0].start_us, words[1].start_us,
        "the page shows when \"so\" is said, not during the hidden \"um\""
    );
    // Every word hidden: the page stays for the word list but takes no time and never draws.
    p.word_looks = vec![hide(words.last().unwrap().end_us)].into();
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    assert!(!pages.pages.is_empty() && pages.pages.iter().all(|pg| pg.start_us == pg.end_us));
    assert!(klipolo_model::captions::page_at(pages, words[1].start_us).is_none());
}

#[test]
fn looks_follow_their_words_through_cuts_and_undo() {
    // A look on "budget" stays on that word when the clip before it is cut, and again after the cut is undone.
    let (mut p, sp) = project_with_words(
        "caption.bold",
        &words_at(0.0, "first we plan then we budget"),
    );
    let budget = &sp.transcripts["v"].words[5];
    p.word_looks = vec![look(
        budget.start_us,
        budget.end_us,
        Look {
            color: Some("#39E508".into()),
            ..Look::default()
        },
    )]
    .into();
    let look_of = |p: &Project| {
        let pages = &captions(p, &sp).pages[&FormatKind::Portrait];
        pages
            .words
            .iter()
            .find(|w| pages.text_of(w) == "BUDGET")
            .map(|w| pages.look(w).color.clone())
    };
    let mut h = klipolo_model::history::Document::new(p.clone()).unwrap();
    h.apply(&klipolo_model::ops::Transaction {
        label: "edit.cut".into(),
        ops: vec![klipolo_model::ops::Op::DeleteRanges {
            ranges: vec![klipolo_model::time::RangeUs {
                start: 0,
                end: s(0.55),
            }],
        }],
        ..Default::default()
    })
    .unwrap();
    assert!(
        !texts(&captions(h.project(), &sp).pages[&FormatKind::Portrait])[0].starts_with("FIRST"),
        "the cut took \"first we\""
    );
    assert_eq!(
        look_of(h.project()),
        Some(Some("#39E508".into())),
        "after the cut"
    );
    h.undo().unwrap();
    assert_eq!(
        look_of(h.project()),
        Some(Some("#39E508".into())),
        "after the undo"
    );
}

#[test]
fn keywords_pick_one_number_or_power_word_per_page() {
    let (p, sp) = project_with_words(
        "caption.bold",
        &words_at(0.0, "I made $5,000 in three days. never do this"),
    );
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    let keyed: Vec<(String, Keyword)> = (0..)
        .zip(pages.words.iter())
        .filter_map(|(i, w)| pages.keyword(i).map(|k| (pages.text_of(w).to_owned(), k)))
        .collect();
    assert_eq!(
        keyed,
        [
            ("$5,000".to_owned(), Keyword::Gain),
            ("NEVER".to_owned(), Keyword::Loss)
        ]
    );
    assert_eq!(
        pages.pages.iter().map(|p| p.keyword).collect::<Vec<_>>(),
        [true, false, true]
    );
}

#[test]
fn the_persons_emphasis_wins_both_ways() {
    let (mut p, sp) =
        project_with_words("caption.bold", &words_at(0.0, "never give up on budgets"));
    let never = &sp.transcripts["v"].words[0];
    let budgets = &sp.transcripts["v"].words[4];
    let emphasis = |on| Look {
        emphasis: Some(on),
        ..Look::default()
    };
    p.word_looks = vec![
        look(never.start_us, never.end_us, emphasis(false)),
        look(budgets.start_us, budgets.end_us, emphasis(true)),
    ]
    .into();
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    let keyed: Vec<&str> = (0..)
        .zip(pages.words.iter())
        .filter(|&(i, _)| pages.keyword(i).is_some())
        .map(|(_, w)| pages.text_of(w))
        .collect();
    assert_eq!(keyed, ["BUDGETS"]);
}

#[test]
fn a_look_survives_a_style_switch() {
    let (mut p, sp) = project_with_words("caption.bold", &words_at(0.0, "this is free"));
    let free = &sp.transcripts["v"].words[2];
    p.word_looks = vec![look(
        free.start_us,
        free.end_us,
        Look {
            color: Some("#39E508".into()),
            ..Look::default()
        },
    )]
    .into();
    p.captions.style = Some(klipolo_model::contrib::ContribId::builtin(
        "caption.karaoke",
    ));
    let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
    let w = pages
        .words
        .iter()
        .find(|w| pages.text_of(w) == "free")
        .unwrap();
    assert_eq!(pages.look(w).color.as_deref(), Some("#39E508"));
}

#[test]
fn cjk_counts_two_columns_and_joins_unspaced() {
    let (p, sp) = project_with_words(
        "caption.boxed",
        &words_at(0.0, "今日 は 予算 の 話 を します"),
    );
    let pages = &captions(&p, &sp).pages[&FormatKind::Portrait];
    assert_eq!(
        texts(pages)[0].replace(' ', ""),
        texts(pages)[0],
        "no spaces between Japanese words"
    );
    assert!(pages.text.chars().all(|c| c != ' '));
    // 22 columns: one page in portrait's 24, two lines' worth at a 10-column limit would page them apart.
    assert_eq!(texts(pages), ["今日は予算の話をします"]);
}

/// Pairs of words as said, paged in Clean, Boxed and Bold for the parity fixture: particles, Latin words carrying a
/// Chinese or Japanese mark, marks and emoji at either edge, full-width letters and digits, a sign before a digit and
/// Hangul.
const JOIN_PAIRS: [[&str; 2]; 26] = [
    ["今日", "は"],
    ["iPhone", "の"],
    ["iPhone、", "カメラ"],
    ["Mac。", "明日"],
    ["天気.", "今日"],
    ["今日", "「OK"],
    ["「OK", "今日"],
    ["今天，", "我们"],
    ["「カメラ」", "を"],
    ["今日！", "明日"],
    ["今日?", "明日"],
    ["OK!", "今日"],
    ["今日", "，iPhone"],
    ["学校。", "iPhone"],
    ["今日", "-5"],
    ["今日", "．5"],
    ["今日", "ＡＩ"],
    ["今日", "ａｉ"],
    ["今日", "５"],
    ["今日", "ｶﾒﾗ"],
    ["今日", "“OK”"],
    ["한국", "今日"],
    ["今日😀", "明日"],
    ["今日", "😀明日"],
    ["iphone，", "拍"],
    ["the", "cat"],
];

/// The fixture "Fix the words" is checked against (`src/features/captions/wordFix.parity.test.ts`
/// `caption-place-parity.json`: the scripts written without spaces (`joins_unspaced`, as its runs over every
/// code point), and pairs of words paged in Clean, Boxed and Bold with what each shows and whether the second is drawn
/// right after the first (no space before it in the pages' text). So a fix's edges are judged as the captions draw
/// them, in every style. And the table the app reads (`src/lib/caption-edge-kept.json`): the
/// characters a style that shows only `?` and `!` keeps at a word's edge (`edge_kept`, through `shown`), as runs over
/// every code point, from Rust's own Unicode tables, so "Fix the words" trims as the captions do whatever Unicode the
/// webview knows (and no copy of the rule can drift). Rewrite both with `KLIPOLO_UPDATE_PARITY=1`.
#[test]
fn caption_join_parity_fixture_is_current() {
    use klipolo_model::captions::shown;
    use klipolo_model::captions::style::{Punctuation, TextCase};
    use klipolo_model::speech::joins_unspaced;

    #[derive(serde::Serialize)]
    struct Pair {
        style: &'static str,
        said: [&'static str; 2],
        shown: [String; 2],
        joined: bool,
    }
    #[derive(serde::Serialize)]
    struct Fixture {
        unspaced: Vec<[u32; 2]>,
        pairs: Vec<Pair>,
    }
    #[derive(serde::Serialize)]
    struct EdgeKept {
        /// The Unicode of Rust's tables the letters and digits come from.
        unicode: String,
        kept: Vec<[u32; 2]>,
    }

    /// The code points `is` holds for, as runs `[first, last]`.
    fn runs(mut is: impl FnMut(char) -> bool) -> Vec<[u32; 2]> {
        let mut out: Vec<[u32; 2]> = Vec::new();
        for c in (0..=0x10_FFFFu32).filter(|&c| char::from_u32(c).is_some_and(&mut is)) {
            match out.last_mut() {
                Some(run) if run[1] + 1 == c => run[1] = c,
                _ => out.push([c, c]),
            }
        }
        out
    }
    let unspaced = runs(joins_unspaced);
    let mut one = String::new();
    let kept = runs(|c| {
        one.clear();
        one.push(c);
        !shown(&one, TextCase::AsSaid, Punctuation::QuestionAndExclamation).is_empty()
    });
    let mut pairs = Vec::new();
    for style in ["clean", "boxed", "bold"] {
        for [a, b] in JOIN_PAIRS {
            let (p, sp) = project_with_words(
                &format!("caption.{style}"),
                &words_at(0.0, &format!("{a} {b}")),
            );
            let pages = &captions(&p, &sp).pages[&FormatKind::Landscape];
            let [first, second] = &pages.words[..] else {
                panic!("{style}: {a} {b} pages {} words", pages.words.len());
            };
            pairs.push(Pair {
                style,
                said: [a, b],
                shown: [pages.text_of(first).into(), pages.text_of(second).into()],
                joined: second.text.0 == first.text.1,
            });
        }
    }
    let (major, minor, update) = char::UNICODE_VERSION;
    let table = EdgeKept {
        unicode: format!("{major}.{minor}.{update}"),
        kept,
    };
    let lib = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../src/lib");
    for (name, text) in [
        (
            "caption-join-parity.json",
            serde_json::to_string_pretty(&Fixture { unspaced, pairs }).unwrap(),
        ),
        // One line: the app bundles it.
        (
            "caption-edge-kept.json",
            serde_json::to_string(&table).unwrap(),
        ),
    ] {
        let (path, text) = (lib.join(name), text + "\n");
        if std::env::var_os("KLIPOLO_UPDATE_PARITY").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            text,
            "src/lib/{name} is stale: run with KLIPOLO_UPDATE_PARITY=1"
        );
    }
}
