//! A lesson recorded as a screen take with a microphone, and "remove the pauses longer than 1.5 seconds and add
//! captions": `removeSilences` cuts the quiet stretches, but not one where the person clicks or types (a step shown
//! in silence), and the message changes nothing else: the format stays, captions come on. The project is made here:
//! - c1: Recording 1, 0 to 30 s of its take, landscape only;
//! - its microphone is quiet from 5 to 8 s and from 15 to 18 s, loud elsewhere;
//! - the person clicks at 6.0 s of the take, in the first quiet stretch.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_chat::{
    AnalysisState, Compiled, CursorTakes, FunctionCall, Inputs, Refs, ToolCall, compile,
};
use klipolo_cursor::press::Press;
use klipolo_cursor::timeline::TakeFacts;
use klipolo_model::cursor_style::RecordingCursor;
use klipolo_model::project::{
    Asset, AssetKind, FormatKind, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole,
    SourceRun,
};
use klipolo_model::speech::{Levels, Speech};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_model::{Project, apply_ops};
use serde_json::{Value, json};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall::parse(&FunctionCall {
        name: name.into(),
        call_id: "call_1".into(),
        arguments: arguments.to_string(),
    })
    .unwrap()
}

fn asset(id: &str, kind: AssetKind) -> Asset {
    let picture = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: format!("{id}.mov"),
        duration_us: secs(60.0),
        width: if picture { 1920 } else { 0 },
        height: if picture { 1080 } else { 0 },
        fps: if picture { 30.0 } else { 0.0 },
        rotation: None,
        has_audio: !picture,
        audio_lead_us: None,
        made_with: None,
    }
}

fn source(id: &str, role: SourceRole, file: &str) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: "capture.source.screen".into(),
        runs: vec![SourceRun {
            asset_id: file.into(),
            offset_us: 0,
            duration_us: secs(60.0),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

fn lesson() -> Project {
    let mut p = Project::new("Lesson");
    p.assets = vec![
        asset("s1", AssetKind::Video),
        asset("mic1", AssetKind::Audio),
    ];
    p.recordings = vec![Recording {
        id: "rec1".into(),
        kind: RecordingKind::Screen,
        sources: vec![
            source("screen", SourceRole::Screen, "s1"),
            source("mic", SourceRole::Mic, "mic1"),
        ],
        primary: "screen".into(),
        cursor: Some(RecordingCursor::burned_in(
            "/takes/rec1/cursor.kcur".to_owned(),
        )),
        markers: Vec::new(),
        game: None,
    }];
    p.timeline.main = vec![Clip::new(
        ClipSource::Recording { id: "rec1".into() },
        0,
        secs(30.0),
    )];
    p
}

/// The microphone's loudness, 100 ms windows: quiet from 5 to 8 s and from 15 to 18 s.
fn speech() -> Speech {
    let db: Vec<f32> = (0..600)
        .map(|w| {
            let t = w as f64 / 10.0;
            if (5.0..8.0).contains(&t) || (15.0..18.0).contains(&t) {
                -70.0
            } else {
                -20.0
            }
        })
        .collect();
    let mut speech = Speech::default();
    speech
        .levels
        .insert("mic1".into(), Arc::new(Levels { db: db.into() }));
    speech
}

#[derive(Debug)]
struct Takes(Arc<TakeFacts>);

impl CursorTakes for Takes {
    fn facts(&self, _recording: &Recording) -> Option<Arc<TakeFacts>> {
        Some(Arc::clone(&self.0))
    }
}

fn inputs(p: &Project, clicks: &[f64]) -> Inputs {
    let facts = TakeFacts {
        presses: (clicks.iter())
            .map(|&at| Press {
                down: secs(at),
                up: secs(at) + 80_000,
                btn: 0,
                x: 0.5,
                y: 0.5,
            })
            .collect(),
        ..TakeFacts::default()
    };
    Inputs {
        message: "remove the pauses longer than 1.5 seconds and add captions".into(),
        refs: Refs::of(p),
        speech: speech(),
        analysis: HashMap::from([
            ("s1".to_owned(), AnalysisState::Done),
            ("mic1".to_owned(), AnalysisState::Done),
        ]),
        cursor: Some(Arc::new(Takes(Arc::new(facts))) as Arc<dyn CursorTakes>),
        ..Inputs::default()
    }
}

/// What `calls` leave of the lesson's take, in seconds of it, once the cut is applied, and the project.
fn kept(clicks: &[f64], calls: &[ToolCall]) -> (Vec<(f64, f64)>, Project) {
    let p = lesson();
    let out: Compiled = compile(&p, &inputs(&p, clicks), calls);
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let after = out.proposal.as_ref().expect("the cut").review.after.clone();
    let parts = (after.timeline.main.iter())
        .map(|c| (c.source_in_us as f64 / 1e6, c.source_out_us as f64 / 1e6))
        .collect();
    (parts, after)
}

fn cuts(parts: &[(f64, f64)]) -> Vec<RangeUs> {
    parts
        .windows(2)
        .map(|w| RangeUs {
            start: secs(w[0].1),
            end: secs(w[1].0),
        })
        .collect()
}

#[test]
fn a_quiet_stretch_with_a_click_in_it_is_a_step_not_a_pause() {
    let remove = call("removeSilences", json!({"minPauseSec": 1.5}));
    // With no click, both quiet stretches go.
    let (parts, _) = kept(&[], std::slice::from_ref(&remove));
    assert_eq!(cuts(&parts).len(), 2, "{parts:?}");
    // With a click at 6.0 s, the first stays: only the second goes.
    let (parts, _) = kept(&[6.0], std::slice::from_ref(&remove));
    let gone = cuts(&parts);
    assert_eq!(gone.len(), 1, "{parts:?}");
    assert!(
        gone[0].start >= secs(15.0) && gone[0].end <= secs(18.0),
        "{gone:?}"
    );
    // Clicks in every quiet stretch: nothing to cut, and the person is told why.
    let p = lesson();
    let out = compile(&p, &inputs(&p, &[6.0, 16.0]), &[remove]);
    assert!(out.proposal.is_none());
    assert_eq!(out.refusals.len(), 1);
    assert!(
        out.refusals[0]
            .reason
            .contains("have clicks or typing in them"),
        "{}",
        out.refusals[0].reason
    );
}

#[test]
fn the_lesson_message_keeps_its_format_and_turns_captions_on() {
    // As chat answers the message: the pauses, then captions, in the style the format has by default.
    let calls = [
        call("removeSilences", json!({"minPauseSec": 1.5})),
        call(
            "setCaptions",
            json!({"on": true, "style": null, "set": null}),
        ),
    ];
    let (parts, after) = kept(&[6.0], &calls);
    assert_eq!(cuts(&parts).len(), 1, "{parts:?}");
    assert!(after.captions.enabled);
    assert_eq!(after.settings.formats, vec![FormatKind::Landscape]);
    assert_eq!(after.settings.primary_format, FormatKind::Landscape);
    let p = lesson();
    let out = compile(&p, &inputs(&p, &[6.0]), &calls);
    let tx = out.additions.as_ref().expect("captions are an addition");
    assert!(apply_ops(&p, &tx.ops).unwrap().captions.enabled);
}
