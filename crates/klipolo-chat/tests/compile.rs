//! The compiler: the model's calls against the project snapshot its context came from, out as additions applied at
//! once and removals proposed. The projects are small and made here:
//! - `talk()`: "talk.mov", 30 s, played whole. Its words run from 0.5 to 7.8 s ("Um, so today we look at the new
//!   module. It took [pause 1.5 s] uh a long time."), then nothing is said.
//! - `broll(length)`: a file with no sound, played whole.
//! - `three_clips()`: one 60 s file in three clips, to test refs, trims and calls that follow one another.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_chat::evidence::typed_times;
use klipolo_chat::{
    ANALYZE_FIRST, AnalysisState, Audience, FunctionCall, Inputs, Job, Moment, Refs, Refusal,
    STILL_LISTENING, SceneEntry, SceneLog, ToolCall, View, compile,
};
use klipolo_model::captions::styles::BuiltinCaptionStyle;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::{Edge, Op, Origin};
use klipolo_model::project::{Asset, AssetKind, CaptionSettings, FormatKind, Settings};
use klipolo_model::query::main_track_duration;
use klipolo_model::speech::{Levels, Speech, Transcript, Word, WordKind, spoken, word_cut};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_model::{Document, Project, apply_ops};

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

fn trim(clip: &str, edge: &str, to: f64) -> ToolCall {
    call(
        "trimClip",
        &format!(r#"{{"clip": "{clip}", "edge": "{edge}", "toSec": {to}}}"#),
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

/// Loudness for a 30 s file, one value per 100 ms: speech everywhere but quiet from 10.0 to 10.8 s and from 15.0 to
/// 17.0 s.
fn levels() -> Arc<Levels> {
    let db: Vec<f32> = (0..300)
        .map(|w| {
            if (100..108).contains(&w) || (150..170).contains(&w) {
                -70.0
            } else {
                -20.0
            }
        })
        .collect();
    Arc::new(Levels { db: db.into() })
}

fn broll(length: f64) -> Project {
    project(
        vec![file("broll", length, false)],
        vec![clip("b", "broll", 0.0, length)],
    )
}

/// "long.mov" (60 s, no sound) in three clips: c1 plays its 0 to 10 s, c2 its 20 to 30 s and c3 its 40 to 50 s, at 0
/// to 10, 10 to 20 and 20 to 30 s on the timeline.
fn three_clips() -> Project {
    project(
        vec![file("long", 60.0, false)],
        vec![
            clip("a", "long", 0.0, 10.0),
            clip("b", "long", 20.0, 30.0),
            clip("c", "long", 40.0, 50.0),
        ],
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
        (words.words.iter())
            .position(|w| words.text(w, &p, &speech) == text)
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

/// Two files with speech: c1 plays talk.mov's first 10 s (words to 7.8 s), c2 all of more.mov (20 s), whose words
/// "Then we flew it up on a rocket." run from 10.5 s on the timeline: "it" ends at 12.0 s and "up" starts at 12.1 s.
fn two_talks() -> (Project, Speech) {
    let (mut p, mut speech) = talk();
    p.assets.push(file("more", 20.0, true));
    p.timeline.main = vec![clip("t", "talk", 0.0, 10.0), clip("m", "more", 0.0, 20.0)];
    speech.transcripts.insert(
        "more".into(),
        transcript(said("Then we flew it up on a rocket."), 20.0),
    );
    (p, speech)
}

#[test]
fn padding_stays_on_the_word_an_edge_matched() {
    let (p, speech) = two_talks();
    let words = spoken(&p, &speech);
    let at = |text: &str| {
        (words.words.iter())
            .position(|w| words.text(w, &p, &speech) == text)
            .unwrap()
    };
    let i = inputs(&p, speech.clone(), "cut from the end of her answer");
    // From the end of "time." in c1 (7.8 s) to the end of "it" in c2: the start stays on the word it matched, not
    // before c2's first word; only the end, on its own word's end, is padded.
    let across = compile(&p, &i, &[cut(7.8, 12.0)]);
    let it_end = word_cut(&words, at("it"), at("it"), 30).end;
    assert!(it_end > secs(12.0));
    assert_eq!(
        across.proposal.expect("a cut").ranges,
        vec![RangeUs {
            start: secs(7.8),
            end: it_end
        }]
    );
    // From the start of "It" in c1 to the start of "up" in c2: the start is padded, the end stays on "up".
    let to_start = compile(&p, &i, &[cut(4.1, 12.1)]);
    let it_start = word_cut(&words, at("It"), at("It"), 30).start;
    assert!(it_start < secs(4.1));
    assert_eq!(
        to_start.proposal.expect("a cut").ranges,
        vec![RangeUs {
            start: it_start,
            end: secs(12.1)
        }]
    );
    // Keeping that span keeps all of it: nothing of it is cut for padding the other way.
    let keep = compile(
        &p,
        &i,
        &[call(
            "keepOnly",
            r#"{"ranges": [{"startSec": 7.8, "endSec": 12.1}]}"#,
        )],
    );
    assert_eq!(
        keep.proposal.expect("the rest goes").ranges,
        vec![span(0.0, 7.8), span(12.1, 30.0)]
    );
}

#[test]
fn remove_fillers_proposes_padded_ranges() {
    let (p, speech) = talk();
    let out = compile(
        &p,
        &inputs(&p, speech.clone(), "remove the ums"),
        &[call("removeFillers", "{}")],
    );
    assert_eq!(out.refusals, vec![]);
    let words = spoken(&p, &speech);
    let expected: Vec<RangeUs> = (0..words.words.len())
        .filter(|&i| words.words[i].kind == WordKind::Filler)
        .map(|i| word_cut(&words, i, i, 30))
        .collect();
    assert_eq!(expected.len(), 2, "Um, and uh");
    // Each range holds its filler and a little around it: "uh" is said from 6.3 to 6.6 s.
    assert!(expected[1].start < secs(6.3) && expected[1].end > secs(6.6));
    let proposal = out.proposal.expect("a proposal");
    assert_eq!(
        proposal.tx.ops,
        vec![Op::DeleteRanges {
            ranges: expected.clone()
        }]
    );
    assert_eq!(proposal.ranges, expected);
    assert_eq!(proposal.review.removals.ranges, expected);
    assert_eq!(
        proposal.removed_us,
        expected.iter().map(RangeUs::len).sum::<Us>()
    );
    assert_eq!(proposal.tx.label, "chat.removeFillers");
    assert_eq!(out.additions, None, "a removal never applies at once");
}

#[test]
fn remove_silences_uses_levels_and_min_pause() {
    let (p, mut speech) = talk();
    speech.levels.insert("talk".into(), levels());
    let i = inputs(&p, speech, "remove the pauses");
    // Over 1.0 s: only the quiet from 15.0 to 17.0 s, a breath of 0.15 s kept at each end.
    let long = compile(&p, &i, &[call("removeSilences", r#"{"minPauseSec": 1.0}"#)]);
    assert_eq!(
        long.proposal.expect("a proposal").ranges,
        vec![span(15.15, 16.85)]
    );
    // No length given: pauses over 0.7 s, so the 0.8 s one goes too.
    let any = compile(
        &p,
        &i,
        &[call("removeSilences", r#"{"minPauseSec": null}"#)],
    );
    let proposal = any.proposal.expect("a proposal");
    assert_eq!(
        proposal.ranges,
        vec![span(10.15, 10.65), span(15.15, 16.85)]
    );
    assert_eq!(proposal.removed_us, secs(2.2));
    assert_eq!(proposal.tx.label, "chat.removeSilences");
    // No pause that long: told to the person, with the length asked.
    let none = compile(&p, &i, &[call("removeSilences", r#"{"minPauseSec": 5}"#)]);
    assert_eq!(none.proposal, None);
    assert_eq!(
        none.refusals,
        vec![Refusal {
            call_index: 0,
            reason: "There are no pauses longer than 5.0 s to remove.".into(),
            audience: Audience::Person,
            clip: None,
        }]
    );
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
fn additions_and_removals_split_in_one_answer() {
    let (p, mut speech) = talk();
    speech.levels.insert("talk".into(), levels());
    let i = inputs(&p, speech, "make it square and remove pauses over 1 s");
    let calls = [
        call("setFormat", r#"{"format": "square", "keepOthers": true}"#),
        call("removeSilences", r#"{"minPauseSec": 1}"#),
    ];
    let out = compile(&p, &i, &calls);
    assert_eq!(out.refusals, vec![]);
    let additions = out.additions.clone().expect("the format applies at once");
    assert_eq!(
        additions.ops,
        vec![Op::SetSettings {
            settings: Settings {
                fps: 30,
                formats: vec![FormatKind::Landscape, FormatKind::Square],
                primary_format: FormatKind::Square,
            }
        }]
    );
    assert_eq!(additions.origin, Some(Origin::Chat));
    assert_eq!(additions.label, "chat.setFormat");
    assert_eq!(out.proposed_calls, vec![1], "the pause removal is proposed");
    let proposal = out.proposal.clone().expect("the pauses are proposed");
    assert_eq!(
        proposal.tx.ops,
        vec![Op::DeleteRanges {
            ranges: vec![span(15.15, 16.85)]
        }]
    );
    assert_eq!(proposal.tx.origin, Some(Origin::Chat));
    // The proposal is judged on the project as the additions leave it, which is what the app applies it to.
    let with_additions = apply_ops(&p, &additions.ops).unwrap();
    assert_eq!(
        proposal.review.after,
        apply_ops(&with_additions, &proposal.tx.ops).unwrap()
    );
    assert_eq!(
        proposal.review.after.settings.primary_format,
        FormatKind::Square
    );
    // The additions are one undo step and Apply another: one undo right after Apply brings the cut footage back.
    let mut doc = Document::new(p.clone()).unwrap();
    doc.apply(&additions).unwrap();
    doc.apply(&proposal.tx).unwrap();
    assert_eq!(doc.project(), &proposal.review.after);
    doc.undo();
    assert_eq!(doc.project(), &with_additions);
    doc.undo();
    assert_eq!(doc.project(), &p);
    // Asked the other way round, it is the same edit: the additions always come first.
    let reversed = compile(&p, &i, &[calls[1].clone(), calls[0].clone()]);
    assert_eq!(reversed.additions, out.additions);
    assert_eq!(reversed.proposal, out.proposal);
}

#[test]
fn an_addition_that_takes_something_away_is_proposed() {
    let mut p = broll(30.0);
    // The captions were placed by hand in landscape: dropping landscape loses that placement.
    p.captions.place.insert(FormatKind::Landscape, 0.8);
    let i = inputs(
        &p,
        Speech::default(),
        "only vertical, with karaoke captions",
    );
    let only_portrait = call(
        "setFormat",
        r#"{"format": "portrait", "keepOthers": false}"#,
    );
    let karaoke = call("setCaptions", r#"{"on": true, "style": "karaoke"}"#);
    let out = compile(&p, &i, &[only_portrait.clone(), karaoke]);
    assert_eq!(out.refusals, vec![]);
    // The captions apply at once; the format waits for the person, judged by the model's own review.
    let additions = out.additions.expect("the captions apply at once");
    assert!(matches!(additions.ops.as_slice(), [Op::SetCaptions { .. }]));
    let proposal = out.proposal.expect("the format is proposed");
    assert_eq!(
        proposal.tx.ops,
        vec![Op::SetSettings {
            settings: Settings {
                fps: 30,
                formats: vec![FormatKind::Portrait],
                primary_format: FormatKind::Portrait,
            }
        }]
    );
    assert_eq!(proposal.tx.label, "chat.setFormat");
    assert_eq!(
        proposal.review.removals.formats,
        vec![FormatKind::Landscape]
    );
    assert_eq!((proposal.removed_us, proposal.ranges.len()), (0, 0));
    // Keeping the other formats loses nothing: applied at once.
    let kept = compile(
        &p,
        &i,
        &[call(
            "setFormat",
            r#"{"format": "portrait", "keepOthers": true}"#,
        )],
    );
    assert!(kept.additions.is_some());
    assert_eq!(kept.proposal, None);
    // Without anything placed by hand, dropping a format loses nothing either.
    let plain = broll(30.0);
    let bare = compile(
        &plain,
        &inputs(&plain, Speech::default(), "only vertical"),
        &[only_portrait],
    );
    assert!(bare.additions.is_some());
    assert_eq!(bare.proposal, None);
}

#[test]
fn format_changes_stay_in_order_when_one_is_proposed() {
    let mut p = broll(30.0);
    p.captions.place.insert(FormatKind::Landscape, 0.8);
    let i = inputs(&p, Speech::default(), "only vertical, then add square too");
    let out = compile(
        &p,
        &i,
        &[
            call(
                "setFormat",
                r#"{"format": "portrait", "keepOthers": false}"#,
            ),
            call("setFormat", r#"{"format": "square", "keepOthers": true}"#),
        ],
    );
    assert_eq!(out.refusals, vec![]);
    // The first is proposed (it drops the hand-placed landscape captions), so the second waits with it, after it.
    assert_eq!(out.additions, None);
    let proposal = out.proposal.expect("both formats are proposed");
    assert_eq!(
        proposal.tx.ops,
        vec![
            Op::SetSettings {
                settings: Settings {
                    fps: 30,
                    formats: vec![FormatKind::Portrait],
                    primary_format: FormatKind::Portrait,
                }
            },
            Op::SetSettings {
                settings: Settings {
                    fps: 30,
                    formats: vec![FormatKind::Portrait, FormatKind::Square],
                    primary_format: FormatKind::Square,
                }
            },
        ]
    );
    assert_eq!(
        proposal.review.after.settings.formats,
        vec![FormatKind::Portrait, FormatKind::Square]
    );
}

#[test]
fn no_analysis_gives_a_friendly_refusal() {
    assert_eq!(STILL_LISTENING, "Klipolo is still listening to this clip");
    assert_eq!(ANALYZE_FIRST, "Analyze this clip first");
    let (p, speech) = talk();
    let calls = [
        call("removeFillers", "{}"),
        call("removeSilences", r#"{"minPauseSec": null}"#),
        call("findMoments", r#"{"query": "the module", "ranges": []}"#),
    ];
    for (state, said) in [
        (Some(AnalysisState::Transcribing), STILL_LISTENING),
        (Some(AnalysisState::NotStarted), ANALYZE_FIRST),
        (None, ANALYZE_FIRST),
    ] {
        let mut i = inputs(&p, speech.clone(), "remove the ums and the pauses");
        i.analysis.clear();
        if let Some(state) = state {
            i.analysis.insert("talk".into(), state);
        }
        let out = compile(&p, &i, &calls);
        // The sentence as it is, and the clip it is about by its ref, so "this clip" is never in doubt.
        let expected: Vec<Refusal> = (0..3)
            .map(|k| Refusal {
                call_index: k,
                reason: said.into(),
                audience: Audience::Person,
                clip: Some("c1".into()),
            })
            .collect();
        assert_eq!(out.refusals, expected, "{state:?}");
        assert_eq!(out.proposal, None);
        assert!(out.moments.is_empty());
    }
    // Two files, one still being heard and one never analyzed: analyzing is what the person can do, so the refusal
    // names the clip that needs it.
    let two = project(
        vec![file("talk", 30.0, true), file("more", 10.0, true)],
        vec![clip("t", "talk", 0.0, 30.0), clip("m", "more", 0.0, 10.0)],
    );
    let mut i = inputs(&two, speech.clone(), "remove the ums");
    i.analysis = HashMap::from([
        ("talk".into(), AnalysisState::Transcribing),
        ("more".into(), AnalysisState::NotStarted),
    ]);
    let out = compile(&two, &i, &calls[..1]);
    assert_eq!(
        (
            out.refusals[0].reason.as_str(),
            out.refusals[0].clip.as_deref()
        ),
        (ANALYZE_FIRST, Some("c2"))
    );
    // Only the second still being heard: that one is named.
    i.analysis.insert("talk".into(), AnalysisState::Done);
    i.analysis
        .insert("more".into(), AnalysisState::Transcribing);
    let out = compile(&two, &i, &calls[..1]);
    assert_eq!(
        (
            out.refusals[0].reason.as_str(),
            out.refusals[0].clip.as_deref()
        ),
        (STILL_LISTENING, Some("c2"))
    );
    // Both done: the fillers are proposed.
    let done = compile(&two, &inputs(&two, speech, "remove the ums"), &calls[..1]);
    assert_eq!(done.refusals, vec![]);
    assert!(done.proposal.is_some());
}

#[test]
fn fix_caption_words_targets_sound_time() {
    // The interview's sound starts 0.5 s into its file, and its clip plays the file from 2.0 s, after a 4.0 s intro.
    let mut interview = file("interview", 20.0, true);
    interview.audio_lead_us = Some(secs(0.5));
    let p = project(
        vec![file("intro", 4.0, false), interview],
        vec![
            clip("i", "intro", 0.0, 4.0),
            clip("v", "interview", 2.0, 20.0),
        ],
    );
    let word = |text: &str, a: f64, b: f64| Word {
        text: text.into(),
        start_us: secs(a),
        end_us: secs(b),
        confidence: 0.9,
        kind: WordKind::Word,
    };
    // In the file's sound time. The first "Caden" (0.2 s, file 0.7 s) plays before the clip's in point: not shown.
    let words = vec![
        word("Caden", 0.2, 0.6),
        word("Hi", 4.6, 4.9),
        word("Caden,", 5.0, 5.4),
        word("welcome.", 5.5, 6.0),
        word("caden", 9.0, 9.3),
        word("Cadence", 10.0, 10.5),
    ];
    let mut speech = Speech::default();
    speech
        .transcripts
        .insert("interview".into(), transcript(words, 19.5));
    let i = inputs(&p, speech, "it's Kaiden, not Caden");
    let fix = |a: f64, b: f64, text: &str| Op::FixWords {
        asset_id: "interview".into(),
        start_us: secs(a),
        end_us: secs(b),
        text: Some(text.into()),
    };
    let out = compile(
        &p,
        &i,
        &[call(
            "fixCaptionWords",
            r#"{"find": "Caden", "replace": "Kaiden"}"#,
        )],
    );
    assert_eq!(out.refusals, vec![]);
    let additions = out.additions.expect("word fixes apply at once");
    // Sound time as the transcript has it, not file time (0.5 s later) nor the timeline's. The comma stays;
    // "Cadence" is another word.
    assert_eq!(
        additions.ops,
        vec![fix(5.0, 5.4, "Kaiden,"), fix(9.0, 9.3, "Kaiden")]
    );
    assert_eq!(additions.label, "chat.fixCaptionWords");
    // Several words read as one fix.
    let both = compile(
        &p,
        &i,
        &[call(
            "fixCaptionWords",
            r#"{"find": "hi caden", "replace": "Hey Kaiden"}"#,
        )],
    );
    assert_eq!(
        both.additions.expect("a fix").ops,
        vec![fix(4.6, 5.4, "Hey Kaiden,")]
    );
    // No such word: said back to the model.
    let none = compile(
        &p,
        &i,
        &[call(
            "fixCaptionWords",
            r#"{"find": "Noelle", "replace": "Noel"}"#,
        )],
    );
    assert_eq!(none.additions, None);
    assert!(refused_to_model(&none, 0).starts_with("fixCaptionWords: "));
}

#[test]
fn failed_call_is_reported_and_others_still_compile() {
    let (mut p, speech) = talk();
    // The file in two clips, cut at 15.0 s.
    p.timeline.main = vec![
        clip("t", "talk", 0.0, 15.0),
        clip("t-2", "talk", 15.0, 30.0),
    ];
    let i = inputs(
        &p,
        speech,
        "vertical only, split at 15, karaoke captions, cut the boring bit and the ums",
    );
    let calls = [
        call(
            "setFormat",
            r#"{"format": "portrait", "keepOthers": false}"#,
        ),
        // Snaps to the cut between the clips, where there is nothing to split: the model refuses it.
        call("splitAt", r#"{"atSec": 15.1}"#),
        call("setCaptions", r#"{"on": true, "style": "karaoke"}"#),
        // Invented: nothing is said or shown there.
        cut(20.0, 24.0),
        call("removeFillers", "{}"),
    ];
    let out = compile(&p, &i, &calls);
    let refused: Vec<(usize, Audience)> = (out.refusals.iter())
        .map(|r| (r.call_index, r.audience))
        .collect();
    assert_eq!(refused, vec![(1, Audience::Model), (3, Audience::Model)]);
    assert!(
        out.refusals[0].reason.starts_with("splitAt: "),
        "{}",
        out.refusals[0].reason
    );
    assert!(out.refusals[1].reason.starts_with("cutRange: "));
    let additions = out
        .additions
        .expect("the format and the captions still apply");
    assert_eq!(
        additions.ops,
        vec![
            Op::SetSettings {
                settings: Settings {
                    fps: 30,
                    formats: vec![FormatKind::Portrait],
                    primary_format: FormatKind::Portrait,
                }
            },
            Op::SetCaptions {
                captions: CaptionSettings {
                    enabled: true,
                    style: Some(BuiltinCaptionStyle::Karaoke.id()),
                    ..p.captions.clone()
                }
            },
        ]
    );
    assert!(apply_ops(&p, &additions.ops).is_ok());
    // One undo step, named for the first change and how many more.
    assert_eq!(additions.label, "chat.andMore");
    let params = additions.params.expect("the label's values");
    assert_eq!(params["more"], ErrorParam::Number(1.0));
    match &params["first"] {
        ErrorParam::Coded(first) => assert_eq!(first.code, "chat.setFormat"),
        other => panic!("{other:?}"),
    }
    // The fillers are still proposed.
    let proposal = out.proposal.expect("the fillers are proposed");
    assert_eq!(proposal.tx.label, "chat.removeFillers");
}

#[test]
fn a_ref_that_no_longer_resolves_is_refused() {
    let three = three_clips();
    // The context named three clips; the project has lost the middle one since. Its c2 is not the old c3.
    let i = inputs(&three, Speech::default(), "end the second clip at 25");
    let mut now = three.clone();
    now.timeline.main.remove(1);
    let out = compile(
        &now,
        &i,
        &[trim("c2", "end", 25.0), trim("c9", "end", 25.0)],
    );
    let reasons: Vec<(&str, Audience)> = (out.refusals.iter())
        .map(|r| (r.reason.as_str(), r.audience))
        .collect();
    assert_eq!(
        reasons,
        vec![
            ("trimClip: that clip is gone.", Audience::Model),
            ("trimClip: there is no clip c9.", Audience::Model),
        ]
    );
    assert_eq!((out.additions, out.proposal), (None, None));
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
    // Between words that join times, or at the end of a phrase.
    for (message, times) in [
        ("split at 12", vec![12.0]),
        ("cut from 12 to 15", vec![12.0, 15.0]),
        ("cut 12 to 15", vec![12.0, 15.0]),
        ("cut 12-15", vec![12.0, 15.0]),
        ("cut between 1:05 and 1:20", vec![65.0, 80.0]),
        ("split at 12, then add captions", vec![12.0]),
        // After a time word, or right after another time, whatever word follows.
        ("cut 12 to 15 please", vec![12.0, 15.0]),
        ("cut at 12 please", vec![12.0]),
        ("cut after 45 because it's boring", vec![45.0]),
        ("split at 30 where she laughs", vec![30.0]),
        ("cut 12-15 please", vec![12.0, 15.0]),
        ("cut between 12 and 15 please", vec![12.0, 15.0]),
        // Two-digit seconds read as a clock even where they could be a ratio.
        ("cut at 16:10", vec![970.0]),
    ] {
        let want: Vec<Us> = times.into_iter().map(secs).collect();
        assert_eq!(typed_times(message, d), want, "{message}");
    }
    // Counts, refs, speeds, ordinals and aspect ratios are no time.
    for message in [
        "remove 3 pauses",
        "remove 3 pauses and 2 ums",
        "cut 2 clips",
        "cut the last two sentences",
        "keep the first 3",
        "make it 9:16",
        "make it 16:9 again",
        "trim c3",
        "play at 2x",
        "slow it to 0.5x",
        "the 2nd clip",
        "at 1:5",
        "cut 5m",
    ] {
        assert_eq!(typed_times(message, d), Vec::<Us>::new(), "{message}");
    }

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
fn trims_stay_within_the_clip_or_its_neighbour_and_the_file() {
    let p = three_clips();
    let run =
        |message: &str, c: ToolCall| compile(&p, &inputs(&p, Speech::default(), message), &[c]);
    // Shorter, to a time the person said: proposed.
    let inward = run("end c2 at 15", trim("c2", "end", 15.0));
    assert_eq!(inward.additions, None);
    let proposal = inward.proposal.expect("a shorter clip is proposed");
    assert_eq!(
        proposal.tx.ops,
        vec![Op::TrimClip {
            id: "b".into(),
            edge: Edge::End,
            source_us: secs(25.0)
        }]
    );
    assert_eq!(proposal.ranges, vec![span(15.0, 20.0)]);
    assert_eq!(proposal.tx.label, "chat.trimClip");
    // Shorter, to an invented time: refused like a cut.
    let invented = run("shorten c2", trim("c2", "end", 17.0));
    assert!(
        refused_to_model(&invented, 0).starts_with(
            "trimClip: c2 plays from 10.0 to 20.0 s; toSec 17.0 is not within 0.3 s of"
        ),
        "{:?}",
        invented.refusals
    );
    // Longer, into the clip next to it, with footage the file has: applied at once.
    let later = run("make c2 longer", trim("c2", "end", 22.0));
    assert_eq!(later.refusals, vec![]);
    assert_eq!(
        later.additions.expect("a longer clip applies at once").ops,
        vec![Op::TrimClip {
            id: "b".into(),
            edge: Edge::End,
            source_us: secs(32.0)
        }]
    );
    let earlier = run("start c2 earlier", trim("c2", "start", 8.0));
    assert_eq!(
        earlier.additions.expect("a longer clip").ops,
        vec![Op::TrimClip {
            id: "b".into(),
            edge: Edge::Start,
            source_us: secs(18.0)
        }]
    );
    // The last clip may grow past the main track's end, as far as its file goes.
    let last = run("longer", trim("c3", "end", 35.0));
    assert_eq!(
        last.additions.expect("a longer clip").ops,
        vec![Op::TrimClip {
            id: "c".into(),
            edge: Edge::End,
            source_us: secs(55.0)
        }]
    );
    // Refused with the clip's range and how far that end may move: a time on the wrong side of it, past the clip
    // next to it, past its file, or where the end already is. The first clip's start can't move earlier.
    for (c, why) in [
        (
            trim("c2", "end", 5.0),
            "trimClip: c2 plays from 10.0 to 20.0 s and can grow to 30.0; 5.0 is outside it.",
        ),
        (
            trim("c2", "start", 25.0),
            "trimClip: c2 plays from 10.0 to 20.0 s and can start as early as 0.0; 25.0 is outside it.",
        ),
        (
            trim("c2", "end", 45.0),
            "trimClip: c2 plays from 10.0 to 20.0 s and can grow to 30.0; 45.0 is outside it.",
        ),
        (
            trim("c3", "end", 45.0),
            "trimClip: c3 plays from 20.0 to 30.0 s and can grow to 40.0; 45.0 is outside it.",
        ),
        (
            trim("c2", "end", 20.0),
            "trimClip: c2 plays from 10.0 to 20.0 s and can grow to 30.0; it already ends at 20.0.",
        ),
        (
            trim("c1", "start", 0.0),
            "trimClip: c1 plays from 0.0 to 10.0 s and can't start earlier; it already starts at 0.0.",
        ),
    ] {
        let out = run("trim 5 15 25 35 45", c);
        assert_eq!(refused_to_model(&out, 0), why);
        assert_eq!((&out.additions, &out.proposal), (&None, &None));
    }
    // Clip edges count as evidence: a cut of the middle clip, a little off.
    let middle = run("remove the second clip", cut(10.1, 19.9));
    assert_eq!(
        middle.proposal.expect("a cut").ranges,
        vec![span(10.0, 20.0)]
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

#[test]
fn an_outward_trim_before_a_cut_shifts_the_cut_by_content() {
    // One 40 s file in two clips: c1 plays its 0 to 10 s, c2 its 20 to 30 s.
    let p = project(
        vec![file("long", 40.0, false)],
        vec![clip("a", "long", 0.0, 10.0), clip("b", "long", 20.0, 30.0)],
    );
    let i = inputs(
        &p,
        Speech::default(),
        "end the first clip at 12 and cut 15 to 17",
    );
    let out = compile(&p, &i, &[trim("c1", "end", 12.0), cut(15.0, 17.0)]);
    assert_eq!(out.refusals, vec![]);
    assert_eq!(
        out.additions.expect("c1 grows at once").ops,
        vec![Op::TrimClip {
            id: "a".into(),
            edge: Edge::End,
            source_us: secs(12.0)
        }]
    );
    // c1 is 2 s longer, so the footage the model named (25 to 27 s of the file, in c2) plays 2 s later.
    let proposal = out.proposal.expect("the cut is proposed");
    assert_eq!(
        proposal.tx.ops,
        vec![Op::DeleteRanges {
            ranges: vec![span(17.0, 19.0)]
        }]
    );
    assert_eq!(proposal.ranges, vec![span(17.0, 19.0)]);
    let pieces: Vec<(Us, Us)> = (proposal.review.after.timeline.main[1..].iter())
        .map(|c| (c.source_in_us, c.source_out_us))
        .collect();
    assert_eq!(
        pieces,
        vec![(secs(20.0), secs(25.0)), (secs(27.0), secs(30.0))]
    );
}

/// Each main clip's part of its file, as (source in, source out).
fn sources(p: &Project) -> Vec<(Us, Us)> {
    (p.timeline.main.iter())
        .map(|c| (c.source_in_us, c.source_out_us))
        .collect()
}

#[test]
fn cuts_follow_the_footage_through_a_sped_up_clip() {
    // long.mov (60 s): c1 plays its 0 to 10 s at twice the speed (0 to 5 s on the timeline), c2 its 20 to 40 s.
    let mut fast = clip("a", "long", 0.0, 10.0);
    fast.speed = 2.0;
    let p = project(
        vec![file("long", 60.0, false)],
        vec![fast, clip("b", "long", 20.0, 40.0)],
    );
    let i = inputs(&p, Speech::default(), "cut 1 to 2 and 3 to 4");
    let out = compile(&p, &i, &[cut(1.0, 2.0), cut(3.0, 4.0)]);
    assert_eq!(out.refusals, vec![]);
    let proposal = out.proposal.expect("a proposal");
    // The first takes the file's 2 to 4 s; the second names its 6 to 8 s, at 2 to 3 s once the first is gone.
    assert_eq!(
        proposal.tx.ops,
        vec![
            Op::DeleteRanges {
                ranges: vec![span(1.0, 2.0)]
            },
            Op::DeleteRanges {
                ranges: vec![span(2.0, 3.0)]
            },
        ]
    );
    assert_eq!(proposal.ranges, vec![span(1.0, 2.0), span(3.0, 4.0)]);
    assert_eq!(
        sources(&proposal.review.after),
        vec![
            (0, secs(2.0)),
            (secs(4.0), secs(6.0)),
            (secs(8.0), secs(10.0)),
            (secs(20.0), secs(40.0)),
        ]
    );
}

#[test]
fn cuts_follow_the_footage_of_a_copied_clip() {
    // broll.mov's first 10 s twice: c1 at 0 to 10 s, its copy c2 at 10 to 20 s.
    let p = project(
        vec![file("broll", 30.0, false)],
        vec![clip("a", "broll", 0.0, 10.0), clip("b", "broll", 0.0, 10.0)],
    );
    let i = inputs(&p, Speech::default(), "cut 5 to 6 and 12 to 14");
    let out = compile(&p, &i, &[cut(5.0, 6.0), cut(12.0, 14.0)]);
    assert_eq!(out.refusals, vec![]);
    let proposal = out.proposal.expect("a proposal");
    // The second names the copy's 2 to 4 s of the file: only the copy loses it, though c1 still plays it too.
    assert_eq!(
        proposal.tx.ops,
        vec![
            Op::DeleteRanges {
                ranges: vec![span(5.0, 6.0)]
            },
            Op::DeleteRanges {
                ranges: vec![span(11.0, 13.0)]
            },
        ]
    );
    assert_eq!(proposal.ranges, vec![span(5.0, 6.0), span(12.0, 14.0)]);
    assert_eq!(
        sources(&proposal.review.after),
        vec![
            (0, secs(5.0)),
            (secs(6.0), secs(10.0)),
            (0, secs(2.0)),
            (secs(4.0), secs(10.0)),
        ]
    );
}

#[test]
fn a_shorter_clip_after_a_cut_in_it_removes_the_same_footage() {
    let p = three_clips();
    let i = inputs(&p, Speech::default(), "cut 12 to 14 and end c2 at 15");
    let out = compile(&p, &i, &[cut(12.0, 14.0), trim("c2", "end", 15.0)]);
    assert_eq!(out.refusals, vec![]);
    let proposal = out.proposal.expect("a proposal");
    // The cut leaves c2 in two pieces (the file's 20 to 22 s and 24 to 30 s), so the trim removes the file's 25 to
    // 30 s from where the second piece plays it, 13 to 18 s, rather than trimming the first piece.
    assert_eq!(
        proposal.tx.ops,
        vec![
            Op::DeleteRanges {
                ranges: vec![span(12.0, 14.0)]
            },
            Op::DeleteRanges {
                ranges: vec![span(13.0, 18.0)]
            },
        ]
    );
    assert_eq!(proposal.ranges, vec![span(12.0, 14.0), span(15.0, 20.0)]);
    assert_eq!(
        sources(&proposal.review.after),
        vec![
            (0, secs(10.0)),
            (secs(20.0), secs(22.0)),
            (secs(24.0), secs(25.0)),
            (secs(40.0), secs(50.0)),
        ]
    );
}

#[test]
fn a_call_failing_part_way_leaves_the_working_copy_as_it_was() {
    let p = project(
        vec![file("talk", 30.0, true)],
        vec![clip("t", "talk", 0.0, 30.0)],
    );
    let word = |text: &str, a: f64, b: f64| Word {
        text: text.into(),
        start_us: secs(a),
        end_us: secs(b),
        confidence: 0.9,
        kind: WordKind::Word,
    };
    // "Caden" twice; the second was heard as taking no time, so it can't hold a fix: the call's second op fails.
    let mut speech = Speech::default();
    speech.transcripts.insert(
        "talk".into(),
        transcript(
            vec![
                word("Caden", 1.0, 1.4),
                word("welcome", 1.5, 2.0),
                word("Caden", 3.0, 3.0),
            ],
            30.0,
        ),
    );
    let i = inputs(&p, speech, "captions on, Kaiden not Caden, and Welcome");
    let out = compile(
        &p,
        &i,
        &[
            call("setCaptions", r#"{"on": true, "style": null}"#),
            call(
                "fixCaptionWords",
                r#"{"find": "Caden", "replace": "Kaiden"}"#,
            ),
            call(
                "fixCaptionWords",
                r#"{"find": "welcome", "replace": "Welcome"}"#,
            ),
        ],
    );
    // The model's own refusal of the second op (`model.fix.range`), so the call did fail part-way.
    assert_eq!(
        refused_to_model(&out, 1),
        "fixCaptionWords: A word fix needs a start before its end."
    );
    // The first "Caden" fix, applied before the second failed, is gone with the call; the next call met the copy as
    // it was.
    let additions = out.additions.expect("the others apply");
    let welcome = Op::FixWords {
        asset_id: "talk".into(),
        start_us: secs(1.5),
        end_us: secs(2.0),
        text: Some("Welcome".into()),
    };
    assert!(matches!(
        additions.ops.as_slice(),
        [Op::SetCaptions { .. }, w] if *w == welcome
    ));
    let after = apply_ops(&p, &additions.ops).unwrap();
    assert_eq!(after.word_fixes.len(), 1);
    assert_eq!(after.word_fixes[0].text, "Welcome");
}

#[test]
fn split_snaps_to_evidence_but_is_never_refused() {
    let p = broll(30.0);
    let i = inputs(&p, Speech::default(), "split at 12");
    let near = compile(&p, &i, &[call("splitAt", r#"{"atSec": 12.1}"#)]);
    let additions = near.additions.expect("a split applies at once");
    assert_eq!(additions.ops, vec![Op::Split { at_us: secs(12.0) }]);
    assert_eq!(additions.label, "chat.splitAt");
    let anywhere = compile(&p, &i, &[call("splitAt", r#"{"atSec": 17.3}"#)]);
    assert_eq!(anywhere.refusals, vec![]);
    assert_eq!(
        anywhere.additions.expect("a split").ops,
        vec![Op::Split { at_us: secs(17.3) }]
    );
}

#[test]
fn moments_snap_to_the_evidence_and_invented_ones_are_refused() {
    let p = broll(60.0);
    let mut i = inputs(&p, Speech::default(), "where does she laugh");
    i.scenes.insert(
        "broll".into(),
        SceneLog {
            items: vec![SceneEntry {
                start_us: secs(3.0),
                end_us: secs(8.0),
                kind: "reaction".into(),
                who: "woman".into(),
                text: "laughs".into(),
                on_screen_text: None,
            }],
        },
    );
    let find = |ranges: &str| {
        compile(
            &p,
            &i,
            &[call(
                "findMoments",
                &format!(r#"{{"query": "she laughs", "ranges": {ranges}}}"#),
            )],
        )
    };
    let found = find(r#"[{"startSec": 3.1, "endSec": 7.9, "label": "she laughs"}]"#);
    assert_eq!(found.refusals, vec![]);
    assert_eq!(
        found.moments,
        vec![Moment {
            range: span(3.0, 8.0),
            label: "she laughs".into()
        }]
    );
    assert_eq!((found.additions, found.proposal), (None, None));
    let invented = find(r#"[{"startSec": 40, "endSec": 45, "label": "she laughs"}]"#);
    assert!(invented.moments.is_empty());
    assert!(
        refused_to_model(&invented, 0)
            .starts_with("findMoments: ranges[0].startSec 40.0 is not within 0.3 s of"),
        "{:?}",
        invented.refusals
    );
    // None found: nothing to show, nothing refused.
    let none = find("[]");
    assert_eq!((none.moments.len(), none.refusals.len()), (0, 0));
}

#[test]
fn undo_with_edits_refuses_the_edits() {
    let p = broll(30.0);
    let i = inputs(&p, Speech::default(), "undo that and make it square");
    let out = compile(
        &p,
        &i,
        &[
            call("undo", "{}"),
            call("setFormat", r#"{"format": "square", "keepOthers": true}"#),
        ],
    );
    assert!(out.undo);
    assert_eq!(out.additions, None);
    assert!(refused_to_model(&out, 1).starts_with("setFormat: "));
    let alone = compile(&p, &i, &[call("undo", "{}")]);
    assert!(alone.undo);
    assert_eq!(alone.refusals, vec![]);
}

#[test]
fn questions_jobs_and_cannot_do_pass_through() {
    let p = broll(30.0);
    let out = compile(
        &p,
        &inputs(
            &p,
            Speech::default(),
            "follow her, blur faces, add music, cut the bad part",
        ),
        &[
            // Dashes the model wrote never reach the person.
            call(
                "askUser",
                r#"{"question": "Which part should go \u2014 the intro?"}"#,
            ),
            call(
                "cannotDo",
                r#"{"reason": "Klipolo can't add music yet \u2014 sorry."}"#,
            ),
            call("followSubject", r#"{"description": "woman astronaut"}"#),
            call("blurObjects", r#"{"description": "face"}"#),
        ],
    );
    assert_eq!(
        out.questions,
        vec!["Which part should go, the intro?".to_owned()]
    );
    assert_eq!(
        out.refusals,
        vec![Refusal {
            call_index: 1,
            reason: "Klipolo can't add music yet, sorry.".into(),
            audience: Audience::Person,
            clip: None,
        }]
    );
    assert_eq!(
        out.jobs,
        vec![
            Job::Follow {
                description: "woman astronaut".into()
            },
            Job::Blur {
                description: "face".into()
            },
        ]
    );
    assert_eq!((out.additions, out.proposal, out.undo), (None, None, false));
}

#[test]
fn proposed_calls_name_the_calls_in_the_proposal() {
    let (p, speech) = talk();
    let i = inputs(&p, speech, "square, no ums, and cut the rest");
    let calls = [
        call("setFormat", r#"{"format": "square", "keepOthers": true}"#),
        call("removeFillers", "{}"),
        cut(17.0, 21.0),
        cut(7.9, 29.8),
        call("askUser", r#"{"question": "Anything else?"}"#),
    ];
    let out = compile(&p, &i, &calls);
    assert_eq!(out.proposed_calls, vec![1, 3]);
    assert_eq!(
        out.refusals
            .iter()
            .map(|r| r.call_index)
            .collect::<Vec<_>>(),
        vec![2]
    );
    // With nothing proposed, no call is.
    let out = compile(&p, &i, &calls[..1]);
    assert_eq!((out.proposal, out.proposed_calls), (None, vec![]));
}

#[test]
fn nothing_to_remove_is_told_to_the_person() {
    // No loudness for talk.mov: no pause to find. Shown to the person, never sent back for a second model call.
    let (p, speech) = talk();
    let i = inputs(&p, speech, "remove pauses over a second");
    let out = compile(&p, &i, &[call("removeSilences", r#"{"minPauseSec": 1}"#)]);
    assert_eq!(out.proposal, None);
    assert_eq!(
        out.refusals,
        vec![Refusal {
            call_index: 0,
            reason: "There are no pauses longer than 1.0 s to remove.".into(),
            audience: Audience::Person,
            clip: None,
        }]
    );
    // No speech in the clips: no filler word.
    let p = three_clips();
    let i = inputs(&p, Speech::default(), "remove the ums");
    let out = compile(&p, &i, &[call("removeFillers", "{}")]);
    assert_eq!(
        out.refusals,
        vec![Refusal {
            call_index: 0,
            reason: "There are no filler words to remove.".into(),
            audience: Audience::Person,
            clip: None,
        }]
    );
}
