//! `keepOnly`'s parts lose what trails after their last words, and nothing else: an end on a word or a pause a long
//! pause after the last word said ends at that word, and an end that takes one stray word after such a pause (the
//! next speaker's first word, said into their sentence) ends before the pause. Starts never move, an end never moves
//! on, and an end near a scene item, a clip's edge or a time the person typed stays. The project is made here:
//! "talk.mov", 40 s, played whole, its words written as a script (`said`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use klipolo_chat::{
    AnalysisState, Compiled, FunctionCall, Inputs, Library, Refs, SceneEntry, SceneLog, ToolCall,
    View, compile,
};
use klipolo_model::Project;
use klipolo_model::project::Asset;
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource};
use serde_json::json;

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

/// Words from a script: each word 0.3 s long, 0.1 s after the one before; `|0.6` makes the next gap 0.6 s. A gap of
/// 0.7 s or more holds a pause word, as a transcript has.
fn said(script: &str) -> Vec<Word> {
    let mut words = Vec::new();
    let (mut at, mut gap) = (0.4, 0.1);
    for token in script.split_whitespace() {
        if let Some(g) = token.strip_prefix('|') {
            gap = g.parse().unwrap();
            continue;
        }
        let start = at + gap;
        if gap >= 0.7 {
            words.push(Word {
                text: String::new(),
                start_us: secs(at),
                end_us: secs(start),
                confidence: 1.0,
                kind: WordKind::Pause,
            });
        }
        words.push(word(token, start, start + 0.3));
        at = start + 0.3;
        gap = 0.1;
    }
    words
}

fn word(text: &str, start: f64, end: f64) -> Word {
    Word {
        text: text.into(),
        start_us: secs(start),
        end_us: secs(end),
        confidence: 0.9,
        kind: WordKind::Word,
    }
}

/// Where the word `text` starts and ends.
fn times(words: &[Word], text: &str) -> (f64, f64) {
    let w = (words.iter())
        .find(|w| w.text == text)
        .unwrap_or_else(|| panic!("{text}"));
    (w.start_us as f64 / 1e6, w.end_us as f64 / 1e6)
}

fn project() -> Project {
    let mut p = Project::new("Talk");
    p.assets = vec![
        serde_json::from_value::<Asset>(json!({
            "id": "talk", "kind": "video", "path": "/media/talk.mov", "name": "talk.mov",
            "durationUs": secs(40.0), "width": 1920, "height": 1080, "fps": 30.0, "hasAudio": true,
        }))
        .unwrap(),
    ];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "talk".into() },
        0,
        secs(40.0),
    )];
    p
}

fn inputs(p: &Project, words: Vec<Word>, scenes: Vec<SceneEntry>, message: &str) -> Inputs {
    let mut speech = Speech::default();
    if !words.is_empty() {
        speech.transcripts.insert(
            "talk".into(),
            Arc::new(Transcript {
                key: "key".into(),
                language: "en".into(),
                model: "test".into(),
                covered_us: secs(40.0),
                complete: true,
                words,
            }),
        );
    }
    Inputs {
        turns: Default::default(),
        message: message.into(),
        view: View::default(),
        refs: Refs::of(p),
        speech,
        scenes: HashMap::from([("talk".to_owned(), SceneLog { items: scenes })]),
        analysis: HashMap::from([("talk".to_owned(), AnalysisState::Done)]),
        time_again: HashSet::new(),
        cursor: None,
        fonts: Default::default(),
        library: Library::default(),
    }
}

/// The source ranges `keepOnly` of `ranges` (timeline seconds) keeps on `p`, in seconds.
fn kept_on(p: &Project, inputs: &Inputs, ranges: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let parts: Vec<_> = (ranges.iter())
        .map(|&(a, b)| json!({"startSec": a, "endSec": b}))
        .collect();
    let call = ToolCall::parse(&FunctionCall {
        name: "keepOnly".into(),
        call_id: "call_1".into(),
        arguments: json!({ "ranges": parts }).to_string(),
    })
    .unwrap();
    let out: Compiled = compile(p, inputs, &[call]);
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let after = &out.proposal.as_ref().expect("a proposal").review.after;
    (after.timeline.main.iter())
        .map(|c| (c.source_in_us as f64 / 1e6, c.source_out_us as f64 / 1e6))
        .collect()
}

fn kept(words: Vec<Word>, ranges: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let p = project();
    kept_on(&p, &inputs(&p, words, Vec::new(), ""), ranges)
}

/// A long answer: a breath after w8 and after w20, then a long pause and the moderator's first words.
const ANSWER: &str = "w1 w2 w3 w4 w5 w6 w7 w8 |0.6 w9 w10 w11 w12 w13 w14 w15 w16 w17 w18 w19 w20 |0.6 \
                      w21 w22 w23 w24 |1.8 Our next question";

#[test]
fn the_next_speakers_first_word_after_a_long_pause_goes_with_the_pause() {
    let words = said(ANSWER);
    let (w21, w24, our) = (
        times(&words, "w21"),
        times(&words, "w24"),
        times(&words, "Our"),
    );
    // Ending on "Our", the moderator's first word after the 1.8 s pause, or on the pause's end.
    for end in [our.1, our.0] {
        let parts = kept(words.clone(), &[(w21.0, end)]);
        let (_, kept_end) = parts[0];
        assert!(
            kept_end >= w24.1 && kept_end < w24.1 + 0.5,
            "ends on w24, not {end}: {kept_end} ({w24:?})"
        );
    }
}

#[test]
fn starts_and_ends_inside_a_phrase_stay_where_the_model_put_them() {
    let words = said(ANSWER);
    let (w11, w18) = (times(&words, "w11"), times(&words, "w18"));
    let parts = kept(words, &[(w11.0, w18.1)]);
    let (start, end) = parts[0];
    assert!((start - w11.0).abs() < 0.1, "{start} ({w11:?})");
    assert!((end - w18.1).abs() < 0.1, "{end} ({w18:?})");
}

#[test]
fn an_end_at_a_quick_change_of_speaker_stays_on_the_speakers_last_word() {
    // A's long answer has no full stop; B answers 0.3 s later.
    let words = said("a1 a2 a3 a4 a5 a6 a7 a8 inspiring |0.3 Thank you. |0.6 b1 b2 b3 b4");
    let (a1, inspiring, thank) = (
        times(&words, "a1"),
        times(&words, "inspiring"),
        times(&words, "Thank"),
    );
    let parts = kept(words, &[(a1.0, inspiring.1)]);
    assert!(parts[0].1 < thank.0, "B's words stay out: {parts:?}");
}

#[test]
fn a_start_after_so_and_a_false_start_left_out_stay_out() {
    // The recipe asks never to start on "so", and to leave out false starts.
    let words = said("w1 w2 done. |0.5 So we went flying today. |0.6 x1 x2 x3");
    let (so, we, today) = (
        times(&words, "So"),
        times(&words, "we"),
        times(&words, "today."),
    );
    let parts = kept(words, &[(we.0, today.1)]);
    assert!(parts[0].0 > so.1 - 0.1, "So stays out: {parts:?}");
    let words = said("Intro words here. |0.5 I think I2 think2 the answer is simple. |0.6 x1 x2");
    let (i1, i2, simple) = (
        times(&words, "I"),
        times(&words, "I2"),
        times(&words, "simple."),
    );
    let parts = kept(words, &[(i2.0, simple.1)]);
    assert!(parts[0].0 > i1.1, "the false start stays out: {parts:?}");
}

#[test]
fn an_end_before_and_does_not_move_on_to_end_on_and() {
    let words = said("w1 w2 w3 it was amazing and |0.6 then we left. |0.6 x1 x2");
    let (w1, amazing, and) = (
        times(&words, "w1"),
        times(&words, "amazing"),
        times(&words, "and"),
    );
    let parts = kept(words, &[(w1.0, amazing.1)]);
    assert!(parts[0].1 < and.0 + 0.05, "ends before and: {parts:?}");
}

#[test]
fn a_speakers_own_last_words_after_a_pause_stay() {
    // A sentence's end after a dramatic pause, and a last word their speech stops after.
    for script in [
        "w1 w2 the one thing that matters |1.2 family. |0.6 x1 x2",
        "w1 w2 the one thing that matters |1.2 family |0.8 x1 x2",
        "w1 w2 the one thing that matters |1.2 is family because x1 x2",
    ] {
        let words = said(script);
        let (w1, family) = (
            times(&words, "w1"),
            (words.iter())
                .find(|w| w.text.starts_with("family"))
                .map(|w| w.end_us as f64 / 1e6)
                .unwrap(),
        );
        let parts = kept(words, &[(w1.0, family)]);
        assert!(
            parts[0].1 >= family - 0.01,
            "{script}: keeps family: {parts:?}"
        );
    }
}

#[test]
fn a_part_ending_where_a_scene_ends_keeps_what_is_seen() {
    // "okay watch this", then 10 s of the jump with nobody talking, ending where Scenes says the jump ends.
    let words = said("okay watch this |20.0 later words here");
    let okay = times(&words, "okay");
    let p = project();
    let jump = SceneEntry {
        start_us: secs(1.5),
        end_us: secs(12.0),
        kind: "action".into(),
        who: "skater".into(),
        text: "the skater jumps the stairs and lands".into(),
        on_screen_text: None,
    };
    let parts = kept_on(&p, &inputs(&p, words, vec![jump], ""), &[(okay.0, 12.0)]);
    assert!(
        (parts[0].1 - 12.0).abs() < 0.05,
        "the jump stays: {parts:?}"
    );
}

#[test]
fn a_word_cut_off_by_a_clip_edge_is_not_said_into_the_next_clip() {
    // Clip 1 plays source 0 to 4.0, ending just after "Our" (a long pause before it); clip 2 starts at source 10.1, in
    // the middle of "y1": on the timeline "Our" and "y1" meet at the cut, 4.0 s.
    let words = vec![
        word("Start.", 0.5, 0.8),
        word("x1", 0.9, 1.2),
        word("x2.", 1.3, 1.6),
        word("Our", 3.5, 4.1),
        word("y1", 10.0, 10.4),
        word("y2", 10.5, 10.8),
        word("y3.", 10.9, 11.2),
    ];
    let mut p = project();
    p.timeline.main = vec![
        Clip::new(ClipSource::Asset { id: "talk".into() }, 0, secs(4.0)),
        Clip::new(
            ClipSource::Asset { id: "talk".into() },
            secs(10.1),
            secs(40.0),
        ),
    ];
    // The part ends at the cut, on "Our": the next words are another clip's, so "Our" doesn't run on into them (and
    // an end at a clip's edge stays anyway).
    let parts = kept_on(&p, &inputs(&p, words, Vec::new(), ""), &[(0.5, 4.0)]);
    assert!(
        (parts[0].1 - 4.0).abs() < 0.05,
        "keeps to the cut: {parts:?}"
    );
}

#[test]
fn times_the_person_typed_are_kept_as_typed() {
    let words = said(ANSWER);
    let (w21, our) = (times(&words, "w21"), times(&words, "Our"));
    let p = project();
    // The person typed the end; the model writes it a little off, on "Our".
    let message = format!("keep {:.1} to {:.1}", w21.0, our.1 + 0.1);
    let parts = kept_on(
        &p,
        &inputs(&p, words, Vec::new(), &message),
        &[(w21.0, our.1)],
    );
    assert!(parts[0].1 >= our.1 - 0.01, "ends where typed: {parts:?}");
}

#[test]
fn a_project_with_no_words_keeps_the_parts_as_given() {
    let p = project();
    let parts = kept_on(
        &p,
        &inputs(&p, Vec::new(), Vec::new(), "keep 0 to 12 s"),
        &[(0.0, 12.0)],
    );
    assert_eq!(parts, vec![(0.0, 12.0)]);
}
