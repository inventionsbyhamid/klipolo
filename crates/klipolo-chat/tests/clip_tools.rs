//! The clip tools: `setClip` changes a clip's speed, volume, mute and Fit or Fill as the clip card does, and
//! `setTransition` adds, changes or removes the transition at a cut as Add transition and the transition card do. They
//! take exactly the values the cards offer, refuse any other with one plain sentence, and leave the project exactly as
//! the same edit by hand would.
//!
//! The project here, `clips()`: "talk.mov" (60 s, with sound) played in three clips c1 0-10 s, c2 10-30 s and c3
//! 30-60 s of the file, and "broll.mov" (20 s, no sound) as c4 2-12 s of its file.

use std::collections::{BTreeMap, HashMap, HashSet};

use klipolo_chat::compile::{NO_ROOM, NO_ROOM_ANYWHERE, NO_TRANSITIONS};
use klipolo_chat::tools::clips::SPEED_WORDS;
use klipolo_chat::tools::{ClipRef, TransitionKind};
use klipolo_chat::{
    AnalysisState, Audience, Compiled, FunctionCall, Inputs, Library, Problem, Refs, ToolCall,
    View, compile,
};
use klipolo_model::clip_settings::SPEEDS;
use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::contrib::builtin;
use klipolo_model::ops::{ClipPatch, Op, Origin};
use klipolo_model::project::{Asset, AssetKind, FormatKind};
use klipolo_model::speech::Speech;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Framing, FramingMode, Transition};
use klipolo_model::{Project, apply_ops};
use serde_json::json;

fn secs(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn parse(name: &str, arguments: serde_json::Value) -> Result<ToolCall, klipolo_chat::ToolError> {
    ToolCall::parse(&FunctionCall {
        name: name.into(),
        call_id: "call_1".into(),
        arguments: arguments.to_string(),
    })
}

/// `setClip` on `clip` with these values, the rest null.
fn set_clip(clip: &str, values: serde_json::Value) -> serde_json::Value {
    let mut args = json!({
        "clip": clip, "speed": null, "volume": null, "muted": null, "framing": null, "framingFormat": null,
    });
    for (k, v) in values.as_object().unwrap() {
        args[k] = v.clone();
    }
    args
}

#[test]
fn set_clip_reads_each_value_the_card_offers() {
    for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
        let read = parse("setClip", set_clip("c2", json!({"speed": speed}))).unwrap();
        assert_eq!(
            read,
            ToolCall::SetClip {
                clip: ClipRef::One("c2".into()),
                speed: Some(speed),
                volume: None,
                muted: None,
                framing: None,
            }
        );
    }
    let read = parse(
        "setClip",
        set_clip(
            "all",
            json!({"volume": 0.7, "muted": false, "framing": "fill", "framingFormat": "portrait"}),
        ),
    )
    .unwrap();
    assert_eq!(
        read,
        ToolCall::SetClip {
            clip: ClipRef::All,
            speed: None,
            volume: Some(0.7),
            muted: Some(false),
            framing: Some((FramingMode::Fill, Some(FormatKind::Portrait))),
        }
    );
    // Fit in every format the video has; the ends of the volume range; "All" read as all.
    let read = parse(
        "setClip",
        set_clip(" All ", json!({"framing": "fit", "volume": 2})),
    )
    .unwrap();
    assert_eq!(
        read,
        ToolCall::SetClip {
            clip: ClipRef::All,
            speed: None,
            volume: Some(2.0),
            muted: None,
            framing: Some((FramingMode::Fit, None)),
        }
    );
    assert!(parse("setClip", set_clip("c1", json!({"volume": 0}))).is_ok());
    // A format goes with framing: with none, no framing changes.
    let read = parse(
        "setClip",
        set_clip("c1", json!({"muted": true, "framingFormat": "portrait"})),
    );
    assert!(matches!(read, Ok(ToolCall::SetClip { framing: None, .. })));
    // A ref only names a clip here: whether one has it is the compiler's to judge.
    assert!(parse("setClip", set_clip("c99", json!({"muted": true}))).is_ok());
}

#[test]
fn set_clip_refuses_what_the_card_does_not_offer() {
    let said = |values| {
        parse("setClip", set_clip("c2", values))
            .unwrap_err()
            .to_string()
    };
    assert_eq!(
        said(json!({"speed": 3})),
        "setClip: speed must be one of 0.5, 0.75, 1, 1.25, 1.5, 2 or null, not 3."
    );
    assert_eq!(
        said(json!({"speed": 0.6})),
        "setClip: speed must be one of 0.5, 0.75, 1, 1.25, 1.5, 2 or null, not 0.6."
    );
    assert_eq!(
        said(json!({"volume": 2.5})),
        "setClip: volume must be between 0 and 2, not 2.5."
    );
    assert_eq!(
        said(json!({"volume": 0.333})),
        "setClip: volume must be in steps of 0.01, not 0.333."
    );
    assert_eq!(
        said(json!({"framing": "stretch"})),
        "setClip: framing must be one of fit, fill or null, not \"stretch\"."
    );
    assert_eq!(
        said(json!({})),
        "setClip: set at least one of speed, volume, muted or framing."
    );
    let empty = parse("setClip", set_clip("", json!({"muted": true}))).unwrap_err();
    assert_eq!(
        (empty.field.as_deref(), empty.problem),
        (Some("clip"), Problem::Empty)
    );
}

/// `setTransition` after `after` of `kind` lasting `duration`.
fn set_transition(after: &str, kind: &str, duration: serde_json::Value) -> serde_json::Value {
    json!({"after": after, "kind": kind, "durationSec": duration})
}

#[test]
fn set_transition_reads_each_kind_a_length_and_every_cut() {
    for (word, kind) in [
        ("dissolve", TransitionKind::Dissolve),
        ("dip", TransitionKind::Dip),
        ("push", TransitionKind::Push),
        ("wipe", TransitionKind::Wipe),
    ] {
        let read = parse("setTransition", set_transition("c1", word, json!(null))).unwrap();
        assert_eq!(
            read,
            ToolCall::SetTransition {
                after: ClipRef::One("c1".into()),
                kind: Some(kind),
                duration_us: None,
            }
        );
    }
    let read = parse("setTransition", set_transition("all", "none", json!(null))).unwrap();
    assert_eq!(
        read,
        ToolCall::SetTransition {
            after: ClipRef::All,
            kind: None,
            duration_us: None,
        }
    );
    let read = parse(
        "setTransition",
        set_transition("c2", "dissolve", json!(1.5)),
    )
    .unwrap();
    assert_eq!(
        read,
        ToolCall::SetTransition {
            after: ClipRef::One("c2".into()),
            kind: Some(TransitionKind::Dissolve),
            duration_us: Some(secs(1.5)),
        }
    );
}

#[test]
fn set_transition_refuses_what_the_card_does_not_offer() {
    let said = |args| parse("setTransition", args).unwrap_err().to_string();
    assert_eq!(
        said(set_transition("c1", "dissolve", json!(7))),
        "setTransition: durationSec must be between 0.1 and 5 seconds, not 7."
    );
    assert_eq!(
        said(set_transition("c1", "dissolve", json!(0.05))),
        "setTransition: durationSec must be between 0.1 and 5 seconds, not 0.05."
    );
    assert_eq!(
        said(set_transition("c1", "crossfade", json!(null))),
        "setTransition: kind must be one of dissolve, dip, push, wipe, none, not \"crossfade\"."
    );
    assert_eq!(
        said(set_transition("c1", "none", json!(0.5))),
        "setTransition: durationSec goes with a transition: make it null when kind is none."
    );
}

#[test]
fn the_speeds_a_refusal_lists_are_the_models() {
    let said: Vec<f64> = (SPEED_WORDS.iter()).map(|w| w.parse().unwrap()).collect();
    assert_eq!(said, SPEEDS);
}

#[test]
fn only_a_ref_names_the_timeline() {
    let one = parse("setClip", set_clip("c2", json!({"muted": true}))).unwrap();
    let all = parse("setClip", set_clip("all", json!({"muted": true}))).unwrap();
    assert!(one.names_timeline() && !all.names_timeline());
    let one = parse("setTransition", set_transition("c1", "dip", json!(null))).unwrap();
    let all = parse("setTransition", set_transition("all", "none", json!(null))).unwrap();
    assert!(one.names_timeline() && !all.names_timeline());
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
        made_with: None,
    }
}

/// Clip `id` playing `from` to `to` seconds of `asset`.
fn clip(id: &str, asset: &str, from: f64, to: f64) -> Clip {
    let mut c = Clip::new(ClipSource::Asset { id: asset.into() }, secs(from), secs(to));
    c.id = id.into();
    c
}

/// "talk.mov" (60 s, with sound) as c1 0-10 s, c2 10-30 s and c3 30-60 s of it, then "broll.mov" (20 s, no sound) as c4
/// 0-12 s of it, so no footage waits before c4 and the cut after c3 has no room for a transition. 30 fps, landscape.
fn clips() -> Project {
    let mut p = Project::new("Clips");
    p.assets = vec![file("talk", 60.0, true), file("broll", 20.0, false)];
    p.timeline.main = vec![
        clip("a", "talk", 0.0, 10.0),
        clip("b", "talk", 10.0, 30.0),
        clip("c", "talk", 30.0, 60.0),
        clip("d", "broll", 0.0, 12.0),
    ];
    p
}

/// "talk.mov" as c1 0-10 s and c2 10-59 s of it, then "broll.mov" as c3 2-12 s of it, with a 2 s dissolve from c2
/// into c3: c2 has 1 s of footage after its out point and c3 2 s before its in point, so the dissolve just fits. At 2x
/// c2's spare footage lasts 0.5 s, so it shortens to 1 s. With only 0.08 s after c2 the dissolve is 0.16 s long, and at
/// 2x it would have 0.08 s: under the shortest a transition lasts, so it no longer fits.
fn with_dissolve(spare_after: f64) -> Project {
    let mut p = Project::new("Dissolve");
    p.assets = vec![file("talk", 60.0, true), file("broll", 20.0, false)];
    p.timeline.main = vec![
        clip("a", "talk", 0.0, 10.0),
        clip("b", "talk", 10.0, 60.0 - spare_after),
        clip("c", "broll", 2.0, 12.0),
    ];
    let length = (2.0_f64).min(spare_after * 2.0);
    apply_ops(&p, &[set_transition_op("b", Some(dissolve(length)))]).unwrap()
}

fn dissolve(seconds: f64) -> Transition {
    Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(seconds),
        params: Default::default(),
    }
}

fn set_transition_op(clip_id: &str, transition: Option<Transition>) -> Op {
    Op::SetTransition {
        clip_id: clip_id.into(),
        transition,
    }
}

/// The clip card's op for `clip_id`: `clipOp(clip, patch)`.
fn card(clip_id: &str, patch: ClipPatch) -> Op {
    Op::UpdateClip {
        id: clip_id.into(),
        patch,
    }
}

fn inputs(p: &Project) -> Inputs {
    Inputs {
        message: String::new(),
        view: View::default(),
        refs: Refs::of(p),
        speech: Speech::default(),
        scenes: HashMap::new(),
        analysis: (p.assets.iter())
            .map(|a| (a.id.clone(), AnalysisState::Done))
            .collect(),
        time_again: HashSet::new(),
        library: Library::default(),
    }
}

fn run(p: &Project, calls: &[(&str, serde_json::Value)]) -> Compiled {
    let calls: Vec<ToolCall> = (calls.iter())
        .map(|(name, args)| parse(name, args.clone()).unwrap_or_else(|e| panic!("{e}")))
        .collect();
    compile(p, &inputs(p), &calls)
}

/// The additions' ops, applied at once with chat's label, and the project they leave.
fn applied(p: &Project, out: &Compiled) -> (Vec<Op>, String, Project) {
    assert_eq!(out.refusals, vec![], "{:#?}", out.refusals);
    let tx = out.additions.clone().expect("additions");
    assert_eq!(tx.origin, Some(Origin::Chat));
    assert!(!tx.needs_review(Some(p)), "an addition");
    let after = apply_ops(p, &tx.ops).unwrap();
    (tx.ops, tx.label, after)
}

fn params(pairs: &[(&str, ErrorParam)]) -> Option<BTreeMap<String, ErrorParam>> {
    Some(
        (pairs.iter())
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    )
}

/// The page's note on a refit, as a label's value.
fn refit(removed: usize, shortened: usize) -> ErrorParam {
    ErrorParam::Coded(Box::new(CodedError {
        code: "timeline.refit".into(),
        params: [
            ("removed".to_owned(), ErrorParam::from(removed)),
            ("shortened".to_owned(), ErrorParam::from(shortened)),
        ]
        .into(),
    }))
}

fn person_refusal(out: &Compiled) -> &str {
    let [r] = out.refusals.as_slice() else {
        panic!("one refusal: {:#?}", out.refusals)
    };
    assert_eq!(r.audience, Audience::Person, "{r:?}");
    &r.reason
}

fn model_refusal(out: &Compiled) -> &str {
    let [r] = out.refusals.as_slice() else {
        panic!("one refusal: {:#?}", out.refusals)
    };
    assert_eq!(r.audience, Audience::Model, "{r:?}");
    &r.reason
}

#[test]
fn a_speed_is_the_clip_cards_edit_applied_at_once() {
    let p = clips();
    let out = run(&p, &[("setClip", set_clip("c2", json!({"speed": 0.5})))]);
    let (ops, label, after) = applied(&p, &out);
    let by_hand = card(
        "b",
        ClipPatch {
            speed: Some(0.5),
            ..ClipPatch::default()
        },
    );
    assert_eq!(ops, std::slice::from_ref(&by_hand));
    assert_eq!(after, apply_ops(&p, &[by_hand]).unwrap());
    assert_eq!(after.timeline.main[1].duration_us(), secs(40.0));
    assert_eq!(label, "chat.setClip");
    assert_eq!(
        out.additions.unwrap().params,
        params(&[("clip", "c2".into()), ("change", "speed".into())])
    );
    assert_eq!(out.notes, Vec::<String>::new());
    // A speed it has already changes nothing, as picking it again in the card does.
    let again = run(
        &after,
        &[("setClip", set_clip("c2", json!({"speed": 0.5})))],
    );
    assert_eq!((again.additions, again.unchanged), (None, vec![0]));
}

#[test]
fn a_speed_that_shortens_a_transition_says_so_in_its_label_and_the_reply() {
    let p = with_dissolve(1.0);
    assert_eq!(p.timeline.main[1].transition_out, Some(dissolve(2.0)));
    let out = run(&p, &[("setClip", set_clip("c2", json!({"speed": 2})))]);
    let (ops, label, after) = applied(&p, &out);
    let by_hand = card(
        "b",
        ClipPatch {
            speed: Some(2.0),
            ..ClipPatch::default()
        },
    );
    assert_eq!(ops, std::slice::from_ref(&by_hand));
    assert_eq!(after, apply_ops(&p, &[by_hand]).unwrap());
    assert_eq!(
        after.timeline.main[1]
            .transition_out
            .as_ref()
            .map(|t| t.duration_us),
        Some(secs(1.0))
    );
    // As the card's `speed.undo.note`.
    assert_eq!(label, "chat.setClip.note");
    assert_eq!(
        out.additions.unwrap().params,
        params(&[
            ("clip", "c2".into()),
            ("change", "speed".into()),
            ("note", refit(0, 1)),
        ])
    );
    assert_eq!(out.notes, ["A transition is shortened."]);
}

#[test]
fn a_speed_that_leaves_a_transition_no_room_waits_for_the_person_and_says_why() {
    let p = with_dissolve(0.08);
    assert_eq!(
        p.timeline.main[1]
            .transition_out
            .as_ref()
            .map(|t| t.duration_us),
        Some(secs(0.16))
    );
    let out = run(&p, &[("setClip", set_clip("c2", json!({"speed": 2})))]);
    assert_eq!(
        (out.additions.clone(), out.refusals.clone()),
        (None, vec![])
    );
    let proposal = out
        .proposal
        .clone()
        .expect("proposed: it takes a transition away");
    assert_eq!(out.proposed_calls, [0]);
    assert_eq!(proposal.review.removals.transitions, ["b"]);
    assert_eq!(proposal.removed_us, 0);
    let by_hand = card(
        "b",
        ClipPatch {
            speed: Some(2.0),
            ..ClipPatch::default()
        },
    );
    assert_eq!(proposal.tx.ops, std::slice::from_ref(&by_hand));
    // Applied, it leaves the project as the card does, and its undo label says what it did.
    assert_eq!(proposal.review.after, apply_ops(&p, &[by_hand]).unwrap());
    assert_eq!(proposal.review.after.timeline.main[1].transition_out, None);
    assert_eq!(proposal.tx.label, "chat.setClip.note");
    assert_eq!(
        proposal.tx.params,
        params(&[
            ("clip", "c2".into()),
            ("change", "speed".into()),
            ("note", refit(1, 0)),
        ])
    );
    assert_eq!(
        out.notes,
        ["At that speed a transition no longer fits, so the change waits for you to apply it."]
    );
}

#[test]
fn volume_and_mute_are_the_cards_and_only_for_a_clip_with_sound() {
    let p = clips();
    let out = run(&p, &[("setClip", set_clip("c1", json!({"volume": 0.6})))]);
    let (ops, _, _) = applied(&p, &out);
    let volume = ClipPatch {
        volume: Some(0.6),
        ..ClipPatch::default()
    };
    assert_eq!(ops, [card("a", volume)]);
    // Muting every clip mutes those with sound; the b-roll has none.
    let out = run(&p, &[("setClip", set_clip("all", json!({"muted": true})))]);
    let (ops, _, after) = applied(&p, &out);
    let mute = |id| {
        card(
            id,
            ClipPatch {
                muted: Some(true),
                ..ClipPatch::default()
            },
        )
    };
    assert_eq!(ops, [mute("a"), mute("b"), mute("c")]);
    assert_eq!(
        out.additions.unwrap().params,
        params(&[("clip", "all".into()), ("change", "mute".into())])
    );
    assert!(!after.timeline.main[3].muted);
    // The b-roll alone has no Volume or Mute to set.
    let out = run(&p, &[("setClip", set_clip("c4", json!({"muted": true})))]);
    assert_eq!(
        model_refusal(&out),
        "setClip: c4 has no sound, so it has no volume or mute."
    );
    // A ref the context doesn't have.
    let out = run(&p, &[("setClip", set_clip("c9", json!({"speed": 2})))]);
    assert_eq!(model_refusal(&out), "setClip: there is no clip c9.");
    // Several values in one call: one step, labelled as several.
    let out = run(
        &p,
        &[(
            "setClip",
            set_clip("c1", json!({"speed": 1.5, "volume": 0.5, "muted": true})),
        )],
    );
    let (ops, _, _) = applied(&p, &out);
    let several = ClipPatch {
        speed: Some(1.5),
        volume: Some(0.5),
        muted: Some(true),
        ..ClipPatch::default()
    };
    assert_eq!(ops, [card("a", several)]);
    assert_eq!(
        out.additions.unwrap().params,
        params(&[("clip", "c1".into()), ("change", "several".into())])
    );
}

#[test]
fn fit_or_fill_is_the_cards_in_the_format_named_or_every_format() {
    let mut p = clips();
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    // An imported video fills every format by default: Fill changes nothing.
    let out = run(
        &p,
        &[(
            "setClip",
            set_clip(
                "c1",
                json!({"framing": "fill", "framingFormat": "portrait"}),
            ),
        )],
    );
    assert_eq!((out.additions, out.unchanged), (None, vec![0]));
    // Fit in portrait: the card's framing there, built on the centre fill it showed, with only the mode changed.
    let out = run(
        &p,
        &[(
            "setClip",
            set_clip("c1", json!({"framing": "fit", "framingFormat": "portrait"})),
        )],
    );
    let (ops, _, after) = applied(&p, &out);
    let fit = Framing {
        mode: FramingMode::Fit,
        focus_x: 0.5,
        focus_y: 0.5,
        follow: None,
    };
    let by_hand = ClipPatch {
        framing: Some([(FormatKind::Portrait, fit)].into()),
        ..ClipPatch::default()
    };
    assert_eq!(ops, [card("a", by_hand)]);
    assert_eq!(
        out.additions.unwrap().params,
        params(&[("clip", "c1".into()), ("change", "fit".into())])
    );
    // A clip's own framing in another format stays, and every format of the video gets Fit when none is named.
    let out = run(
        &after,
        &[("setClip", set_clip("c1", json!({"framing": "fit"})))],
    );
    let (ops, _, _) = applied(&after, &out);
    let both = ClipPatch {
        framing: Some([(FormatKind::Landscape, fit), (FormatKind::Portrait, fit)].into()),
        ..ClipPatch::default()
    };
    assert_eq!(ops, [card("a", both)]);
    // A format the video doesn't have.
    let out = run(
        &p,
        &[(
            "setClip",
            set_clip("c1", json!({"framing": "fit", "framingFormat": "square"})),
        )],
    );
    assert_eq!(
        model_refusal(&out),
        "setClip: the video has no square format. Add it with setFormat first."
    );
}

#[test]
fn a_new_transition_is_add_transitions_on_the_frame_grid() {
    let p = clips();
    let out = run(
        &p,
        &[(
            "setTransition",
            set_transition("c1", "dissolve", json!(null)),
        )],
    );
    let (ops, label, after) = applied(&p, &out);
    // As Add transition makes it: a dissolve of half a second at 30 fps, no settings.
    assert_eq!(ops, [set_transition_op("a", Some(dissolve(0.5)))]);
    assert_eq!(label, "chat.setTransition");
    assert_eq!(
        out.additions.unwrap().params,
        params(&[
            ("after", "c1".into()),
            ("change", "add".into()),
            ("count", ErrorParam::from(1usize)),
        ])
    );
    // A length goes on the frame grid, as the card's Length does.
    let out = run(
        &p,
        &[(
            "setTransition",
            set_transition("c1", "push", json!(1.234567)),
        )],
    );
    let (ops, _, _) = applied(&p, &out);
    let push = Transition {
        effect: builtin::TRANSITION_PUSH.into(),
        duration_us: 1_233_333,
        params: Default::default(),
    };
    assert_eq!(ops, [set_transition_op("a", Some(push))]);
    // A new kind starts from its own defaults and keeps the length, as the card's Type does.
    let mut dip_red = dissolve(0.5);
    dip_red.effect = builtin::TRANSITION_DIP.into();
    dip_red.params = [("color".to_owned(), json!("#ff0000"))]
        .into_iter()
        .map(|(k, v)| (k, serde_json::from_value(v).unwrap()))
        .collect();
    let red = apply_ops(&p, &[set_transition_op("a", Some(dip_red))]).unwrap();
    let out = run(
        &red,
        &[("setTransition", set_transition("c1", "wipe", json!(null)))],
    );
    let (ops, _, _) = applied(&red, &out);
    let mut wipe = dissolve(0.5);
    wipe.effect = builtin::TRANSITION_WIPE.into();
    assert_eq!(ops, [set_transition_op("a", Some(wipe))]);
    assert_eq!(
        out.additions.unwrap().params.unwrap()["change"],
        ErrorParam::from("change")
    );
    // The same kind and length again changes nothing.
    let out = run(
        &after,
        &[(
            "setTransition",
            set_transition("c1", "dissolve", json!(null)),
        )],
    );
    assert_eq!((out.additions, out.unchanged), (None, vec![0]));
}

#[test]
fn a_transition_is_refused_where_the_cut_has_no_room_or_no_cut_follows() {
    let p = clips();
    // c1 and c2 play one file back to back: 10 s of footage on either side, so 5 s fit.
    let out = run(
        &p,
        &[(
            "setTransition",
            set_transition("c3", "dissolve", json!(null)),
        )],
    );
    assert_eq!(person_refusal(&out), NO_ROOM);
    let out = run(
        &p,
        &[(
            "setTransition",
            set_transition("c4", "dissolve", json!(null)),
        )],
    );
    assert_eq!(
        model_refusal(&out),
        "setTransition: c4 is the last clip, so no cut follows it."
    );
    // More than the cut has room for: the model is told the most, never given less.
    let tight = with_dissolve(1.0);
    let out = run(
        &tight,
        &[("setTransition", set_transition("c2", "dissolve", json!(3)))],
    );
    assert_eq!(
        model_refusal(&out),
        "setTransition: the cut after c2 has room for at most 2.0 s."
    );
    assert_eq!(out.additions, None);
}

#[test]
fn every_cut_takes_one_where_it_has_room() {
    let p = clips();
    let out = run(
        &p,
        &[(
            "setTransition",
            set_transition("all", "dissolve", json!(null)),
        )],
    );
    let (ops, _, _) = applied(&p, &out);
    // The cut after c3 has no footage before c4's in point: left as it is.
    assert_eq!(
        ops,
        [
            set_transition_op("a", Some(dissolve(0.5))),
            set_transition_op("b", Some(dissolve(0.5))),
        ]
    );
    assert_eq!(
        out.additions.unwrap().params.unwrap()["count"],
        ErrorParam::from(2usize)
    );
    // Where none has room, the person is told.
    let mut bare = clips();
    bare.timeline.main = vec![clip("a", "talk", 0.0, 60.0), clip("d", "broll", 0.0, 12.0)];
    let out = run(
        &bare,
        &[(
            "setTransition",
            set_transition("all", "dissolve", json!(null)),
        )],
    );
    assert_eq!(person_refusal(&out), NO_ROOM_ANYWHERE);
}

#[test]
fn removing_transitions_is_proposed_as_the_cards_remove() {
    let p = with_dissolve(1.0);
    let out = run(
        &p,
        &[("setTransition", set_transition("all", "none", json!(null)))],
    );
    assert_eq!(
        (out.additions.clone(), out.refusals.clone()),
        (None, vec![])
    );
    let proposal = out.proposal.expect("taking a transition away is proposed");
    let by_hand = set_transition_op("b", None);
    assert_eq!(proposal.tx.ops, std::slice::from_ref(&by_hand));
    assert_eq!(proposal.review.after, apply_ops(&p, &[by_hand]).unwrap());
    assert_eq!(proposal.review.removals.transitions, ["b"]);
    assert_eq!(proposal.tx.label, "chat.setTransition");
    assert_eq!(
        proposal.tx.params,
        params(&[
            ("after", "all".into()),
            ("change", "remove".into()),
            ("count", ErrorParam::from(1usize)),
        ])
    );
    // None to remove.
    let out = run(
        &clips(),
        &[("setTransition", set_transition("all", "none", json!(null)))],
    );
    assert_eq!(person_refusal(&out), NO_TRANSITIONS);
    let out = run(
        &clips(),
        &[("setTransition", set_transition("c1", "none", json!(null)))],
    );
    assert_eq!(
        model_refusal(&out),
        "setTransition: the cut after c1 has no transition to remove."
    );
}

#[test]
fn a_speed_and_a_transition_in_one_answer_are_one_step_in_call_order() {
    let p = clips();
    let out = run(
        &p,
        &[
            ("setClip", set_clip("c1", json!({"speed": 0.5}))),
            (
                "setTransition",
                set_transition("c1", "dissolve", json!(null)),
            ),
        ],
    );
    let (ops, label, after) = applied(&p, &out);
    let slow = card(
        "a",
        ClipPatch {
            speed: Some(0.5),
            ..ClipPatch::default()
        },
    );
    assert_eq!(
        ops,
        [slow.clone(), set_transition_op("a", Some(dissolve(0.5)))]
    );
    assert_eq!(label, "chat.andMore");
    let by_hand = apply_ops(
        &apply_ops(&p, &[slow]).unwrap(),
        &[set_transition_op("a", Some(dissolve(0.5)))],
    )
    .unwrap();
    assert_eq!(after, by_hand);
}
