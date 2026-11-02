//! `autoZoom`: zoom in on the clicks of the screen recordings on the main track, as a new take's auto zoom does, or
//! take those zooms away. The project is made here:
//! - c1 (clip "a"): Recording 1, 0 to 30 s, clicked at 2.0, 2.8, 10.0 and 10.5 s of its take;
//! - c2 (clip "b"): "broll.mov", 30 to 40 s, an imported file;
//! - c3 (clip "c"): Recording 2, 40 to 60 s, never clicked.
//!
//! Every change is an addition, applied at once as one undo step: never a proposal.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_chat::{
    Audience, Compiled, CursorTakes, FunctionCall, Inputs, Problem, Refs, ToolCall, compile,
};
use klipolo_cursor::press::Press;
use klipolo_cursor::timeline::TakeFacts;
use klipolo_model::cursor_style::RecordingCursor;
use klipolo_model::ops::{Op, Origin};
use klipolo_model::project::{
    Asset, AssetKind, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole, SourceRun,
};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, TrackKind, ZoomTarget};
use klipolo_model::{Project, apply_ops};
use serde_json::{Value, json};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn parse(arguments: Value) -> Result<ToolCall, klipolo_chat::ToolError> {
    ToolCall::parse(&FunctionCall {
        name: "autoZoom".into(),
        call_id: "call_1".into(),
        arguments: arguments.to_string(),
    })
}

fn zoom(on: bool, scale: Option<f64>) -> ToolCall {
    parse(json!({"on": on, "scale": scale})).unwrap()
}

fn asset(id: &str, kind: AssetKind, length: f64, has_audio: bool) -> Asset {
    let picture = kind == AssetKind::Video;
    Asset {
        id: id.into(),
        kind,
        path: format!("/media/{id}"),
        name: format!("{id}.mov"),
        duration_us: secs(length),
        width: if picture { 1920 } else { 0 },
        height: if picture { 1080 } else { 0 },
        fps: if picture { 30.0 } else { 0.0 },
        rotation: None,
        has_audio,
        audio_lead_us: None,
        made_with: None,
    }
}

fn source(id: &str, role: SourceRole, file: &str, length: f64) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: "capture.source.screen".into(),
        runs: vec![SourceRun {
            asset_id: file.into(),
            offset_us: 0,
            duration_us: secs(length),
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

fn recording(id: &str, screen: &str, mic: &str) -> Recording {
    Recording {
        id: id.into(),
        kind: RecordingKind::Screen,
        sources: vec![
            source("screen", SourceRole::Screen, screen, 60.0),
            source("mic", SourceRole::Mic, mic, 60.0),
        ],
        primary: "screen".into(),
        cursor: Some(RecordingCursor::burned_in(format!(
            "/takes/{id}/cursor.kcur"
        ))),
        markers: Vec::new(),
        game: None,
    }
}

/// The project of the module's comment.
fn takes() -> Project {
    let mut p = Project::new("Takes");
    p.assets = vec![
        asset("s1", AssetKind::Video, 60.0, false),
        asset("mic1", AssetKind::Audio, 60.0, true),
        asset("broll", AssetKind::Video, 10.0, true),
        asset("s2", AssetKind::Video, 60.0, false),
        asset("mic2", AssetKind::Audio, 60.0, true),
    ];
    p.recordings = vec![
        recording("rec1", "s1", "mic1"),
        recording("rec2", "s2", "mic2"),
    ];
    let mut a = Clip::new(ClipSource::Recording { id: "rec1".into() }, 0, secs(30.0));
    a.id = "a".into();
    let mut b = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(10.0));
    b.id = "b".into();
    let mut c = Clip::new(ClipSource::Recording { id: "rec2".into() }, 0, secs(20.0));
    c.id = "c".into();
    p.timeline.main = vec![a, b, c];
    p
}

/// Each take's facts, by recording id, as the app reads them from its cursor file.
#[derive(Debug, Default)]
struct Takes(HashMap<String, Arc<TakeFacts>>);

impl CursorTakes for Takes {
    fn facts(&self, recording: &Recording) -> Option<Arc<TakeFacts>> {
        self.0.get(&recording.id).cloned()
    }
}

fn press(at: f64, x: f32, y: f32) -> Press {
    Press {
        down: secs(at),
        up: secs(at) + 80_000,
        btn: 0,
        x,
        y,
    }
}

/// Recording 1 clicked four times, Recording 2 never.
fn clicked() -> Takes {
    let first = TakeFacts {
        presses: vec![
            press(2.0, 0.30, 0.40),
            press(2.8, 0.32, 0.42),
            press(10.0, 0.80, 0.70),
            press(10.5, 0.81, 0.71),
        ],
        ..TakeFacts::default()
    };
    Takes(HashMap::from([
        ("rec1".to_owned(), Arc::new(first)),
        ("rec2".to_owned(), Arc::new(TakeFacts::default())),
    ]))
}

fn inputs(p: &Project, takes: Option<Takes>) -> Inputs {
    Inputs {
        refs: Refs::of(p),
        cursor: takes.map(|t| Arc::new(t) as Arc<dyn CursorTakes>),
        ..Inputs::default()
    }
}

fn run(p: &Project, takes: Option<Takes>, calls: &[ToolCall]) -> Compiled {
    compile(p, &inputs(p, takes), calls)
}

/// The project once the additions are applied: one chat step, never a proposal.
fn applied(p: &Project, out: &Compiled) -> Project {
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    assert!(out.proposal.is_none(), "never a proposal");
    let tx = out.additions.as_ref().expect("additions");
    assert_eq!(tx.origin, Some(Origin::Chat));
    apply_ops(p, &tx.ops).unwrap()
}

fn zooms(p: &Project) -> Vec<Item> {
    let mut out: Vec<Item> = (p.timeline.tracks.iter())
        .filter(|t| t.kind == TrackKind::Zoom)
        .flat_map(|t| t.items.iter().cloned())
        .collect();
    out.sort_by_key(|i| i.start_us);
    out
}

#[test]
fn auto_zoom_reads_on_and_a_scale() {
    assert_eq!(
        zoom(true, None),
        ToolCall::AutoZoom {
            on: true,
            scale: None
        }
    );
    assert_eq!(
        zoom(true, Some(1.5)),
        ToolCall::AutoZoom {
            on: true,
            scale: Some(1.5)
        }
    );
    let err = parse(json!({"on": true, "scale": 4})).unwrap_err();
    assert_eq!(err.field.as_deref(), Some("scale"));
    assert!(matches!(err.problem, Problem::NotWithin { .. }), "{err:?}");
    assert!(!zoom(true, None).names_timeline() && !zoom(false, None).names_layer());
}

#[test]
fn auto_zoom_on_adds_zooms_over_recordings_only() {
    let p = takes();
    let out = run(&p, Some(clicked()), &[zoom(true, None)]);
    let after = applied(&p, &out);
    let added = zooms(&after);
    // Clicks 8 s apart: two zooms, both over Recording 1 (0 to 30 s), none over the b-roll or Recording 2.
    assert_eq!(added.len(), 2, "{added:#?}");
    for z in &added {
        assert!(z.end_us() <= secs(30.0), "{z:?}");
        assert!(z.attach.is_some(), "attached to the clip under it");
        let ItemContent::Zoom { target, scale } = &z.content else {
            panic!("a zoom");
        };
        assert_eq!(*scale, 2.0);
        assert!(matches!(target, ZoomTarget::Auto { .. }));
    }
    assert!(added[0].start_us <= secs(1.7) && added[1].start_us > added[0].end_us());
    assert_eq!(out.additions.as_ref().unwrap().label, "chat.autoZoom");
    // A scale asked for is each zoom's.
    let out = run(&p, Some(clicked()), &[zoom(true, Some(1.5))]);
    let after = applied(&p, &out);
    assert!(
        zooms(&after)
            .iter()
            .all(|z| matches!(z.content, ItemContent::Zoom { scale, .. } if scale == 1.5))
    );
    // Asked again, the zooms already there are left alone: nothing new.
    let out = run(&after, Some(clicked()), &[zoom(true, None)]);
    assert_eq!(out.additions, None);
    assert_eq!(out.refusals.len(), 1);
    assert_eq!(out.refusals[0].audience, Audience::Person);
}

#[test]
fn auto_zoom_off_removes_only_auto_zooms_and_is_never_a_proposal() {
    let p = takes();
    let with_zooms = applied(&p, &run(&p, Some(clicked()), &[zoom(true, None)]));
    // A zoom the person drew by hand, over the b-roll: kept.
    let mut by_hand = zooms(&with_zooms)[0].clone();
    by_hand.id = "mine".into();
    by_hand.start_us = secs(32.0);
    by_hand.attach = None;
    by_hand.content = ItemContent::Zoom {
        target: ZoomTarget::Rect {
            rect: klipolo_model::layout::BoxN {
                x: 0.1,
                y: 0.1,
                w: 0.5,
                h: 0.5,
            },
        },
        scale: 2.0,
    };
    let with_mine = apply_ops(
        &with_zooms,
        &[Op::AddItem {
            track: klipolo_model::ops::TrackTarget::Auto,
            item: by_hand,
            attach: false,
        }],
    )
    .unwrap();
    let out = run(&with_mine, Some(clicked()), &[zoom(false, None)]);
    let after = applied(&with_mine, &out);
    let left = zooms(&after);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, "mine");
}

#[test]
fn auto_zoom_without_recordings_or_clicks_says_why() {
    let mut p = takes();
    p.timeline.main.retain(|c| c.id == "b");
    let out = run(&p, Some(clicked()), &[zoom(true, None)]);
    assert_eq!(out.additions, None);
    assert_eq!(
        (out.refusals[0].reason.as_str(), out.refusals[0].audience),
        (
            "There is no screen recording to zoom in on.",
            Audience::Person
        )
    );
    // Recordings, but no cursor known for them.
    let p = takes();
    let out = run(&p, None, &[zoom(true, None)]);
    assert_eq!(
        out.refusals[0].reason,
        "These screen recordings have no clicks to zoom in on."
    );
    // Nothing to take away.
    let out = run(&p, Some(clicked()), &[zoom(false, None)]);
    assert_eq!(
        out.refusals[0].reason,
        "There are no auto zooms to take away."
    );
}
