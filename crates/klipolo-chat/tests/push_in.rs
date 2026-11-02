//! A slow push-in on the main footage from chat: `setMotion` on a clip with no camera bubble takes scale keys (and an
//! x and a y for the point), and makes one push zoom (`ZoomTarget::Push`) on a zoom track: the footage whole at the
//! first scale key, at the last key's scale from its time on, held to the clip's end, eased in and out. The project:
//! "talk.mov" (30 s) as c1 0-10 s and c2 10-30 s of the file.

use std::collections::HashSet;

use klipolo_chat::{AnalysisState, Compiled, FunctionCall, Inputs, Refs, ToolCall, View, compile};
use klipolo_model::layout::BoxN;
use klipolo_model::ops::Op;
use klipolo_model::project::Asset;
use klipolo_model::speech::Speech;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, ItemContent, TrackKind, ZoomTarget};
use klipolo_model::{Project, apply_ops};
use serde_json::{Value, json};

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn set_motion(layer: &str, keys: Value, presets: Value) -> ToolCall {
    ToolCall::parse(&FunctionCall {
        name: "setMotion".into(),
        call_id: "call_1".into(),
        arguments: json!({"layer": layer, "keys": keys, "presets": presets}).to_string(),
    })
    .unwrap()
}

fn key(property: &str, at: f64, value: f64) -> Value {
    json!({"property": property, "atSec": at, "value": value, "ease": null})
}

fn project() -> Project {
    let mut p = Project::new("Talk");
    let talk: Asset = serde_json::from_value(json!({
        "id": "talk", "kind": "video", "path": "/media/talk.mov", "name": "talk.mov", "durationUs": secs(30.0),
        "width": 1920, "height": 1080, "fps": 30.0, "hasAudio": true,
    }))
    .unwrap();
    p.assets = vec![talk];
    let mut a = Clip::new(ClipSource::Asset { id: "talk".into() }, 0, secs(10.0));
    a.id = "a".into();
    let mut b = Clip::new(
        ClipSource::Asset { id: "talk".into() },
        secs(10.0),
        secs(30.0),
    );
    b.id = "b".into();
    p.timeline.main = vec![a, b];
    p
}

fn inputs(p: &Project) -> Inputs {
    Inputs {
        message: "slowly zoom into the speaker over five seconds".into(),
        view: View::default(),
        refs: Refs::of(p),
        speech: Speech::default(),
        scenes: Default::default(),
        analysis: (p.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
        time_again: HashSet::new(),
        cursor: None,
        fonts: Default::default(),
        library: Default::default(),
    }
}

fn run(p: &Project, call: ToolCall) -> Compiled {
    compile(p, &inputs(p), &[call])
}

/// The one push the additions leave, on a zoom track: (start, duration, scale, area, ramp).
fn pushed(p: &Project, out: &Compiled) -> (Project, (Us, Us, f64, BoxN, Us)) {
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let tx = out.additions.as_ref().expect("applied at once");
    assert_eq!(tx.label, "chat.setMotion");
    let after = apply_ops(p, &tx.ops).unwrap();
    let zooms: Vec<_> = (after.timeline.tracks.iter())
        .filter(|t| t.kind == TrackKind::Zoom)
        .flat_map(|t| &t.items)
        .collect();
    assert_eq!(zooms.len(), 1);
    let z = zooms[0];
    let ItemContent::Zoom {
        target: ZoomTarget::Push { rect, ramp_us },
        scale,
    } = &z.content
    else {
        panic!("{:?}", z.content);
    };
    let found = (z.start_us, z.duration_us, *scale, *rect, *ramp_us);
    (after, found)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn scale_keys_on_a_clip_push_in_slowly_toward_a_point() {
    let p = project();
    let out = run(
        &p,
        set_motion(
            "c2",
            json!([
                key("scale", 2.0, 1.0),
                key("scale", 7.0, 1.3),
                key("x", 0.0, 0.5),
                key("y", 0.0, 0.4)
            ]),
            Value::Null,
        ),
    );
    let (after, (start, duration, scale, rect, ramp)) = pushed(&p, &out);
    // From 2 s into c2 (12 s on the timeline) to its end at 30 s, reaching 1.3 five seconds in.
    assert_eq!((start, duration, ramp), (secs(12.0), secs(18.0), secs(5.0)));
    assert!(near(scale, 1.3));
    let side = 1.0 / 1.3;
    assert!(near(rect.w, side) && near(rect.h, side), "{rect:?}");
    assert!(
        near(rect.x + rect.w / 2.0, 0.5) && near(rect.y + rect.h / 2.0, 0.4),
        "{rect:?}"
    );
    // It moves with the clip: attached to it.
    let z = (after.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| matches!(i.content, ItemContent::Zoom { .. }))
        .unwrap();
    assert_eq!(z.attach.as_ref().map(|a| a.clip_id.as_str()), Some("b"));
}

#[test]
fn one_scale_key_pushes_from_the_clips_start_toward_the_centre() {
    let p = project();
    let out = run(
        &p,
        set_motion("c1", json!([key("scale", 5.0, 1.25)]), Value::Null),
    );
    let (_, (start, duration, scale, rect, ramp)) = pushed(&p, &out);
    assert_eq!((start, duration, ramp), (0, secs(10.0), secs(5.0)));
    assert!(near(scale, 1.25));
    assert!(near(rect.x + rect.w / 2.0, 0.5) && near(rect.y + rect.h / 2.0, 0.5));
}

#[test]
fn a_push_it_cant_make_is_said_back() {
    let p = project();
    for (keys, presets, says) in [
        (
            json!([key("scale", 5.0, 1.2)]),
            json!("enter fade"),
            "no presets",
        ),
        (
            json!([key("rotation", 5.0, 10.0)]),
            Value::Null,
            "scale keys",
        ),
        (json!([key("scale", 5.0, 0.8)]), Value::Null, "more than 1"),
        (json!([key("scale", 5.0, 6.0)]), Value::Null, "at most 5"),
        (
            json!([
                key("scale", 1.0, 1.0),
                key("scale", 3.0, 1.2),
                key("scale", 6.0, 1.0)
            ]),
            Value::Null,
            "two scale keys",
        ),
        (
            json!([key("scale", 5.0, 1.2), key("x", 0.0, 1.5)]),
            Value::Null,
            "0 to 1",
        ),
        (json!([key("x", 0.0, 0.5)]), Value::Null, "scale key"),
    ] {
        let out = run(&p, set_motion("c1", keys.clone(), presets));
        assert_eq!(out.additions, None, "{keys}");
        assert_eq!(out.refusals.len(), 1, "{keys}");
        let reason = &out.refusals[0].reason;
        assert!(reason.starts_with("setMotion: c1"), "{reason}");
        assert!(reason.contains(says), "{keys}: {reason}");
    }
}

#[test]
fn no_keys_take_the_clips_pushes_away_as_a_proposal() {
    let p = project();
    let (after, _) = pushed(
        &p,
        &run(
            &p,
            set_motion("c2", json!([key("scale", 4.0, 1.2)]), Value::Null),
        ),
    );
    let out = run(&after, set_motion("c2", json!([]), Value::Null));
    assert_eq!(out.refusals, vec![]);
    assert_eq!(out.additions, None);
    let proposal = out.proposal.expect("taking a zoom away is proposed");
    assert!(matches!(
        proposal.tx.ops.as_slice(),
        [Op::RemoveItem { .. }]
    ));
}
