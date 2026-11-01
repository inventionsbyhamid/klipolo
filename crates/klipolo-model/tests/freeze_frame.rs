//! Freeze frame: a still of the frame at the playhead put into the main track there, holding for its length.
mod common;
use common::*;
use klipolo_model::contrib::builtin;
use klipolo_model::history::Document;
use klipolo_model::ops::{op_risk, Edge, Op, OpRisk};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::query::{anchor_at, main_track_duration};
use klipolo_model::risk::removals;
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{
    Attach, Clip, ClipSource, EffectInstance, Framing, FramingMode, Look,
};
use klipolo_model::{apply_ops, Transaction};

const FRAME: Us = 33_334;

fn freeze(at: f64, seconds: f64) -> Op {
    Op::FreezeFrame {
        at_us: secs(at),
        duration_us: secs(seconds),
    }
}

/// Each main clip as (id, its frame held or None, timeline length).
fn main_of(p: &Project) -> Vec<(String, Option<Us>, Us)> {
    p.timeline
        .main
        .iter()
        .map(|c| (c.id.clone(), c.freeze_at_us, c.duration_us()))
        .collect()
}

fn held(p: &Project) -> &Clip {
    p.timeline
        .main
        .iter()
        .find(|c| c.freeze_at_us.is_some())
        .expect("a freeze frame")
}

#[test]
fn freezing_inside_a_clip_splits_it_and_holds_the_frame_between_its_pieces() {
    let mut p = fixture();
    // "b" plays the b-roll at 20 s to 30 s, with a look, a framing and an effect of its own.
    let b = &mut p.timeline.main[1];
    b.look = Look {
        preset: Some(builtin::look("warm")),
        ..Look::default()
    };
    b.framing.insert(
        FormatKind::Portrait,
        Framing {
            mode: FramingMode::Fill,
            focus_x: 0.3,
            focus_y: 0.5,
            follow: None,
        },
    );
    b.effects.push(EffectInstance {
        id: "fx".into(),
        ..EffectInstance::new(builtin::EFFECT_BLUR)
    });
    (b.fade_in_us, b.fade_out_us) = (secs(1.0), secs(1.0));
    let before = p.timeline.main[1].clone();
    let p = apply_ops(&p, &[freeze(25.0, 2.0)]).unwrap();
    assert_eq!(
        main_of(&p),
        [
            ("a".into(), None, secs(20.0)),
            ("b".into(), None, secs(5.0)),
            ("b-freeze".into(), Some(secs(5.0)), secs(2.0)),
            ("b-2".into(), None, secs(5.0)),
        ]
    );
    assert_eq!(main_track_duration(&p.timeline.main), secs(32.0));
    let f = held(&p);
    assert_eq!(
        (&f.look, &f.framing, &f.effects, &f.source),
        (
            &before.look,
            &before.framing,
            &before.effects,
            &before.source
        )
    );
    // Silent, with no fades or transition of its own, at normal speed.
    assert!(f.muted);
    assert_eq!((f.fade_in_us, f.fade_out_us, f.speed), (0, 0, 1.0));
    assert!(f.transition_out.is_none());
    assert_eq!(op_risk(&freeze(25.0, 2.0)), OpRisk::Additive);
    assert_eq!(freeze(25.0, 2.0).tag(), "freezeFrame");
}

#[test]
fn at_a_clips_edge_the_freeze_holds_that_clips_first_or_last_frame() {
    // At the start of "b": its first frame, before it.
    let p = apply_ops(&fixture(), &[freeze(20.0, 2.0)]).unwrap();
    assert_eq!(
        main_of(&p),
        [
            ("a".into(), None, secs(20.0)),
            ("b-freeze".into(), Some(0), secs(2.0)),
            ("b".into(), None, secs(10.0)),
        ]
    );
    // 5 ms before the end of "a": too close to split, so its last frame, after it.
    let p = apply_ops(&fixture(), &[freeze(19.995, 1.0)]).unwrap();
    assert_eq!(p.timeline.main[1].freeze_at_us, Some(secs(19.995)));
    assert_eq!(p.timeline.main[1].id, "a-freeze");
    assert_eq!(p.timeline.main.len(), 3);
    // At the main track's end: the last clip's last frame, after it.
    let p = apply_ops(&fixture(), &[freeze(30.0, 3.0)]).unwrap();
    assert_eq!(
        main_of(&p).last().unwrap(),
        &("b-freeze".into(), Some(secs(10.0) - 1), secs(3.0))
    );
    assert_eq!(main_track_duration(&p.timeline.main), secs(33.0));
}

#[test]
fn a_freeze_needs_a_clip_and_a_length() {
    let mut empty = fixture();
    empty.timeline.main.clear();
    assert_eq!(
        apply_ops(&empty, &[freeze(0.0, 2.0)]).unwrap_err(),
        refused("There's no clip at that time to freeze.")
    );
    assert_eq!(
        apply_ops(&fixture(), &[freeze(31.0, 2.0)]).unwrap_err(),
        refused("There's no clip at that time to freeze.")
    );
    assert_eq!(
        apply_ops(&fixture(), &[freeze(5.0, 0.005)]).unwrap_err(),
        refused("A freeze frame must be between 10 ms and 10 minutes long.")
    );
    assert!(apply_ops(&fixture(), &[freeze(5.0, 601.0)]).is_err());
}

#[test]
fn layers_attached_after_the_freeze_move_later_with_their_footage() {
    let mut p = fixture();
    let mut title = text_item("t", 26.0, 2.0);
    title.attach = anchor_at(&p.timeline.main, title.start_us);
    let mut early = text_item("e", 21.0, 1.0);
    early.attach = anchor_at(&p.timeline.main, early.start_us);
    p.timeline
        .tracks
        .push(overlay_track("o", vec![early, title]));
    let p = apply_ops(&p, &[freeze(25.0, 2.0)]).unwrap();
    assert_eq!(item_by_id(&p, "t").start_us, secs(28.0));
    assert_eq!(item_by_id(&p, "e").start_us, secs(21.0));
}

#[test]
fn a_freeze_frame_trims_longer_than_its_file_and_splits_into_two_freezes() {
    // A freeze of the b-roll's last frame, 2 s long, after "b".
    let p = apply_ops(&fixture(), &[freeze(30.0, 2.0)]).unwrap();
    let f = held(&p).clone();
    // Its length isn't footage: it may run past the 10 s file.
    let p = apply_ops(
        &p,
        &[Op::TrimClip {
            id: f.id.clone(),
            edge: Edge::End,
            source_us: f.source_in_us + secs(20.0),
        }],
    )
    .unwrap();
    assert_eq!(held(&p).duration_us(), secs(20.0));
    // Split, both pieces hold the same frame.
    let p = apply_ops(&p, &[Op::Split { at_us: secs(40.0) }]).unwrap();
    let frames: Vec<_> = p.timeline.main[2..]
        .iter()
        .map(|c| (c.freeze_at_us, c.duration_us()))
        .collect();
    assert_eq!(
        frames,
        [
            (Some(secs(10.0) - 1), secs(10.0)),
            (Some(secs(10.0) - 1), secs(10.0))
        ]
    );
}

#[test]
fn a_freeze_frame_has_no_sound_to_separate() {
    let p = apply_ops(&fixture(), &[freeze(25.0, 2.0)]).unwrap();
    let id = held(&p).id.clone();
    assert_eq!(
        apply_ops(&p, &[Op::DetachSound { clip_id: id }]).unwrap_err(),
        refused("A freeze frame has nothing to separate.")
    );
    let p = apply_ops(&fixture(), &[freeze(5.0, 2.0)]).unwrap();
    let id = held(&p).id.clone();
    for source in ["mic", "camera"] {
        assert_eq!(
            apply_ops(
                &p,
                &[Op::DetachSource {
                    clip_id: id.clone(),
                    source_id: source.into()
                }]
            )
            .unwrap_err(),
            refused("A freeze frame has nothing to separate.")
        );
    }
}

#[test]
fn a_frozen_recording_holds_the_take_s_frame_with_its_camera() {
    let p = apply_ops(&fixture(), &[freeze(5.0, 2.0)]).unwrap();
    let f = held(&p);
    assert_eq!(f.freeze_at_us, Some(secs(5.0)));
    assert_eq!(f.camera, p.timeline.main[0].camera);
    // Frame times stay as the clip shows them, not rounded to a frame.
    let p = apply_ops(&fixture(), &[freeze(5.0 + 0.5 * FRAME as f64 / 1e6, 2.0)]).unwrap();
    assert_eq!(held(&p).freeze_at_us, Some(secs(5.0) + FRAME / 2));
}

#[test]
fn the_project_needs_freeze_frames_while_it_has_one() {
    let p = apply_ops(&fixture(), &[freeze(25.0, 2.0)]).unwrap();
    assert!(p.requires.contains("freeze@1"));
    let id = held(&p).id.clone();
    let p = apply_ops(&p, &[Op::RemoveClip { id }]).unwrap();
    assert!(!p.requires.contains("freeze@1"));
    // A clip that isn't a freeze writes no freeze field.
    let json = serde_json::to_value(&fixture().timeline.main[1]).unwrap();
    assert!(json.get("freezeAtUs").is_none(), "{json}");
}

/// Freeze, then undo: the clip is whole again, exactly as before.
#[test]
fn freezing_then_undo_puts_the_clip_back() {
    let mut doc = Document::new(fixture()).unwrap();
    let before = doc.project().clone();
    doc.apply(&Transaction {
        label: "undo.freeze".into(),
        ops: vec![freeze(25.0, 2.0)],
        origin: None,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(doc.project().timeline.main.len(), 4);
    doc.undo();
    assert_eq!(*doc.project(), before);
}

#[test]
fn making_a_freeze_frame_removes_nothing_and_removing_one_is_a_removal() {
    let before = fixture();
    let p = apply_ops(&before, &[freeze(25.0, 2.0)]).unwrap();
    assert!(removals(&before, &p).ranges.is_empty());
    let gone = apply_ops(
        &p,
        &[Op::RemoveClip {
            id: "b-freeze".into(),
        }],
    )
    .unwrap();
    assert_eq!(
        removals(&p, &gone).ranges,
        [RangeUs {
            start: secs(25.0),
            end: secs(27.0)
        }]
    );
}

/// A layer anchored to footage of the b-roll never lands on a freeze of it, which shows none: with its own clip gone,
/// it goes where that footage still plays.
#[test]
fn an_anchor_never_falls_back_onto_a_freeze_frame() {
    // A freeze of the b-roll's first frame holds for 10 s before "b", at 20 s.
    let mut p = apply_ops(&fixture(), &[freeze(20.0, 10.0)]).unwrap();
    // A title anchored 6 s into the b-roll on a clip that is gone: in "b", at 30 s + 6 s, never on the freeze, whose
    // length only looks like the same stretch of the file.
    let mut title = text_item("t", 0.0, 1.0);
    title.attach = Some(Attach {
        clip_id: "gone".into(),
        source: ClipSource::Asset { id: "broll".into() },
        source_us: secs(6.0),
    });
    p.timeline.tracks.push(overlay_track("o", vec![title]));
    let p = apply_ops(&p, &[Op::Rename { name: "x".into() }]).unwrap();
    assert_eq!(item_by_id(&p, "t").start_us, secs(36.0));
}
