//! The compiler: the model's calls against the project snapshot its context came from, out as additions applied at
//! once and removals proposed. The projects are small and made here:
//! - `talk()`: "talk.mov", 30 s, played whole. Its words run from 0.5 to 7.8 s ("Um, so today we look at the new
//!   module. It took [pause 1.5 s] uh a long time."), then nothing is said.
//! - `broll(length)`: a file with no sound, played whole.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_chat::evidence::typed_times;
use klipolo_chat::{AnalysisState, Audience, FunctionCall, Inputs, Refs, ToolCall, View, compile};
use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::query::main_track_duration;
use klipolo_model::speech::{Speech, Transcript, Word, WordKind, spoken, word_cut};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn span(a: f64, b: f64) -> RangeUs {
    RangeUs {
        start: secs(a),
        end: secs(b),
    }
}

/// One call as the model wrote it, read by the tools' own reader.
fn call(name: &str, arguments: &str) -> ToolCall {
    ToolCall::parse(&FunctionCall {
        name: name.into(),
        call_id: "call_1".into(),
        arguments: arguments.into(),
    })
    .unwrap_or_else(|e| panic!("{name} {arguments}: {e}"))
}

fn cut(a: f64, b: f64) -> ToolCall {
    call(
        "cutRange",
        &format!(r#"{{"startSec": {a}, "endSec": {b}}}"#),
    )
}

fn file(id: &str, length: f64, has_audio: bool) -> Asset {
    Asset {
        id: id.into(),
        kind: AssetKind::Video,
        path: format!("/media/{id}.mov"),
        name: format!("{id}.mov"),
        duration_us: secs(length),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio,
        audio_lead_us: None,
    }
}

/// Clip `id` playing `from` to `to` seconds of `asset`.
fn clip(id: &str, asset: &str, from: f64, to: f64) -> Clip {
    let mut c = Clip::new(ClipSource::Asset { id: asset.into() }, secs(from), secs(to));
    c.id = id.into();
    c
}

/// A project at 30 fps, landscape.
fn project(assets: Vec<Asset>, clips: Vec<Clip>) -> Project {
    let mut p = Project::new("Test");
    p.assets = assets;
    p.timeline.main = clips;
    p
}

/// What `compile` reads besides the project: `message`, the refs of `project` as its context gave them, every file
/// analyzed, the playhead at 0 and no selection.
fn inputs(project: &Project, speech: Speech, message: &str) -> Inputs {
    Inputs {
        message: message.into(),
        view: View::default(),
        refs: Refs::of(project),
        speech,
        scenes: HashMap::new(),
        analysis: (project.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
    }
}

/// Words said one after another from 0.5 s, each 0.3 s long with 0.1 s between; `|1.5` makes the next gap 1.5 s.
/// "Um," and "uh" are fillers; a gap of 0.7 s or more holds a pause word, as the analysis writes them.
fn said(script: &str) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut gap = 0.1;
    let mut at = 0.4;
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
        let filler = matches!(token.to_lowercase().trim_end_matches(','), "um" | "uh");
        words.push(Word {
            text: token.into(),
            start_us: secs(start),
            end_us: secs(start + 0.3),
            confidence: 0.9,
            kind: if filler {
                WordKind::Filler
            } else {
                WordKind::Word
            },
        });
        at = start + 0.3;
        gap = 0.1;
    }
    words
}

fn transcript(words: Vec<Word>, covered: f64) -> Arc<Transcript> {
    Arc::new(Transcript {
        key: "key".into(),
        language: "en".into(),
        model: "test".into(),
        covered_us: secs(covered),
        complete: true,
        words,
    })
}

fn talk() -> (Project, Speech) {
    let p = project(
        vec![file("talk", 30.0, true)],
        vec![clip("t", "talk", 0.0, 30.0)],
    );
    let mut speech = Speech::default();
    speech.transcripts.insert(
        "talk".into(),
        transcript(
            said("Um, so today we look at the new module. It took |1.5 uh a long time."),
            30.0,
        ),
    );
    (p, speech)
}

fn broll(length: f64) -> Project {
    project(
        vec![file("broll", length, false)],
        vec![clip("b", "broll", 0.0, length)],
    )
}

/// The one refusal's reason, for call `index`, to the model.
fn refused_to_model(out: &klipolo_chat::Compiled, index: usize) -> &str {
    assert_eq!(out.refusals.len(), 1, "{:#?}", out.refusals);
    let r = &out.refusals[0];
    assert_eq!(
        (r.call_index, r.audience),
        (index, Audience::Model),
        "{r:?}"
    );
    assert!(!r.reason.contains(['\n', '\u{2013}', '\u{2014}']), "{r:?}");
    &r.reason
}

#[test]
fn invented_cut_is_refused_with_reason() {
    let (p, speech) = talk();
    let i = inputs(&p, speech, "cut the boring bit");
    // Nothing is said, shown or selected from 7.8 s to the end at 30.0 s.
    let out = compile(&p, &i, &[cut(17.0, 21.0)]);
    assert_eq!((&out.proposal, &out.additions), (&None, &None));
    let reason = refused_to_model(&out, 0);
    assert!(
        reason
            .starts_with("cutRange: startSec 17.0 is not within 0.3 s of a word, a pause, a scene"),
        "{reason}"
    );
    // Only the end is invented: the reason names it.
    let end = compile(&p, &i, &[cut(7.8, 21.0)]);
    assert!(
        refused_to_model(&end, 0).starts_with("cutRange: endSec 21.0 is not within 0.3 s of"),
        "{:?}",
        end.refusals
    );
    // Near what the context shows, a cut passes: "time." ends at 7.8 s and the video at 30.0 s.
    let near = compile(&p, &i, &[cut(7.9, 29.8)]);
    assert_eq!(near.refusals, vec![]);
    assert_eq!(
        near.proposal.expect("a proposal").ranges,
        vec![span(7.8, 30.0)]
    );
}

#[test]
fn cut_snaps_to_word_edges_with_padding() {
    let (p, speech) = talk();
    let i = inputs(&p, speech.clone(), "cut today we");
    // "today" is said from 1.3 to 1.6 s and "we" from 1.7 to 2.0 s; the model is a little off at both ends.
    let out = compile(&p, &i, &[cut(1.35, 1.95)]);
    assert_eq!(out.refusals, vec![]);
    let words = spoken(&p, &speech);
    let at = |text: &str| {
        (words.shown(&p, &speech))
            .position(|(_, t)| t == text)
            .unwrap()
    };
    let padded = word_cut(&words, at("today"), at("we"), 30);
    assert!(
        padded.start < secs(1.3) && padded.end > secs(2.0),
        "{padded:?} reaches past the words"
    );
    assert!(
        padded.start > secs(1.2) && padded.end < secs(2.1),
        "{padded:?} keeps the words around them"
    );
    let proposal = out.proposal.expect("a proposal");
    assert_eq!(
        proposal.tx.ops,
        vec![Op::DeleteRanges {
            ranges: vec![padded]
        }]
    );
    assert_eq!(proposal.ranges, vec![padded]);
    assert_eq!(proposal.removed_us, padded.len());
    assert_eq!(proposal.tx.origin, Some(Origin::Chat));
    assert_eq!(proposal.tx.label, "chat.cutRange");
    let params = proposal.tx.params.expect("the label's values");
    assert_eq!(params["startUs"], ErrorParam::Number(padded.start as f64));
    assert_eq!(params["endUs"], ErrorParam::Number(padded.end as f64));
}

#[test]
fn keep_only_proposes_the_complement() {
    let p = broll(30.0);
    let i = inputs(
        &p,
        Speech::default(),
        "keep only 2 to 5 seconds and from 10 to 12.5",
    );
    let out = compile(
        &p,
        &i,
        &[call(
            "keepOnly",
            r#"{"ranges": [{"startSec": 10, "endSec": 12.5}, {"startSec": 2.1, "endSec": 5}]}"#,
        )],
    );
    assert_eq!(out.refusals, vec![]);
    let proposal = out.proposal.expect("a proposal");
    assert_eq!(
        proposal.ranges,
        vec![span(0.0, 2.0), span(5.0, 10.0), span(12.5, 30.0)]
    );
    assert_eq!(proposal.removed_us, secs(24.5));
    assert_eq!(proposal.tx.label, "chat.keepOnly");
    assert_eq!(
        main_track_duration(&proposal.review.after.timeline.main),
        secs(5.5)
    );
}

#[test]
fn typed_times_count_as_evidence() {
    let d = secs(120.0);
    assert_eq!(
        typed_times("cut the first 5 seconds", d),
        vec![0, secs(5.0)]
    );
    assert_eq!(
        typed_times("trim the last 10 s", d),
        vec![secs(110.0), secs(120.0)]
    );
    assert_eq!(
        typed_times("from 1:05 to 1:10.5", d),
        vec![secs(65.0), secs(70.5)]
    );
    assert_eq!(
        typed_times("remove 12.5s to 20", d),
        vec![secs(12.5), secs(20.0)]
    );
    assert_eq!(typed_times("the first five seconds", d), vec![0, secs(5.0)]);
    assert_eq!(
        typed_times("cut the last minute", d),
        vec![secs(60.0), secs(120.0)]
    );
    assert_eq!(typed_times("keep 1.5 minutes", d), vec![secs(90.0)]);
    assert_eq!(typed_times("cut the boring part", d), Vec::<Us>::new());

    let p = broll(120.0);
    for (message, a, b, removed) in [
        ("cut the first 5 seconds", 0.0, 5.0, span(0.0, 5.0)),
        ("trim the last 10 s", 110.0, 120.0, span(110.0, 120.0)),
        ("cut from 1:05 to 1:10.5", 65.04, 70.5, span(65.0, 70.5)),
        ("cut the first five seconds", 0.0, 5.1, span(0.0, 5.0)),
    ] {
        let out = compile(&p, &inputs(&p, Speech::default(), message), &[cut(a, b)]);
        assert_eq!(out.refusals, vec![], "{message}");
        assert_eq!(out.proposal.expect(message).ranges, vec![removed]);
    }
    // Without a time in the message, the same cut is invented.
    let none = compile(
        &p,
        &inputs(&p, Speech::default(), "cut the boring part"),
        &[cut(65.0, 70.5)],
    );
    refused_to_model(&none, 0);
    // The playhead, and the selection, exactly.
    let mut i = inputs(&p, Speech::default(), "cut from here to 40");
    i.view.playhead_us = secs(33.33);
    let out = compile(&p, &i, &[cut(33.3, 40.0)]);
    assert_eq!(out.proposal.expect("a cut").ranges, vec![span(33.33, 40.0)]);
    i.message = "cut this".into();
    i.view.selection = Some(span(50.02, 61.47));
    let out = compile(&p, &i, &[cut(50.0, 61.5)]);
    assert_eq!(
        out.proposal.expect("a cut").ranges,
        vec![span(50.02, 61.47)]
    );
}

#[test]
fn two_cuts_in_one_answer_remove_exactly_the_named_ranges() {
    let p = broll(60.0);
    let i = inputs(&p, Speech::default(), "cut 10 to 12 and 20 to 22");
    let out = compile(&p, &i, &[cut(10.0, 12.0), cut(20.0, 22.0)]);
    assert_eq!(out.refusals, vec![]);
    let proposal = out.proposal.expect("a proposal");
    // The second cut follows the footage: once the first is gone it plays at 18 to 20 s.
    assert_eq!(
        proposal.tx.ops,
        vec![
            Op::DeleteRanges {
                ranges: vec![span(10.0, 12.0)]
            },
            Op::DeleteRanges {
                ranges: vec![span(18.0, 20.0)]
            },
        ]
    );
    assert_eq!(proposal.ranges, vec![span(10.0, 12.0), span(20.0, 22.0)]);
    assert_eq!(proposal.removed_us, secs(4.0));
    assert_eq!(proposal.tx.label, "chat.andMore");
}

#[test]
fn overlapping_cuts_are_removed_once() {
    let p = broll(60.0);
    let i = inputs(&p, Speech::default(), "cut 10 to 14 and 12 to 16");
    let out = compile(&p, &i, &[cut(10.0, 14.0), cut(12.0, 16.0)]);
    assert_eq!(out.refusals, vec![]);
    let proposal = out.proposal.expect("a proposal");
    assert_eq!(proposal.ranges, vec![span(10.0, 16.0)]);
    assert_eq!(proposal.removed_us, secs(6.0));
    // Only what is left of the second goes: 14 to 16 s, at 10 to 12 s by then.
    assert_eq!(
        proposal.tx.ops[1],
        Op::DeleteRanges {
            ranges: vec![span(10.0, 12.0)]
        }
    );
    // A cut an earlier call already made whole is said back to the model.
    let again = compile(
        &p,
        &inputs(&p, Speech::default(), "cut 10 to 14, and 11 to 13"),
        &[cut(10.0, 14.0), cut(11.0, 13.0)],
    );
    assert!(
        refused_to_model(&again, 1).contains("an earlier part of this answer already removes"),
        "{:?}",
        again.refusals
    );
    assert_eq!(
        again.proposal.expect("the first cut").ranges,
        vec![span(10.0, 14.0)]
    );
}
