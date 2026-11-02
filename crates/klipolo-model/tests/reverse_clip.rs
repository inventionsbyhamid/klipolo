//! Reverse: an imported clip plays backward from a copy of its file's stretch written backward, and forward again from
//! its own file.
mod common;
use common::*;
use klipolo_model::fill_focus::{FillFocus, FocusKey, FocusPath, Keep, KeepRange, Who};
use klipolo_model::history::Document;
use klipolo_model::motion::Key;
use klipolo_model::ops::{op_risk, Op, OpRisk};
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::query::anchor_at;
use klipolo_model::timeline::{ClipSource, ReverseCopy, Reversed};
use klipolo_model::{apply_ops, Transaction};

/// The b-roll's 0 to 10 s written backward: a 10 s file.
fn copy_asset() -> Asset {
    let mut a = fixture().asset("broll").unwrap().clone();
    a.id = "broll-rev".into();
    a.path = "/media/broll reversed.mov".into();
    a.name = "broll reversed.mov".into();
    a.kind = AssetKind::Video;
    a
}

fn with_copy() -> Project {
    apply_ops(
        &fixture(),
        &[Op::AddAssets {
            assets: vec![copy_asset()],
        }],
    )
    .unwrap()
}

fn reverse(clip: &str, copy: Option<(f64, f64)>) -> Op {
    Op::ReverseClip {
        clip_id: clip.into(),
        copy: copy.map(|(from, to)| ReverseCopy {
            asset_id: "broll-rev".into(),
            from_us: secs(from),
            to_us: secs(to),
        }),
    }
}

#[test]
fn a_reversed_clip_plays_its_stretch_from_the_copy_turned_around() {
    let mut p = with_copy();
    // "b" plays the b-roll's 2 to 5 s.
    (
        p.timeline.main[1].source_in_us,
        p.timeline.main[1].source_out_us,
    ) = (secs(2.0), secs(5.0));
    let p = apply_ops(&p, &[reverse("b", Some((0.0, 10.0)))]).unwrap();
    let b = &p.timeline.main[1];
    assert_eq!(
        b.source,
        ClipSource::Asset {
            id: "broll-rev".into()
        }
    );
    // The copy's 5 to 8 s is the b-roll's 5 to 2 s.
    assert_eq!((b.source_in_us, b.source_out_us), (secs(5.0), secs(8.0)));
    assert_eq!(
        b.reversed,
        Some(Reversed {
            of: "broll".into(),
            from_us: 0,
            to_us: secs(10.0)
        })
    );
    assert_eq!(op_risk(&reverse("b", None)), OpRisk::Additive);
    assert_eq!(reverse("b", None).tag(), "reverseClip");
}

#[test]
fn reversing_again_plays_the_clip_forward_from_its_own_file() {
    let mut p = with_copy();
    (
        p.timeline.main[1].source_in_us,
        p.timeline.main[1].source_out_us,
    ) = (secs(2.0), secs(5.0));
    let before = p.timeline.main[1].clone();
    let p = apply_ops(&p, &[reverse("b", Some((0.0, 10.0)))]).unwrap();
    let p = apply_ops(&p, &[reverse("b", None)]).unwrap();
    assert_eq!(p.timeline.main[1], before);
}

#[test]
fn a_copy_of_part_of_the_file_reverses_inside_its_stretch() {
    let mut p = with_copy();
    (
        p.timeline.main[1].source_in_us,
        p.timeline.main[1].source_out_us,
    ) = (secs(3.0), secs(5.0));
    // The copy holds the b-roll's 1 to 7 s (6 s): the b-roll's 5 s is the copy's 2 s.
    let p = apply_ops(&p, &[reverse("b", Some((1.0, 7.0)))]).unwrap();
    let b = &p.timeline.main[1];
    assert_eq!((b.source_in_us, b.source_out_us), (secs(2.0), secs(4.0)));
    // A copy that doesn't hold the whole clip is refused.
    let mut q = with_copy();
    (
        q.timeline.main[1].source_in_us,
        q.timeline.main[1].source_out_us,
    ) = (secs(3.0), secs(9.0));
    assert_eq!(
        apply_ops(&q, &[reverse("b", Some((1.0, 7.0)))]).unwrap_err(),
        refused("That reversed copy doesn't hold this clip's footage.")
    );
}

#[test]
fn layers_attached_to_a_reversed_clip_keep_their_place() {
    let mut p = with_copy();
    let mut title = text_item("t", 23.0, 1.0);
    title.attach = anchor_at(&p.timeline.main, title.start_us);
    p.timeline.tracks.push(overlay_track("o", vec![title]));
    let p = apply_ops(&p, &[reverse("b", Some((0.0, 10.0)))]).unwrap();
    // Removing "a" moves "b" to 0 s: the title, 3 s into it, goes with it.
    let p = apply_ops(&p, &[Op::RemoveClip { id: "a".into() }]).unwrap();
    assert_eq!(item_by_id(&p, "t").start_us, secs(3.0));
}

#[test]
fn only_an_imported_clip_plays_backward() {
    let p = with_copy();
    assert_eq!(
        apply_ops(&p, &[reverse("a", Some((0.0, 10.0)))]).unwrap_err(),
        refused("Only imported video clips can play backward.")
    );
    assert_eq!(
        apply_ops(&p, &[reverse("b", None)]).unwrap_err(),
        refused("This clip needs a reversed copy of its footage first.")
    );
    let frozen = apply_ops(
        &p,
        &[Op::FreezeFrame {
            at_us: secs(25.0),
            duration_us: secs(2.0),
        }],
    )
    .unwrap();
    assert_eq!(
        apply_ops(&frozen, &[reverse("b-freeze", Some((0.0, 10.0)))]).unwrap_err(),
        refused("A freeze frame can't play backward.")
    );
    let missing = apply_ops(&fixture(), &[reverse("b", Some((0.0, 10.0)))]);
    assert!(missing.is_err());
}

/// A reversed clip trimmed out to its copy's end, the copy measured a hair longer than the stretch it holds: played
/// forward, it starts at its file's start, never before it.
#[test]
fn playing_forward_never_starts_before_the_file() {
    let mut copy = copy_asset();
    copy.duration_us = secs(10.02);
    let p = apply_ops(&fixture(), &[Op::AddAssets { assets: vec![copy] }]).unwrap();
    let mut p = apply_ops(&p, &[reverse("b", Some((0.0, 10.0)))]).unwrap();
    p.timeline.main[1].source_out_us = secs(10.02);
    let p = apply_ops(&p, &[reverse("b", None)]).unwrap();
    let b = &p.timeline.main[1];
    assert_eq!((b.source_in_us, b.source_out_us), (0, secs(10.0)));
}

/// A reversed clip trimmed out into its copy's extra length, with a volume key and a fade out at its end: played
/// forward it is shorter, and its fade and keys fit what is left instead of the turn being refused.
#[test]
fn playing_forward_fits_fades_and_volume_keys_to_the_shorter_clip() {
    let mut copy = copy_asset();
    copy.duration_us = secs(10.5);
    let p = apply_ops(&fixture(), &[Op::AddAssets { assets: vec![copy] }]).unwrap();
    let mut p = apply_ops(&p, &[reverse("b", Some((0.0, 10.0)))]).unwrap();
    let b = &mut p.timeline.main[1];
    b.source_out_us = secs(10.5);
    b.fade_out_us = secs(10.5);
    b.volume_keys = vec![
        Key {
            t_us: secs(2.0),
            v: 1.0,
            ease: None,
        },
        Key {
            t_us: secs(10.5),
            v: 0.0,
            ease: None,
        },
    ];
    // As it is, it is a clip the model takes.
    let p = apply_ops(&p, &[]).unwrap();
    let p = apply_ops(&p, &[reverse("b", None)]).unwrap();
    let b = &p.timeline.main[1];
    assert_eq!((b.source_in_us, b.source_out_us), (0, secs(10.0)));
    assert_eq!((b.fade_in_us, b.fade_out_us), (0, secs(10.0)));
    assert_eq!(b.volume_keys.first().map(|k| k.t_us), Some(secs(2.0)));
    assert!(b.volume_keys.iter().all(|k| k.t_us <= secs(10.0)));
}

fn focus_key(at_s: f64, x: f64, cut: bool) -> FocusKey {
    FocusKey {
        at_us: secs(at_s),
        x,
        y: 0.5,
        cut,
    }
}

fn keep_range(from_s: f64, to_s: f64, who: Who) -> KeepRange {
    KeepRange {
        start_us: secs(from_s),
        end_us: secs(to_s),
        who,
    }
}

/// The b-roll's speaker moving right over its first 5 s, then a new shot at 8 s with them on the left; the first
/// person kept from 1 to 3 s and both from 6 to 9 s.
fn followed() -> FillFocus {
    FillFocus::Speaker {
        path: FocusPath {
            keys: vec![
                focus_key(0.0, 0.3, false),
                focus_key(5.0, 0.7, false),
                focus_key(8.0, 0.2, true),
            ],
        },
        keep: Keep::Ranges {
            ranges: vec![
                keep_range(1.0, 3.0, Who::Person { person: 1 }),
                keep_range(6.0, 9.0, Who::Both),
            ],
        },
        people: Vec::new(),
    }
}

/// "b" playing the b-roll's `from` to `to` s, following its speaker in portrait.
fn with_follow(from: f64, to: f64) -> Project {
    let mut p = with_copy();
    let b = &mut p.timeline.main[1];
    (b.source_in_us, b.source_out_us) = (secs(from), secs(to));
    b.fill_focus.insert(FormatKind::Portrait, followed());
    apply_ops(&p, &[]).unwrap()
}

/// Whether the crop's centres `a` and `b` are the same, to rounding.
fn same(a: Option<(f64, f64)>, b: Option<(f64, f64)>) -> bool {
    a.zip(b)
        .is_some_and(|(a, b)| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9)
}

fn focus_of(p: &Project) -> (&FocusPath, &Keep) {
    match p.timeline.main[1].fill_focus.get(&FormatKind::Portrait) {
        Some(FillFocus::Speaker { path, keep, .. }) => (path, keep),
        other => panic!("not following the speaker: {other:?}"),
    }
}

/// Reversed, the clip plays the copy's `t`, the b-roll's `10 - t`: the crop and who it keeps are where they were at
/// that moment of the b-roll, cut included. Forward again, they are as they were.
#[test]
fn a_reversed_clip_follows_its_speaker_on_the_same_footage() {
    let before = with_follow(0.0, 10.0);
    let (path, keep) = focus_of(&before);
    let back = apply_ops(&before, &[reverse("b", Some((0.0, 10.0)))]).unwrap();
    let (turned, turned_keep) = focus_of(&back);
    for t in [
        0.0, 0.5, 1.5, 1.99, 2.01, 2.5, 3.5, 5.0, 6.5, 7.5, 8.5, 9.5, 10.0,
    ] {
        let (now, then) = (secs(t), secs(10.0 - t));
        let (a, b) = (turned.centre_at(now), path.centre_at(then));
        assert!(same(a, b), "at {t} s: {a:?}, was {b:?}");
        assert_eq!(turned_keep.who_at(now), keep.who_at(then), "who at {t} s");
    }
    // The new shot comes first now: it holds up to where it ended, then the crop jumps back to the earlier shot.
    assert_eq!(turned.centre_at(secs(2.0)), Some((0.2, 0.5)));
    assert_eq!(turned.centre_at(secs(2.0) + 1), Some((0.7, 0.5)));
    assert_eq!(
        *turned_keep,
        Keep::Ranges {
            ranges: vec![
                keep_range(1.0, 4.0, Who::Both),
                keep_range(7.0, 9.0, Who::Person { person: 1 }),
            ]
        }
    );
    let forward = apply_ops(&back, &[reverse("b", None)]).unwrap();
    let (again, again_keep) = focus_of(&forward);
    assert_eq!(again_keep, keep);
    for t in [0.0, 1.0, 2.5, 4.99, 5.0, 6.0, 7.99, 8.0, 8.5, 10.0] {
        assert_eq!(
            again.centre_at(secs(t)),
            path.centre_at(secs(t)),
            "at {t} s"
        );
    }
}

/// A copy of only the b-roll's 1 to 7 s: the path and the ranges past 7 s (before the copy's start once turned) give
/// way to where the crop was at 7 s and who was kept then, and the turned clip is one the model takes.
#[test]
fn a_reversed_part_of_the_file_keeps_only_the_follow_it_can_turn() {
    let before = with_follow(3.0, 5.0);
    let (path, keep) = focus_of(&before);
    let back = apply_ops(&before, &[reverse("b", Some((1.0, 7.0)))]).unwrap();
    let (turned, turned_keep) = focus_of(&back);
    assert!(turned.is_valid() && turned_keep.is_valid());
    assert!(turned.keys.iter().all(|k| k.at_us >= 0));
    for t in [0.0, 0.5, 2.0, 3.0, 4.5, 6.5] {
        let (now, then) = (secs(t), secs(7.0 - t));
        assert!(
            same(turned.centre_at(now), path.centre_at(then)),
            "at {t} s"
        );
        assert_eq!(turned_keep.who_at(now), keep.who_at(then), "who at {t} s");
    }
}

/// A clip whose sound plays in its own layer (Separate sound) is not reversed: the layer would keep playing forward.
#[test]
fn a_clip_whose_sound_is_separate_is_not_reversed() {
    let p = apply_ops(
        &with_copy(),
        &[Op::DetachSound {
            clip_id: "b".into(),
        }],
    )
    .unwrap();
    assert_eq!(
        apply_ops(&p, &[reverse("b", Some((0.0, 10.0)))]).unwrap_err(),
        refused("This clip's sound plays in its own layer. Remove that layer before reversing the clip.")
    );
    // Its sound separated after it was reversed: playing it forward is refused too.
    let p = apply_ops(&with_copy(), &[reverse("b", Some((0.0, 10.0)))]).unwrap();
    let p = apply_ops(
        &p,
        &[Op::DetachSound {
            clip_id: "b".into(),
        }],
    )
    .unwrap();
    assert!(apply_ops(&p, &[reverse("b", None)]).is_err());
}

/// The file a reversed clip plays forward from can't be removed while the clip is there.
#[test]
fn a_reversed_clip_keeps_its_own_file() {
    let p = apply_ops(&with_copy(), &[reverse("b", Some((0.0, 10.0)))]).unwrap();
    assert_eq!(
        apply_ops(&p, &[Op::RemoveAsset { id: "broll".into() }]).unwrap_err(),
        refused("A reversed clip still needs this file to play forward again. Play it forward or remove the clip first.")
    );
}

/// Reverse, then undo: the clip plays forward from its own file, exactly as before.
#[test]
fn reversing_then_undo_puts_the_clip_back() {
    let mut doc = Document::new(fixture()).unwrap();
    let before = doc.project().clone();
    doc.apply(&Transaction {
        label: "undo.reverse".into(),
        ops: vec![
            Op::AddAssets {
                assets: vec![copy_asset()],
            },
            reverse("b", Some((0.0, 10.0))),
        ],
        origin: None,
        ..Default::default()
    })
    .unwrap();
    assert!(doc.project().timeline.main[1].reversed.is_some());
    doc.undo();
    assert_eq!(*doc.project(), before);
}
