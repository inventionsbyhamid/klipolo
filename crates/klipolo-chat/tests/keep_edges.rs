//! `keepOnly`'s parts held to where speech breaks: an edge the model chose by what was said that falls inside a run of
//! speech moves to the nearest break within 1.5 s (a gap of 0.4 s or more, or a clip's edge), and a short fragment
//! after a long pause at a part's end goes with the pause. The project is made here: "talk.mov", 40 s, played whole,
//! its words written as a script (`said`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use klipolo_chat::{
    AnalysisState, Compiled, FunctionCall, Inputs, Library, Refs, ToolCall, View, compile,
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
        words.push(Word {
            text: token.into(),
            start_us: secs(start),
            end_us: secs(start + 0.3),
            confidence: 0.9,
            kind: WordKind::Word,
        });
        at = start + 0.3;
        gap = 0.1;
    }
    words
}

/// The words of `script` by text: where each starts and ends.
fn times(words: &[Word], text: &str) -> (f64, f64) {
    let w = words
        .iter()
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

fn inputs(p: &Project, words: Vec<Word>, message: &str) -> Inputs {
    let mut speech = Speech::default();
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
    Inputs {
        message: message.into(),
        view: View::default(),
        refs: Refs::of(p),
        speech,
        scenes: HashMap::new(),
        analysis: HashMap::from([("talk".to_owned(), AnalysisState::Done)]),
        time_again: HashSet::new(),
        cursor: None,
        fonts: Default::default(),
        library: Library::default(),
    }
}

/// The source ranges `keepOnly` of `ranges` (timeline seconds) keeps, in seconds.
fn kept(words: Vec<Word>, message: &str, ranges: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let p = project();
    let parts: Vec<_> = (ranges.iter())
        .map(|&(a, b)| json!({"startSec": a, "endSec": b}))
        .collect();
    let call = ToolCall::parse(&FunctionCall {
        name: "keepOnly".into(),
        call_id: "call_1".into(),
        arguments: json!({ "ranges": parts }).to_string(),
    })
    .unwrap();
    let out: Compiled = compile(&p, &inputs(&p, words, message), &[call]);
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let after = &out.proposal.as_ref().expect("a proposal").review.after;
    (after.timeline.main.iter())
        .map(|c| (c.source_in_us as f64 / 1e6, c.source_out_us as f64 / 1e6))
        .collect()
}

/// A long answer: a breath after w8 and after w20, then a long pause and the moderator's first words.
const ANSWER: &str = "w1 w2 w3 w4 w5 w6 w7 w8 |0.6 w9 w10 w11 w12 w13 w14 w15 w16 w17 w18 w19 w20 |0.6 \
                      w21 w22 w23 w24 |1.8 Our next question";

#[test]
fn a_start_inside_a_phrase_moves_back_to_the_breath_before_it() {
    let words = said(ANSWER);
    let (w9, w11, w24) = (
        times(&words, "w9"),
        times(&words, "w11"),
        times(&words, "w24"),
    );
    let w8 = times(&words, "w8");
    // The model starts on w11, two words into the phrase that starts at w9.
    let parts = kept(words, "", &[(w11.0, w24.1)]);
    assert_eq!(parts.len(), 1, "{parts:?}");
    let (start, _) = parts[0];
    assert!(
        start <= w9.0 && start > w8.1,
        "starts at the breath before w9: {start} ({w9:?})"
    );
}

#[test]
fn an_end_inside_a_phrase_moves_on_to_the_breath_after_it() {
    let words = said(ANSWER);
    let (w1, w18, w20, w21) = (
        times(&words, "w1"),
        times(&words, "w18"),
        times(&words, "w20"),
        times(&words, "w21"),
    );
    // The model ends on w18; the phrase runs on to w20, 0.8 s later.
    let parts = kept(words, "", &[(w1.0, w18.1)]);
    let (_, end) = parts[0];
    assert!(
        end >= w20.1 && end < w21.0,
        "ends at the breath after w20: {end} ({w20:?})"
    );
}

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
        let parts = kept(words.clone(), "", &[(w21.0, end)]);
        let (_, kept_end) = parts[0];
        assert!(
            kept_end >= w24.1 && kept_end < w24.1 + 0.5,
            "ends on w24, not {end}: {kept_end} ({w24:?})"
        );
    }
}

#[test]
fn an_edge_with_no_break_near_stays_where_the_words_put_it() {
    // 40 words said without a breath: no break within 1.5 s of w20.
    let script: Vec<String> = (1..=40).map(|i| format!("x{i}")).collect();
    let words = said(&script.join(" "));
    let (x20, x30) = (times(&words, "x20"), times(&words, "x30"));
    let parts = kept(words, "", &[(x20.0, x30.1)]);
    let (start, end) = parts[0];
    assert!((start - x20.0).abs() < 0.1, "{start} ({x20:?})");
    assert!((end - x30.1).abs() < 0.1, "{end} ({x30:?})");
}

#[test]
fn times_the_person_typed_are_kept_as_typed() {
    let words = said(ANSWER);
    let (w11, w18) = (times(&words, "w11"), times(&words, "w18"));
    let message = format!("keep {:.1} to {:.1}", w11.0, w18.1);
    let parts = kept(words, &message, &[(w11.0, w18.1)]);
    let (start, end) = parts[0];
    assert!((start - w11.0).abs() < 0.11, "{start} ({w11:?})");
    assert!((end - w18.1).abs() < 0.11, "{end} ({w18:?})");
}
