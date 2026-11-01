mod common;
use common::*;
use klipolo_model::contrib::builtin;
use klipolo_model::layout::BoxN;
use klipolo_model::ops::{
    op_tags, ClipPatch, Edge, EffectTarget, ItemPatch, Op, OpRisk, Origin, TrackTarget, Transaction,
};
use klipolo_model::params::ParamValue;
use klipolo_model::project::{FormatKind, Settings};
use klipolo_model::risk::{removals, Removals};
use klipolo_model::timeline::{
    Clip, ClipSource, EffectInstance, Item, ItemContent, TrackKind, Transition, ZoomTarget,
};
use klipolo_model::{apply_ops, Project, RangeUs};

fn after(p: &Project, ops: Vec<Op>) -> Project {
    apply_ops(p, &ops).unwrap()
}

fn range(a: f64, b: f64) -> RangeUs {
    RangeUs {
        start: secs(a),
        end: secs(b),
    }
}

#[test]
fn a_cut_lists_the_footage_it_removes_in_timeline_time() {
    let p = fixture();
    let r = removals(
        &p,
        &after(
            &p,
            vec![Op::DeleteRanges {
                ranges: vec![range(5.0, 8.0)],
            }],
        ),
    );
    assert_eq!(r.ranges, vec![range(5.0, 8.0)]);
    assert!(r.clips.is_empty());
}

#[test]
fn removing_a_clip_lists_it_and_its_span() {
    let p = fixture();
    let r = removals(&p, &after(&p, vec![Op::RemoveClip { id: "b".into() }]));
    assert_eq!(r.clips, vec!["b".to_string()]);
    assert_eq!(r.ranges, vec![range(20.0, 30.0)]);
}

#[test]
fn trimming_a_faster_clip_maps_source_time_to_timeline_time() {
    // "a" plays its first 20 s of source at 2×: 10 s on the timeline. Trimming its source end to 10 s removes 5–10.
    let p = after(
        &fixture(),
        vec![Op::UpdateClip {
            id: "a".into(),
            patch: ClipPatch {
                speed: Some(2.0),
                ..Default::default()
            },
        }],
    );
    let r = removals(
        &p,
        &after(
            &p,
            vec![Op::TrimClip {
                id: "a".into(),
                edge: Edge::End,
                source_us: secs(10.0),
            }],
        ),
    );
    assert_eq!(r.ranges, vec![range(5.0, 10.0)]);
}

#[test]
fn splits_and_moves_remove_nothing() {
    let p = fixture();
    let next = after(
        &p,
        vec![
            Op::Split { at_us: secs(5.0) },
            Op::MoveClip {
                id: "b".into(),
                index: 0,
            },
        ],
    );
    assert!(removals(&p, &next).is_empty());
}

#[test]
fn a_faster_video_layer_keeps_its_footage() {
    // Shorter on the timeline, the same footage: not "shortened" (the measure `risk_against` uses).
    let mut p = fixture();
    p.timeline.tracks = vec![overlay_track("T", vec![video_item("v", 0.0, 4.0, 0.0)])];
    let tx = Transaction {
        label: "edit.speed".into(),
        ops: vec![Op::UpdateItem {
            id: "v".into(),
            patch: klipolo_model::ops::ItemPatch {
                content: Some(klipolo_model::timeline::ItemContent::Video {
                    asset_id: "broll".into(),
                    source_in_us: 0,
                    speed: 2.0,
                    volume: 1.0,
                    muted: false,
                    mirror: false,
                    tie: None,
                }),
                ..Default::default()
            },
        }],
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    let review = tx.review(&p).unwrap();
    assert!(
        review.removals.is_empty() && review.risk == OpRisk::Additive,
        "{:?}",
        review.removals
    );
}

#[test]
fn layers_effects_and_transitions_that_go_are_listed() {
    let mut p = fixture();
    p.timeline.main[1].source_in_us = secs(2.0); // spare footage before "b", so "a" can take a transition
    let mut title = text_item("t", 2.0, 4.0);
    title.effects = vec![EffectInstance {
        id: "fx".into(),
        ..EffectInstance::new(builtin::EFFECT_BLUR)
    }];
    p.timeline.tracks = vec![overlay_track("T", vec![title])];
    let dissolve = Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(1.0),
        params: Default::default(),
    };
    let p = after(
        &p,
        vec![Op::SetTransition {
            clip_id: "a".into(),
            transition: Some(dissolve),
        }],
    );
    let next = after(
        &p,
        vec![
            Op::TrimItem {
                id: "t".into(),
                edge: Edge::End,
                at_us: secs(4.0),
            },
            Op::RemoveEffect {
                target: EffectTarget::Item { id: "t".into() },
                id: "fx".into(),
            },
            Op::SetTransition {
                clip_id: "a".into(),
                transition: None,
            },
        ],
    );
    let r = removals(&p, &next);
    assert_eq!(
        (r.shortened.as_slice(), r.effects.as_slice()),
        (&["t".to_string()][..], &["t".to_string()][..])
    );
    assert_eq!(r.transitions, vec!["a".to_string()]);
    assert!(r.ranges.is_empty() && r.items.is_empty());
    assert_eq!(r.count(), 3);
}

#[test]
fn external_agents_and_chat_are_judged_by_what_the_batch_removes() {
    let p = fixture();
    let tx = |origin: Option<Origin>, ops: Vec<Op>| Transaction {
        label: "edit.rename".into(),
        ops,
        origin,
        ..Default::default()
    };
    let external = || {
        Some(Origin::External {
            client: "Example Client".into(),
        })
    };
    let rename = || vec![Op::Rename { name: "New".into() }];
    let cut = || {
        vec![Op::DeleteRanges {
            ranges: vec![range(5.0, 8.0)],
        }]
    };
    assert!(
        !tx(external(), rename()).needs_review(Some(&p)),
        "whether an app's additions wait is MCP's trust level"
    );
    assert!(tx(external(), cut()).needs_review(Some(&p)));
    assert!(!tx(Some(Origin::Chat), rename()).needs_review(Some(&p)));
    assert!(tx(Some(Origin::Chat), cut()).needs_review(Some(&p)));
    assert!(
        !tx(None, cut()).needs_review(Some(&p)),
        "the person's own edits never wait"
    );
    let json = serde_json::to_value(Origin::External {
        client: "Example Client".into(),
    })
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!({"type": "external", "client": "Example Client"})
    );
}

#[test]
fn a_review_applies_once_and_agrees_with_the_risk() {
    let p = fixture();
    let tx = Transaction {
        label: "edit.cut".into(),
        ops: vec![Op::DeleteRanges {
            ranges: vec![range(5.0, 8.0)],
        }],
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    let review = tx.review(&p).unwrap();
    assert_eq!(review.after, apply_ops(&p, &tx.ops).unwrap());
    assert_eq!(
        (review.risk, review.removals.ranges.clone()),
        (OpRisk::Removal, vec![range(5.0, 8.0)])
    );
    assert_eq!(tx.risk_against(&p), OpRisk::Removal);
    let gone = Transaction {
        ops: vec![Op::RemoveClip { id: "nope".into() }],
        ..tx
    };
    assert!(gone.review(&p).is_err());
}

#[test]
fn the_op_schema_lists_every_op_and_tags_match_the_json() {
    let tags = op_tags();
    // The first 34, then `fixWords`, then five recording ops in place of `detachCamera`, then `setWordLook`, then
    // `setCursorStyle`, then `useRecording` and `addRecordingSource`, then `detachSound`, `freezeFrame` and
    // `reverseClip`.
    assert_eq!(tags.len(), 34 + 1 + 4 + 1 + 1 + 2 + 3, "{tags:?}");
    for op in [
        Op::Split { at_us: 0 },
        Op::RemoveClip { id: "a".into() },
        Op::SetTransition {
            clip_id: "a".into(),
            transition: None,
        },
    ] {
        assert_eq!(serde_json::to_value(&op).unwrap()["type"], op.tag());
        assert!(tags.iter().any(|t| t == op.tag()));
    }
}

/// A word's look restyles it, hiding included: a hidden word stays in the word list, and undo brings it back.
#[test]
fn a_words_look_is_additive() {
    let op = Op::SetWordLook {
        asset_id: "a".into(),
        ranges: vec![klipolo_model::time::RangeUs { start: 0, end: 1 }],
        look: klipolo_model::captions::looks::Look {
            hidden: Some(true),
            ..Default::default()
        },
    };
    assert_eq!(klipolo_model::ops::op_risk(&op), OpRisk::Additive);
}

/// Where a piece takes over what its original had: a split layer's second piece holds the rest, and a split
/// clip's second piece carries its transition. Nothing is listed, as nothing is judged removed.
#[test]
fn split_pieces_keep_what_their_original_had() {
    let mut p = fixture();
    p.timeline.main[1].source_in_us = secs(2.0);
    p.timeline.tracks = vec![overlay_track("T", vec![text_item("t", 2.0, 4.0)])];
    let dissolve = Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(1.0),
        params: Default::default(),
    };
    let p = after(
        &p,
        vec![Op::SetTransition {
            clip_id: "a".into(),
            transition: Some(dissolve),
        }],
    );
    let ops = vec![
        Op::SplitItem {
            id: "t".into(),
            at_us: secs(3.0),
        },
        Op::Split { at_us: secs(10.0) },
    ];
    let tx = Transaction {
        label: "edit.split".into(),
        ops,
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    let review = tx.review(&p).unwrap();
    assert!(
        review.removals.is_empty() && review.risk == OpRisk::Additive,
        "{:?}",
        review.removals
    );
}

/// Lineage, as for footage: only what the batch itself adds, holding the same, makes up for what goes. Something
/// longer that was already there, or new of another text, picture stretch or clip, doesn't.
#[test]
fn something_added_elsewhere_doesnt_make_up_for_what_goes() {
    let review = |p: &Project, ops: Vec<Op>| {
        let tx = Transaction {
            label: "edit.test".into(),
            ops,
            origin: Some(Origin::Chat),
            ..Default::default()
        };
        let r = tx.review(p).unwrap();
        assert_eq!(r.risk, OpRisk::Removal, "{:?}", r.removals);
        r.removals
    };
    let shortened = |ids: &[&str]| Removals {
        shortened: ids.iter().map(|id| id.to_string()).collect(),
        ..Default::default()
    };

    // A title cut to 1 s, and a new text of other words.
    let mut p = fixture();
    p.timeline.tracks = vec![overlay_track("T", vec![text_item("t", 2.0, 4.0)])];
    let mut words = text_item("n", 10.0, 3.0);
    if let ItemContent::Text { text, .. } = &mut words.content {
        *text = "New".into();
    }
    let trim = |id: &str, at: f64| Op::TrimItem {
        id: id.into(),
        edge: Edge::End,
        at_us: secs(at),
    };
    let add = |item| Op::AddItem {
        track: TrackTarget::Auto,
        item,
        attach: false,
    };
    assert_eq!(
        review(&p, vec![trim("t", 3.0), add(words)]),
        shortened(&["t"])
    );

    // A logo cut by 5 s, and another picture of it 5 s longer.
    let image = |id: &str, start, len| Item {
        content: ItemContent::Image {
            asset_id: "logo".into(),
        },
        ..still_item(id, start, len)
    };
    p.timeline.tracks = vec![overlay_track(
        "T",
        vec![image("logo-a", 0.0, 10.0), image("logo-b", 12.0, 8.0)],
    )];
    let longer = Op::UpdateItem {
        id: "logo-b".into(),
        patch: ItemPatch {
            duration_us: Some(secs(13.0)),
            ..Default::default()
        },
    };
    assert_eq!(
        review(&p, vec![trim("logo-a", 5.0), longer]),
        shortened(&["logo-a"])
    );

    // The music cut to its first 30 s, and the file's start added again later.
    let music = |id: &str, start, len| Item {
        content: ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        ..still_item(id, start, len)
    };
    let mut track = overlay_track("A", vec![music("m", 0.0, 60.0)]);
    track.kind = TrackKind::Audio;
    p.timeline.tracks = vec![track];
    assert_eq!(
        review(&p, vec![trim("m", 30.0), add(music("m2", 40.0, 30.0))]),
        shortened(&["m"])
    );

    // The dissolve after "a" taken away, and one given to the next clip of the same recording.
    let mut p = fixture();
    p.timeline.main[1].source_in_us = secs(2.0);
    let dissolve = || Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(1.0),
        params: Default::default(),
    };
    let set = |clip: &str, t| Op::SetTransition {
        clip_id: clip.into(),
        transition: t,
    };
    let p = after(
        &p,
        vec![Op::Split { at_us: secs(10.0) }, set("a", Some(dissolve()))],
    );
    let swapped = review(&p, vec![set("a", None), set("a-2", Some(dissolve()))]);
    assert_eq!(swapped.transitions, vec!["a".to_string()]);
}

fn chat(ops: Vec<Op>) -> Transaction {
    Transaction {
        label: "edit.test".into(),
        ops,
        origin: Some(Origin::Chat),
        ..Default::default()
    }
}

/// A zoom, an adjustment layer or a generator makes up for one that lost time only when it shows the same: the same
/// target and scale, the same effects, the same contribution and settings.
#[test]
fn only_a_new_layer_that_shows_the_same_makes_up_for_one_cut_short() {
    let trim = |id: &str, at: f64| Op::TrimItem {
        id: id.into(),
        edge: Edge::End,
        at_us: secs(at),
    };
    let add = |item| Op::AddItem {
        track: TrackTarget::Auto,
        item,
        attach: false,
    };
    let risk = |p: &Project, ops| chat(ops).review(p).unwrap();

    let zoom = |id: &str, start, len, target| Item {
        content: ItemContent::Zoom { target, scale: 2.0 },
        ..still_item(id, start, len)
    };
    let corner = || ZoomTarget::Rect {
        rect: BoxN {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        },
    };
    let mut p = fixture();
    let mut track = overlay_track("Z", vec![zoom("z1", 2.0, 4.0, corner())]);
    track.kind = TrackKind::Zoom;
    p.timeline.tracks = vec![track];
    let elsewhere = risk(
        &p,
        vec![
            trim("z1", 3.0),
            add(zoom("z2", 10.0, 3.0, ZoomTarget::Auto { using: vec![] })),
        ],
    );
    assert_eq!(
        (elsewhere.risk, elsewhere.removals.shortened),
        (OpRisk::Removal, vec!["z1".to_string()])
    );
    let same = risk(
        &p,
        vec![trim("z1", 3.0), add(zoom("z2", 10.0, 3.0, corner()))],
    );
    assert_eq!(same.risk, OpRisk::Additive, "{:?}", same.removals);

    let adjustment = |id: &str, start, len, effect: &str| Item {
        content: ItemContent::Adjustment,
        effects: vec![EffectInstance {
            id: "fx".into(),
            ..EffectInstance::new(effect)
        }],
        ..still_item(id, start, len)
    };
    p.timeline.tracks = vec![overlay_track(
        "T",
        vec![adjustment("blur", 0.0, 10.0, builtin::EFFECT_BLUR)],
    )];
    let grade = risk(
        &p,
        vec![
            trim("blur", 5.0),
            add(adjustment("grade", 12.0, 5.0, builtin::EFFECT_VIGNETTE)),
        ],
    );
    assert_eq!(
        (grade.risk, grade.removals.shortened),
        (OpRisk::Removal, vec!["blur".to_string()])
    );
    let again = risk(
        &p,
        vec![
            trim("blur", 5.0),
            add(adjustment("blur-2", 12.0, 5.0, builtin::EFFECT_BLUR)),
        ],
    );
    assert_eq!(again.risk, OpRisk::Additive, "{:?}", again.removals);
    // The same blur over another area hides something else: what the first one hid shows again.
    let mut elsewhere = adjustment("blur-3", 12.0, 5.0, builtin::EFFECT_BLUR);
    elsewhere.layout.rect = BoxN {
        x: 0.5,
        y: 0.5,
        w: 0.4,
        h: 0.1,
    };
    let moved = risk(&p, vec![trim("blur", 5.0), add(elsewhere)]);
    assert_eq!(
        (moved.risk, moved.removals.shortened),
        (OpRisk::Removal, vec!["blur".to_string()])
    );
    // What the adjustment draw uses counts: a fainter blur hides less. Its rotation isn't drawn, so it doesn't.
    let mut fainter = adjustment("blur-4", 12.0, 5.0, builtin::EFFECT_BLUR);
    fainter.layout.opacity = 0.5;
    let fainter = risk(&p, vec![trim("blur", 5.0), add(fainter)]);
    assert_eq!(
        (fainter.risk, fainter.removals.shortened),
        (OpRisk::Removal, vec!["blur".to_string()])
    );
    let mut turned = adjustment("blur-5", 12.0, 5.0, builtin::EFFECT_BLUR);
    turned.layout.rotation = 15.0;
    let turned = risk(&p, vec![trim("blur", 5.0), add(turned)]);
    assert_eq!(turned.risk, OpRisk::Additive, "{:?}", turned.removals);
}

/// A lost transition is made up only by the piece now ending where its clip ended, never by a new clip of the same
/// recording elsewhere; a split that leaves the transition on its last piece loses nothing.
#[test]
fn a_new_clip_elsewhere_doesnt_make_up_for_a_lost_transition() {
    let mut p = fixture();
    p.timeline.main[1].source_in_us = secs(2.0);
    let dissolve = || Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(1.0),
        params: Default::default(),
    };
    let set = |clip: &str, t| Op::SetTransition {
        clip_id: clip.into(),
        transition: t,
    };
    let p = after(
        &p,
        vec![Op::Split { at_us: secs(10.0) }, set("a", Some(dissolve()))],
    );
    let mut x = Clip::new(
        ClipSource::Recording { id: "rec".into() },
        secs(30.0),
        secs(40.0),
    );
    x.id = "x".into();
    x.transition_out = Some(dissolve());
    let insert = Op::InsertClip {
        clip: x,
        index: Some(2),
    };
    let swapped = chat(vec![set("a", None), insert]).review(&p).unwrap();
    assert_eq!(
        (swapped.risk, swapped.removals.transitions),
        (OpRisk::Removal, vec!["a".to_string()])
    );
    let split = chat(vec![Op::Split { at_us: secs(5.0) }])
        .review(&p)
        .unwrap();
    assert_eq!(split.risk, OpRisk::Additive, "{:?}", split.removals);

    // "Move the dissolve to 0:05": the piece now meeting "a-2" has none, though "a" carries a new one.
    let moved = chat(vec![
        set("a", None),
        Op::Split { at_us: secs(5.0) },
        set("a", Some(dissolve())),
    ])
    .review(&p)
    .unwrap();
    assert_eq!(
        (moved.risk, moved.removals.transitions),
        (OpRisk::Removal, vec!["a".to_string()])
    );
    // A longer clip keeps its transition, and a cut inside it leaves it on the piece meeting "a-2".
    let longer = Op::TrimClip {
        id: "a".into(),
        edge: Edge::End,
        source_us: secs(11.0),
    };
    let longer = chat(vec![longer]).review(&p).unwrap();
    assert_eq!(longer.risk, OpRisk::Additive, "{:?}", longer.removals);
    let cut = chat(vec![Op::DeleteRanges {
        ranges: vec![range(2.0, 3.0)],
    }])
    .review(&p)
    .unwrap();
    assert!(cut.removals.transitions.is_empty(), "{:?}", cut.removals);
}

/// Two clips of one recording that end at the same moment (a replay of a clip's tail) are judged apart, in either
/// order: a clip whose end didn't move keeps its transition only if it still carries one, and only a clip whose end
/// moved (a split or a cut) is judged by the piece now ending there.
#[test]
fn clips_ending_at_the_same_source_point_are_judged_apart() {
    let dissolve = || Transition {
        effect: builtin::TRANSITION_DISSOLVE.into(),
        duration_us: secs(0.5),
        params: Default::default(),
    };
    let set = |clip: &str, t| Op::SetTransition {
        clip_id: clip.into(),
        transition: t,
    };
    let clip = |id: &str, from: f64, to: f64| {
        let mut c = Clip::new(
            ClipSource::Recording { id: "rec".into() },
            secs(from),
            secs(to),
        );
        c.id = id.into();
        c.transition_out = Some(dissolve());
        c
    };
    let base = fixture();
    let mut b = base.timeline.main[1].clone();
    b.source_in_us = secs(2.0);
    // "a" plays 2–10 s of the recording and "replay" its last 5 s again; both end at 10 s with a dissolve.
    for replay_first in [false, true] {
        let mut p = base.clone();
        let (a, replay) = (clip("a", 2.0, 10.0), clip("replay", 5.0, 10.0));
        p.timeline.main = if replay_first {
            vec![replay, a, b.clone()]
        } else {
            vec![a, replay, b.clone()]
        };
        let split_at = if replay_first { 2.5 } else { 10.5 };
        // The dissolve after "a" goes; "replay" is split, its last piece keeps its dissolve and it gets a new one.
        let swap = chat(vec![
            set("a", None),
            Op::Split {
                at_us: secs(split_at),
            },
            set("replay", Some(dissolve())),
        ])
        .review(&p)
        .unwrap();
        assert_eq!(
            (swap.risk, swap.removals.transitions),
            (OpRisk::Removal, vec!["a".to_string()]),
            "replay first: {replay_first}"
        );
        // Another copy of the tail, with no transition, takes nothing from "a", which still carries its own.
        let mut copy = clip("copy", 6.0, 10.0);
        copy.transition_out = None;
        let added = chat(vec![Op::InsertClip {
            clip: copy,
            index: Some(3),
        }])
        .review(&p)
        .unwrap();
        assert_eq!(
            added.risk,
            OpRisk::Additive,
            "replay first: {replay_first}: {:?}",
            added.removals
        );
    }
}

/// A clip's footage is kept by its own pieces, whatever their look (a new speed or volume on a piece is an edit, as on
/// the whole clip), and never by a fresh copy of the same file.
#[test]
fn only_a_clips_own_pieces_keep_its_footage() {
    let p = fixture();
    let judge = |ops: Vec<Op>| chat(ops).review(&p).unwrap();
    let patch = |id: &str, patch: ClipPatch| Op::UpdateClip {
        id: id.into(),
        patch,
    };
    // "b" (broll 0–10 s, at 20–30 s) loses 2–5 s, and the whole file goes in again: even an id shaped like a piece's
    // doesn't make it one, since the review knows the batch inserted it.
    for id in ["fresh", "b-2"] {
        let mut copy = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(10.0));
        copy.id = id.into();
        let r = judge(vec![
            Op::InsertClip {
                clip: copy,
                index: None,
            },
            Op::DeleteRanges {
                ranges: vec![range(22.0, 25.0)],
            },
        ]);
        assert_eq!(
            (r.risk, r.removals.ranges),
            (OpRisk::Removal, vec![range(22.0, 25.0)]),
            "{id}"
        );
    }
    // "Speed up 0:05–0:10": two splits, and the middle piece twice as fast.
    let faster = ClipPatch {
        speed: Some(2.0),
        ..Default::default()
    };
    let ramp = judge(vec![
        Op::Split { at_us: secs(5.0) },
        Op::Split { at_us: secs(10.0) },
        patch("a-2", faster),
    ]);
    assert!(
        ramp.removals.is_empty() && ramp.risk == OpRisk::Additive,
        "{:?}",
        ramp.removals
    );
    // A split piece made quieter.
    let quieter = ClipPatch {
        volume: Some(0.3),
        ..Default::default()
    };
    let r = judge(vec![Op::Split { at_us: secs(25.0) }, patch("b-2", quieter)]);
    assert!(
        r.removals.is_empty() && r.risk == OpRisk::Additive,
        "{:?}",
        r.removals
    );
    // A cut lists only the part it cuts.
    let cut = judge(vec![Op::DeleteRanges {
        ranges: vec![range(22.0, 25.0)],
    }]);
    assert_eq!(cut.removals.ranges, vec![range(22.0, 25.0)]);
}

/// Dropping a format is a removal only when something loses work stored for it: the captions' place for portrait.
/// The camera's portrait mode and a title placed by hand for portrait stay in the file (they come back with the
/// format), so they aren't.
#[test]
fn dropping_a_format_is_a_removal_only_with_work_stored_for_it() {
    let mut p = fixture();
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    p.timeline.tracks = vec![overlay_track("T", vec![text_item("t", 2.0, 4.0)])];
    let p = after(&p, vec![]);
    assert!(p.timeline.main[0]
        .camera
        .as_ref()
        .unwrap()
        .mode_overrides
        .contains_key(&FormatKind::Portrait));
    let landscape_only = Op::SetSettings {
        settings: Settings {
            formats: vec![FormatKind::Landscape],
            ..p.settings.clone()
        },
    };
    let r = chat(vec![landscape_only.clone()]).review(&p).unwrap();
    assert!(
        r.removals.is_empty() && r.risk == OpRisk::Additive,
        "{:?}",
        r.removals
    );
    let mut placed = p.clone();
    placed.timeline.tracks[0].items[0].layout.overrides.insert(
        FormatKind::Portrait,
        BoxN {
            x: 0.1,
            y: 0.7,
            w: 0.8,
            h: 0.1,
        },
    );
    let r = chat(vec![landscape_only.clone()]).review(&placed).unwrap();
    assert!(
        r.removals.is_empty() && r.risk == OpRisk::Additive,
        "{:?}",
        r.removals
    );
    placed.captions.place.insert(FormatKind::Portrait, 0.8);
    let r = chat(vec![landscape_only]).review(&placed).unwrap();
    assert_eq!(
        (r.risk, r.removals),
        (
            OpRisk::Removal,
            Removals {
                formats: vec![FormatKind::Portrait],
                ..Default::default()
            }
        )
    );
}

/// A clip the batch inserts is new even under the id of a clip it removed (an origin that sends raw ops, such as a
/// plugin): the clip that was there is gone, with its footage and its camera.
#[test]
fn a_new_clip_under_a_removed_clips_id_is_still_a_removal() {
    let p = fixture();
    let mut fresh = Clip::new(ClipSource::Recording { id: "rec".into() }, 0, secs(20.0));
    fresh.id = "a".into();
    let r = chat(vec![
        Op::RemoveClip { id: "a".into() },
        Op::InsertClip {
            clip: fresh,
            index: Some(0),
        },
    ])
    .review(&p)
    .unwrap();
    assert_eq!(r.risk, OpRisk::Removal);
    assert_eq!(
        (r.removals.clips, r.removals.ranges),
        (vec!["a".to_string()], vec![range(0.0, 20.0)])
    );
}

/// An effect is kept by its content: the same contribution and settings, under any id, or the same effect adjusted in
/// place (its id and contribution, as `UpdateEffect` leaves it). A stack that reuses an id for another effect (ids are
/// minted per stack, so every first effect is "fx") has lost the effect that was there, whoever sent the ops.
#[test]
fn an_effect_replaced_under_the_same_id_is_a_removal() {
    let mut p = fixture();
    let fx = |effect: &str, key: &str, value: f64| EffectInstance {
        id: "fx".into(),
        ..EffectInstance::new(effect).with(key, ParamValue::Number(value))
    };
    p.timeline.main[0].effects = vec![fx(builtin::EFFECT_PIXELATE, "size", 24.0)];
    let p = after(&p, vec![]);
    let stack = |effects: Vec<EffectInstance>| {
        chat(vec![Op::UpdateClip {
            id: "a".into(),
            patch: ClipPatch {
                effects: Some(effects),
                ..Default::default()
            },
        }])
    };
    // The privacy pixelate swapped for a vignette under the same id: an MCP or plugin batch is judged like chat's.
    let swapped = stack(vec![fx(builtin::EFFECT_VIGNETTE, "amount", 0.4)]);
    assert!(swapped.needs_review(Some(&p)));
    assert_eq!(
        swapped.review(&p).unwrap().removals.effects,
        vec!["a".to_string()]
    );
    // Another pixelate under a new id is a replaced effect too.
    let mut other = fx(builtin::EFFECT_PIXELATE, "size", 8.0);
    other.id = "t-fx1".into();
    assert!(stack(vec![other]).needs_review(Some(&p)));
    // The same pixelate under a new id, or adjusted in place, beside a new vignette: nothing is lost.
    let mut copy = fx(builtin::EFFECT_PIXELATE, "size", 24.0);
    copy.id = "t-fx1".into();
    let mut vignette = fx(builtin::EFFECT_VIGNETTE, "amount", 0.4);
    vignette.id = "t-fx2".into();
    assert!(!stack(vec![copy, vignette.clone()]).needs_review(Some(&p)));
    let adjusted = fx(builtin::EFFECT_PIXELATE, "size", 40.0);
    assert!(!stack(vec![adjusted, vignette]).needs_review(Some(&p)));
}

/// Each effect keeps one effect only: a doubled pixelate that loses its copy loses an
/// effect, while one kept in place and its copy re-made under a new id lose nothing.
#[test]
fn a_doubled_effect_that_loses_a_copy_is_a_removal() {
    let mut p = fixture();
    let pixelate = |id: &str| EffectInstance {
        id: id.into(),
        ..EffectInstance::new(builtin::EFFECT_PIXELATE).with("size", ParamValue::Number(30.0))
    };
    p.timeline.main[0].effects = vec![pixelate("fx"), pixelate("fx-2")];
    let p = after(&p, vec![]);
    let stack = |effects: Vec<EffectInstance>| {
        chat(vec![Op::UpdateClip {
            id: "a".into(),
            patch: ClipPatch {
                effects: Some(effects),
                ..Default::default()
            },
        }])
    };
    let one = stack(vec![pixelate("fx")]);
    assert!(one.needs_review(Some(&p)));
    assert_eq!(
        one.review(&p).unwrap().removals.effects,
        vec!["a".to_string()]
    );
    assert!(!stack(vec![pixelate("fx"), pixelate("t-fx1")]).needs_review(Some(&p)));
}
