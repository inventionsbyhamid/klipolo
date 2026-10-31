//! The pages the Captions panel reads are the engine's: a window of the model's own pages, at most 200 per answer,
//! from one build kept while what pages are made from is unchanged; and "Fix all" finds every place a word shows.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_app::captions::{CaptionsCache, PagesQuery, WordView, answer, find};
use klipolo_engine::registry::Registry;
use klipolo_engine::session::ProjectFeed;
use klipolo_model::apply_ops;
use klipolo_model::captions::looks::{Look, WordLook};
use klipolo_model::contrib::ContribId;
use klipolo_model::ops::Op;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::speech::{Speech, Transcript, Word, WordFix, WordKind};
use klipolo_model::time::{RangeUs, secs_to_us as s};
use klipolo_model::timeline::{Clip, ClipSource};

/// The five styles the panel shows, by their builtin keys.
const SHOWN: [&str; 5] = ["clean", "bold", "karaoke", "highlight", "boxed"];

const ALL: PagesQuery = PagesQuery::Range {
    from_us: 0,
    to_us: i64::MAX,
};

fn word(text: &str, start: f64, end: f64, kind: WordKind) -> Word {
    Word {
        text: text.into(),
        start_us: s(start),
        end_us: s(end),
        confidence: 0.9,
        kind,
    }
}

/// `sentence`'s words from `start` s, 0.3 s apart, each 0.25 s long; "um" and "uh" heard as fillers.
fn words_at(start: f64, sentence: &str) -> Vec<Word> {
    sentence
        .split_whitespace()
        .enumerate()
        .map(|(i, t)| {
            let at = start + i as f64 * 0.3;
            let filler = matches!(t.trim_matches(',').to_lowercase().as_str(), "um" | "uh");
            let kind = if filler {
                WordKind::Filler
            } else {
                WordKind::Word
            };
            word(t, at, at + 0.25, kind)
        })
        .collect()
}

fn video(id: &str, secs: f64) -> Asset {
    Asset {
        id: id.into(),
        kind: AssetKind::Video,
        path: format!("/media/{id}"),
        name: id.into(),
        duration_us: s(secs),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
    }
}

fn transcript(words: Vec<Word>, language: &str) -> Arc<Transcript> {
    Arc::new(Transcript {
        key: "k".into(),
        language: language.into(),
        model: "m".into(),
        covered_us: s(3600.0 * 3.0),
        complete: true,
        words,
    })
}

/// File `v` (`secs` long) as one clip, captions on in `style` in landscape and portrait, and its transcript of `words`.
fn captioned(style: &str, secs: f64, words: Vec<Word>) -> (Project, Speech) {
    let mut p = Project::new("Captions");
    p.id = "p1".into();
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    p.assets = vec![video("v", secs)];
    let mut clip = Clip::new(ClipSource::Asset { id: "v".into() }, 0, s(secs));
    clip.id = "c1".into();
    p.timeline.main = vec![clip];
    p.captions.enabled = true;
    p.captions.style = Some(ContribId::builtin(&format!("caption.{style}")));
    let p = apply_ops(&p, &[]).unwrap();
    let speech = Speech {
        transcripts: HashMap::from([("v".to_owned(), transcript(words, "en"))]),
        ..Speech::default()
    };
    (p, speech)
}

/// Two sentences with a filler, 60 s of file.
fn talk(style: &str) -> (Project, Speech) {
    let mut words = words_at(1.0, "So the budget has three columns.");
    words.extend(words_at(4.0, "Um, date and amount, and the notes."));
    words.extend(words_at(
        8.0,
        "Teo asked me once why I keep the cracked ones.",
    ));
    words.extend(words_at(12.0, "Never. Maybe once for Teo."));
    captioned(style, 60.0, words)
}

fn feed(project: Project, speech: Speech, revision: u64) -> ProjectFeed {
    ProjectFeed {
        project_id: project.id.clone(),
        revision,
        opened: 1,
        project: Arc::new(project),
        speech: Arc::new(speech),
        text_language: Arc::from("en"),
        registry: Registry::builtin(),
    }
}

/// Every word on every page of `format`, as the panel gets them.
fn all_words(cache: &CaptionsCache, f: &ProjectFeed, format: FormatKind) -> Vec<WordView> {
    let view = answer(cache, Some(f), &f.project_id, format, ALL).unwrap();
    view.pages.into_iter().flat_map(|p| p.words).collect()
}

/// A page's words that show (hidden ones left out), joined by spaces.
fn shown(words: &[WordView]) -> String {
    let texts: Vec<&str> = words
        .iter()
        .filter(|w| w.look.hidden != Some(true))
        .map(|w| w.text.as_str())
        .collect();
    texts.join(" ")
}

#[test]
fn each_style_renders_a_page() {
    for style in SHOWN {
        let (p, sp) = talk(style);
        let f = feed(p, sp, 0);
        let cache = CaptionsCache::default();
        for format in [FormatKind::Landscape, FormatKind::Portrait] {
            let view = answer(&cache, Some(&f), "p1", format, ALL).unwrap();
            assert!(view.total > 0, "{style} {format:?} has no page");
            assert!(
                !view.pages[0].words.is_empty(),
                "{style} {format:?}'s first page has no word"
            );
            assert_eq!(view.pages.len() as u32, view.total);
        }
    }
}

#[test]
fn fix_word_changes_page_text() {
    let (mut p, sp) = talk("clean");
    let teo = sp.transcripts["v"]
        .words
        .iter()
        .find(|w| w.text == "Teo")
        .unwrap()
        .clone();
    p.word_fixes = Arc::new(vec![WordFix {
        asset_id: "v".into(),
        start_us: teo.start_us,
        end_us: teo.end_us,
        text: "Theo".into(),
    }]);
    let f = feed(p, sp, 1);
    let cache = CaptionsCache::default();
    let words = all_words(&cache, &f, FormatKind::Landscape);
    let fixed = words
        .iter()
        .find(|w| w.source_start_us == teo.start_us)
        .unwrap();
    assert_eq!(fixed.text, "Theo");
    assert_eq!(fixed.heard.as_deref(), Some("Teo"), "says what was heard");
    // The other "Teo." is untouched: a fix is one range of one file.
    assert!(words.iter().any(|w| w.text == "Teo."));
    assert!(words.iter().all(|w| w.heard.is_none() || w.text == "Theo"));
}

#[test]
fn hidden_word_not_on_page() {
    let (mut p, sp) = talk("boxed");
    let um = sp.transcripts["v"]
        .words
        .iter()
        .find(|w| w.text == "Um,")
        .unwrap()
        .clone();
    p.word_looks = Arc::new(vec![WordLook {
        asset_id: "v".into(),
        start_us: um.start_us,
        end_us: um.end_us,
        look: Look {
            hidden: Some(true),
            ..Look::default()
        },
    }]);
    let f = feed(p, sp, 1);
    let cache = CaptionsCache::default();
    let view = answer(&cache, Some(&f), "p1", FormatKind::Landscape, ALL).unwrap();
    let page = view
        .pages
        .iter()
        .find(|pg| pg.words.iter().any(|w| w.source_start_us == um.start_us))
        .expect("the hidden word stays in the list, to show it again");
    let hidden = page
        .words
        .iter()
        .find(|w| w.source_start_us == um.start_us)
        .unwrap();
    assert_eq!(hidden.look.hidden, Some(true));
    assert!(!shown(&page.words).contains("Um"), "{}", shown(&page.words));
    // The model's own page draws it neither.
    let model = klipolo_model::captions::captions(&f.project, &f.speech);
    let pages = &model.pages[&FormatKind::Landscape];
    let drawn: Vec<String> = pages.pages.iter().map(|pg| pages.shown_text(pg)).collect();
    assert!(drawn.iter().all(|t| !t.contains("Um")), "{drawn:?}");
}

#[test]
fn pages_never_cross_a_cut() {
    let (p, sp) = captioned(
        "karaoke",
        60.0,
        words_at(0.0, "one two three four five six"),
    );
    // A cut in the gap between "three" (0.6 to 0.85 s) and "four" (from 0.9 s): the words on each side of it never
    // share a page.
    let cut = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: s(0.87),
                end: s(0.88),
            }],
        }],
    )
    .unwrap();
    let f = feed(cut, sp, 1);
    let cache = CaptionsCache::default();
    let view = answer(&cache, Some(&f), "p1", FormatKind::Landscape, ALL).unwrap();
    let texts: Vec<String> = view.pages.iter().map(|pg| shown(&pg.words)).collect();
    assert_eq!(texts, ["one two three", "four five six"]);
    let four = view.pages[1].words.first().unwrap();
    assert_eq!(four.cut_before, Some(true), "a cut lies before it");
}

#[test]
fn a_cut_through_a_word_leaves_it_whole_on_one_side() {
    let (p, sp) = captioned(
        "karaoke",
        60.0,
        words_at(0.0, "one two three four five six"),
    );
    // Cut through "four" (0.9 to 1.15 s): it shows once, on the far side of the cut, as the whole word heard, so a
    // fix of it covers all of it.
    let cut = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: s(0.95),
                end: s(1.0),
            }],
        }],
    )
    .unwrap();
    let f = feed(cut, sp, 1);
    let cache = CaptionsCache::default();
    let view = answer(&cache, Some(&f), "p1", FormatKind::Landscape, ALL).unwrap();
    let texts: Vec<String> = view.pages.iter().map(|pg| shown(&pg.words)).collect();
    assert_eq!(texts, ["one two three", "four five six"]);
    let four = &view.pages[1].words[0];
    assert_eq!(
        (four.source_start_us, four.source_end_us),
        (s(0.9), s(1.15))
    );
    assert_eq!(four.cut_before, Some(true));
}

#[test]
fn a_word_names_its_range_in_its_files_sound_time() {
    // The file's sound starts 0.3 s after its picture: a word heard at 8 s in the sound plays at 8.3 s of the file.
    let (mut p, sp) = talk("clean");
    p.assets[0].audio_lead_us = Some(s(0.3));
    let p = apply_ops(&p, &[]).unwrap();
    let heard = sp.transcripts["v"]
        .words
        .iter()
        .find(|w| w.text == "Teo")
        .unwrap()
        .clone();
    let f = feed(p.clone(), sp.clone(), 1);
    let words = all_words(&CaptionsCache::default(), &f, FormatKind::Landscape);
    let teo = words.iter().find(|w| w.text == "Teo").unwrap();
    // Its range is the transcript's (what a fix names), its time on the timeline the file's.
    assert_eq!(
        (teo.source_start_us, teo.source_end_us),
        (heard.start_us, heard.end_us)
    );
    assert_eq!(teo.start_us, heard.start_us + s(0.3));
    // A fix sent at that range changes that word, and only it.
    let fixed = apply_ops(
        &p,
        &[Op::FixWords {
            asset_id: teo.asset_id.clone(),
            start_us: teo.source_start_us,
            end_us: teo.source_end_us,
            text: Some("Theo".into()),
        }],
    )
    .unwrap();
    let f = feed(fixed, sp, 2);
    let words = all_words(&CaptionsCache::default(), &f, FormatKind::Landscape);
    let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    assert!(texts.contains(&"Theo"), "{texts:?}");
    assert!(texts.contains(&"Teo."), "{texts:?}");
}

#[test]
fn the_pages_are_the_engines() {
    let (p, sp) = captioned(
        "bold",
        600.0,
        (0..60)
            .flat_map(|k| words_at(k as f64 * 5.0, "so the budget has three columns."))
            .collect(),
    );
    let f = feed(p, sp, 0);
    let cache = CaptionsCache::default();
    let query = PagesQuery::Index { from: 0, count: 10 };
    let got = answer(&cache, Some(&f), "p1", FormatKind::Landscape, query).unwrap();
    let want = klipolo_model::captions::captions(&f.project, &f.speech);
    let landscape = &want.pages[&FormatKind::Landscape];
    assert_eq!(got.total as usize, landscape.pages.len());
    assert!(got.total > 10 && got.pages.len() == 10);
    let first = &got.pages[0];
    assert_eq!(
        (first.index, first.start_us, first.end_us),
        (0, landscape.pages[0].start_us, landscape.pages[0].end_us)
    );
    assert_eq!(
        shown(&first.words),
        landscape.shown_text(&landscape.pages[0]).replace('\n', " ")
    );
    // A window of the timeline: the pages that overlap it, the first of them named by its index.
    let at = landscape.pages[5].start_us + 1;
    let range = PagesQuery::Range {
        from_us: at,
        to_us: at + 1,
    };
    let one = answer(&cache, Some(&f), "p1", FormatKind::Landscape, range).unwrap();
    assert_eq!((one.first, one.pages.len()), (5, 1));
    // One build for both answers; another project's id answers nothing, and no project at all forgets the build.
    assert_eq!(cache.builds(), 1);
    assert!(answer(&cache, Some(&f), "p2", FormatKind::Landscape, query).is_none());
    // New speech: built again.
    let again = ProjectFeed {
        speech: Arc::new((*f.speech).clone()),
        ..f.clone()
    };
    answer(&cache, Some(&again), "p1", FormatKind::Landscape, query).unwrap();
    assert_eq!(cache.builds(), 2);
    assert!(answer(&cache, None, "p1", FormatKind::Landscape, query).is_none());
}

#[test]
fn a_fix_reads_as_said_in_an_upper_case_style() {
    // Bold is upper case. A fix starts from what was said, never the style's capitals.
    let (p, sp) = talk("bold");
    let f = feed(p, sp, 0);
    let cache = CaptionsCache::default();
    let query = PagesQuery::Index { from: 0, count: 1 };
    let got = answer(&cache, Some(&f), "p1", FormatKind::Portrait, query).unwrap();
    let letters = |s: &str| -> String { s.chars().filter(|c| c.is_alphanumeric()).collect() };
    let word = &got.pages[0].words[0];
    assert_eq!(word.text, word.text.to_uppercase());
    let said = word.said.as_deref().expect("said otherwise than shown");
    assert_ne!(said, word.text);
    assert_ne!(said, said.to_uppercase(), "as said, not in capitals");
    assert_eq!(letters(&said.to_uppercase()), letters(&word.text));
}

#[test]
fn a_query_answers_at_most_200_pages() {
    // Two hours of talk.
    let words: Vec<Word> = (0..2400)
        .flat_map(|k| words_at(k as f64 * 3.0, "we plan the budget here"))
        .collect();
    let (p, sp) = captioned("bold", 7200.0, words);
    let f = feed(p, sp, 0);
    let cache = CaptionsCache::default();
    let got = answer(&cache, Some(&f), "p1", FormatKind::Portrait, ALL).unwrap();
    assert!(
        got.pages.len() == 200 && got.total > 1_000,
        "{} of {}",
        got.pages.len(),
        got.total
    );
    assert!(got.capped, "more pages than one answer holds");
    let tail = PagesQuery::Index {
        from: got.total - 3,
        count: 50,
    };
    let last = answer(&cache, Some(&f), "p1", FormatKind::Portrait, tail).unwrap();
    assert_eq!((last.first, last.pages.len()), (got.total - 3, 3));
    assert!(!last.capped);
}

#[test]
fn a_revision_that_pages_alike_keeps_the_build() {
    // A place or a colour doesn't page: the next ask reuses the build. A style does.
    let (p, sp) = talk("bold");
    let f = feed(p, sp, 0);
    let cache = CaptionsCache::default();
    let query = PagesQuery::Index { from: 0, count: 3 };
    let ask = |project: Project, revision: u64| {
        let next = ProjectFeed {
            project: Arc::new(project),
            revision,
            ..f.clone()
        };
        answer(&cache, Some(&next), "p1", FormatKind::Portrait, query).unwrap()
    };
    let first = ask((*f.project).clone(), f.revision);
    let mut placed = (*f.project).clone();
    placed.captions.place.insert(FormatKind::Portrait, 0.3);
    placed.captions.accent = Some("#FF0000".into());
    let again = ask(placed.clone(), f.revision + 1);
    assert_eq!(cache.builds(), 1, "kept");
    assert_eq!(
        (again.revision, &again.pages),
        (f.revision + 1, &first.pages)
    );
    let mut karaoke = placed;
    karaoke.captions.style = Some(ContribId::builtin("caption.karaoke"));
    ask(karaoke, f.revision + 2);
    assert_eq!(cache.builds(), 2, "a new style pages again");
}

#[test]
fn chinese_and_japanese_words_say_they_follow_without_a_space() {
    // A clip said in Japanese pages its words joined; English words keep their spaces.
    let (mut p, mut sp) = talk("clean");
    p.assets.push(video("ja", 10.0));
    let mut clip = Clip::new(ClipSource::Asset { id: "ja".into() }, 0, s(10.0));
    clip.id = "c2".into();
    p.timeline.main.push(clip);
    let p = apply_ops(&p, &[]).unwrap();
    let ja: Vec<Word> = ["今日", "は", "いい", "天気", "です。"]
        .iter()
        .enumerate()
        .map(|(i, t)| {
            word(
                t,
                1.0 + i as f64 * 0.3,
                1.25 + i as f64 * 0.3,
                WordKind::Word,
            )
        })
        .collect();
    sp.transcripts.insert("ja".into(), transcript(ja, "ja"));
    let f = feed(p, sp, 0);
    let cache = CaptionsCache::default();
    let got = answer(&cache, Some(&f), "p1", FormatKind::Landscape, ALL).unwrap();
    let ja = got
        .pages
        .iter()
        .find(|pg| pg.words[0].text == "今日")
        .expect("the Japanese page");
    assert!(
        ja.words.len() > 1 && ja.words[1..].iter().all(|w| w.joined),
        "{ja:?}"
    );
    let en = got
        .pages
        .iter()
        .find(|pg| pg.words.len() > 1 && pg.words[0].asset_id == "v")
        .expect("an English page");
    assert!(en.words[1..].iter().all(|w| !w.joined), "{en:?}");
}

#[test]
fn a_correction_split_in_place_is_fixed_whole() {
    // A correction whose words a split left in place now crosses reads its whole range, so fixing it again replaces
    // all of it (never just its first piece, which would bring the old tail back).
    let (mut p, sp) = talk("clean");
    let words = &sp.transcripts["v"].words;
    let k = words.iter().position(|w| w.text == "Teo").unwrap();
    let (first, middle, last) = (&words[k], &words[k + 1], &words[k + 2]);
    // Split the clip in place inside the middle word.
    let at = middle.start_us + (middle.end_us - middle.start_us) / 2;
    let mut tail = p.timeline.main[0].clone();
    tail.id = "c1b".into();
    tail.source_in_us = at;
    p.timeline.main[0].source_out_us = at;
    p.timeline.main.push(tail);
    p.word_fixes = Arc::new(vec![WordFix {
        asset_id: "v".into(),
        start_us: first.start_us,
        end_us: last.end_us,
        text: "Fixed three words".into(),
    }]);
    let p = apply_ops(&p, &[]).unwrap();
    let f = feed(p, sp.clone(), 0);
    let cache = CaptionsCache::default();
    let fixed = all_words(&cache, &f, FormatKind::Landscape)
        .into_iter()
        .find(|w| w.said.as_deref() == Some("Fixed three words") || w.text == "Fixed three words")
        .expect("the corrected words");
    assert_eq!(
        (fixed.source_start_us, fixed.source_end_us),
        (first.start_us, last.end_us)
    );
    // Its look is read at the middle of the piece it shows, inside the first clip.
    assert!(
        fixed
            .look_us
            .is_some_and(|us| first.start_us <= us && us < at),
        "{:?}",
        fixed.look_us
    );
}

#[test]
fn a_word_says_when_a_cut_lies_before_it() {
    // The clip split in place between "has" (1.9 to 2.15 s) and "three" (2.2 s): one shot, so one page, but two clips,
    // whose file time between may later play elsewhere. A fix must stop there.
    let (mut p, sp) = talk("clean");
    let mut tail = p.timeline.main[0].clone();
    tail.id = "c1b".into();
    tail.source_in_us = s(2.18);
    p.timeline.main[0].source_out_us = s(2.18);
    p.timeline.main.insert(1, tail);
    let p = apply_ops(&p, &[]).unwrap();
    let f = feed(p, sp, 0);
    let cache = CaptionsCache::default();
    let view = answer(&cache, Some(&f), "p1", FormatKind::Landscape, ALL).unwrap();
    let first = &view.pages[0].words;
    assert_eq!(shown(first), "So the budget has three columns.");
    let at = |src: f64| {
        first
            .iter()
            .find(|w| (w.source_start_us - s(src)).abs() < s(0.1))
            .unwrap_or_else(|| panic!("no word at {src} s"))
            .cut_before
    };
    assert_eq!(at(1.3), None, "the");
    assert_eq!(at(2.2), Some(true), "three");
    assert_eq!(first[0].cut_before, None, "the first word");
}

#[test]
fn a_filler_says_so() {
    let (p, sp) = talk("clean");
    let f = feed(p, sp, 0);
    let cache = CaptionsCache::default();
    let words = all_words(&cache, &f, FormatKind::Landscape);
    let um = words.iter().find(|w| w.text == "Um,").unwrap();
    assert_eq!(um.filler, Some(true));
    assert!(
        words
            .iter()
            .filter(|w| w.text != "Um,")
            .all(|w| w.filler.is_none())
    );
}

#[test]
fn find_names_each_place_a_word_shows_once() {
    // "Teo" twice ("Teo" and "Teo."), whatever the case and marks; a clip's copy plays the same file words, which one
    // fix covers, so they count once; a hidden one doesn't show.
    let (mut p, sp) = talk("bold");
    let mut copy = p.timeline.main[0].clone();
    copy.id = "copy".into();
    copy.source_in_us = s(7.5);
    copy.source_out_us = s(11.0);
    p.timeline.main.push(copy);
    let p = apply_ops(&p, &[]).unwrap();
    let f = feed(p.clone(), sp.clone(), 0);
    let cache = CaptionsCache::default();
    let found = find(&cache, Some(&f), "p1", FormatKind::Portrait, "teo,").unwrap();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .all(|w| w.said.as_deref().unwrap_or(&w.text).starts_with("Teo"))
    );
    assert!(found[0].start_us < found[1].start_us, "in timeline order");
    // Hidden: left out.
    let mut hid = p;
    let last = &found[1];
    hid.word_looks = Arc::new(vec![WordLook {
        asset_id: "v".into(),
        start_us: last.source_start_us,
        end_us: last.source_end_us,
        look: Look {
            hidden: Some(true),
            ..Look::default()
        },
    }]);
    let f = feed(hid, sp, 1);
    assert_eq!(
        find(&cache, Some(&f), "p1", FormatKind::Portrait, "Teo")
            .unwrap()
            .len(),
        1
    );
    assert!(find(&cache, Some(&f), "p2", FormatKind::Portrait, "Teo").is_none());
    assert!(
        find(&cache, Some(&f), "p1", FormatKind::Portrait, "...")
            .unwrap()
            .is_empty()
    );
}
