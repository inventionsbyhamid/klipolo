//! Reverse: an imported clip plays backward from a copy of its file's stretch written backward, and forward again from
//! its own file.
mod common;
use common::*;
use klipolo_model::history::Document;
use klipolo_model::ops::{op_risk, Op, OpRisk};
use klipolo_model::project::{Asset, AssetKind, Project};
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
