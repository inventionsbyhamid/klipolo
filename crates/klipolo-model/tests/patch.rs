mod common;
use common::*;
use klipolo_model::cursor_style::{CursorStyle, Smoothing};
use klipolo_model::history::{Document, HistoryLimits, PRESSURE_STEPS};
use klipolo_model::ops::{CursorTarget, Op, TrackTarget};
use klipolo_model::patch::{apply, compose, diff};
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::TrackKind;
use klipolo_model::{apply_ops, Project, Transaction};

/// Two overlay tracks: "t1" with "title" (1–4 s) and "t2" with "pic" (6–8 s).
fn layered() -> Project {
    let mut p = fixture();
    p.timeline.tracks = vec![
        overlay_track("t1", vec![text_item("title", 1.0, 3.0)]),
        overlay_track("t2", vec![text_item("pic", 6.0, 2.0)]),
    ];
    p
}

fn round_trip(a: &Project, ops: &[Op]) {
    let b = apply_ops(a, ops).unwrap();
    assert_eq!(apply(a, &diff(a, &b)).unwrap(), b, "forward: {ops:?}");
    assert_eq!(apply(&b, &diff(&b, a)).unwrap(), *a, "back: {ops:?}");
}

#[test]
fn every_kind_of_edit_round_trips() {
    let p = layered();
    let edits: Vec<Vec<Op>> = vec![
        vec![Op::Rename {
            name: "Demo".into(),
        }],
        vec![Op::MoveClip {
            id: "b".into(),
            index: 0,
        }],
        vec![Op::Split { at_us: secs(5.0) }],
        vec![Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: secs(2.0),
                end: secs(4.0),
            }],
        }],
        vec![Op::MoveItem {
            id: "title".into(),
            track_id: Some("t2".into()),
            start_us: secs(9.0),
        }],
        vec![Op::RemoveTrack { id: "t1".into() }],
        vec![Op::AddTrack {
            id: Some("t3".into()),
            kind: TrackKind::Audio,
            index: None,
        }],
        vec![Op::AddItem {
            track: TrackTarget::Auto,
            item: text_item("new", 12.0, 1.0),
            attach: true,
        }],
        // The project's cursor style (a top-level part the UI's copy must hear of), and one clip's own.
        vec![Op::SetCursorStyle {
            target: CursorTarget::Project,
            style: Some(CursorStyle {
                size: 2.0,
                ..CursorStyle::default()
            }),
        }],
        vec![Op::SetCursorStyle {
            target: CursorTarget::Clips {
                ids: vec!["a".into()],
            },
            style: Some(CursorStyle {
                smoothing: Smoothing::Strong,
                ..CursorStyle::default()
            }),
        }],
    ];
    for ops in &edits {
        round_trip(&p, ops);
    }
    // Two patches in a row compose into one that does both, forward and back, for every
    // pair of these edits (what folding a history step relies on).
    for first in &edits {
        let b = apply_ops(&p, first).unwrap();
        for then in &edits {
            let Ok(c) = apply_ops(&b, then) else {
                continue;
            };
            let both = compose(&diff(&p, &b), &diff(&b, &c));
            assert_eq!(
                apply(&p, &both).unwrap(),
                c,
                "forward: {first:?} then {then:?}"
            );
            let back = compose(&diff(&c, &b), &diff(&b, &p));
            assert_eq!(
                apply(&c, &back).unwrap(),
                p,
                "back: {then:?} then {first:?}"
            );
        }
    }
}

#[test]
fn moving_one_layer_sends_only_that_layer() {
    let a = layered();
    let b = apply_ops(
        &a,
        &[Op::MoveItem {
            id: "title".into(),
            track_id: None,
            start_us: secs(1.5),
        }],
    )
    .unwrap();
    let patch = diff(&a, &b);
    assert_eq!(
        patch
            .items
            .iter()
            .map(|i| i.id.as_str())
            .collect::<Vec<_>>(),
        ["title"]
    );
    assert!(
        patch.clips.is_empty()
            && patch.main.is_none()
            && patch.tracks.is_none()
            && patch.name.is_none()
    );
    assert!(diff(&a, &a).is_empty());
}

fn rename(n: usize) -> Transaction {
    Transaction {
        label: "Rename".into(),
        ops: vec![Op::Rename {
            name: format!("Name {n}"),
        }],
        origin: None,
        ..Default::default()
    }
}

#[test]
fn undo_and_redo_report_exactly_what_they_changed() {
    let mut doc = Document::new(layered()).unwrap();
    let before = doc.project().clone();
    let change = doc
        .apply_change(&Transaction {
            label: "Move layer".into(),
            ops: vec![Op::MoveItem {
                id: "pic".into(),
                track_id: None,
                start_us: secs(10.0),
            }],
            origin: None,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(change.base_revision, 0);
    let after = doc.project().clone();
    assert_eq!(apply(&before, &change.patch).unwrap(), after);
    let undone = doc.undo_change().unwrap();
    assert_eq!((undone.base_revision, doc.revision()), (1, 2));
    assert_eq!(apply(&after, &undone.patch).unwrap(), before);
    assert_eq!(*doc.project(), before);
    let redone = doc.redo_change().unwrap();
    assert_eq!(apply(&before, &redone.patch).unwrap(), after);
    assert_eq!(*doc.project(), after);
}

#[test]
fn the_byte_budget_drops_the_oldest_steps_first() {
    let mut doc = Document::new(layered()).unwrap();
    doc.set_limits(HistoryLimits {
        steps: 200,
        bytes: 2_000,
    });
    for n in 0..100 {
        doc.apply(&rename(n)).unwrap();
    }
    assert!(doc.history_bytes() <= 2_000, "{}", doc.history_bytes());
    let kept = doc.undo_steps();
    assert!(kept > 1 && kept < 100, "{kept}");
    while doc.undo().is_some() {}
    assert_eq!(doc.project().name, format!("Name {}", 99 - kept));
}

#[test]
fn memory_pressure_keeps_fifty_steps() {
    let mut doc = Document::new(layered()).unwrap();
    for n in 0..120 {
        doc.apply(&rename(n)).unwrap();
    }
    doc.set_limits(HistoryLimits::under_pressure());
    assert_eq!(doc.undo_steps(), PRESSURE_STEPS);
    doc.set_limits(HistoryLimits::default());
    assert_eq!(
        doc.undo_steps(),
        PRESSURE_STEPS,
        "steps already dropped don't come back"
    );
}

#[test]
fn redo_steps_count_in_the_budget_and_the_pressure_trim() {
    let mut doc = Document::new(layered()).unwrap();
    for n in 0..120 {
        doc.apply(&rename(n)).unwrap();
    }
    let bytes = doc.history_bytes();
    for _ in 0..100 {
        doc.undo().unwrap();
    }
    assert!(
        doc.history_bytes() > bytes / 2,
        "redo steps are counted: {} of {bytes}",
        doc.history_bytes()
    );
    doc.set_limits(HistoryLimits::under_pressure());
    let (undo, redo) = (doc.undo_steps(), doc.redo_steps());
    assert_eq!(undo + redo, PRESSURE_STEPS, "{undo} + {redo}");
    assert!(
        undo >= 1 && redo >= 1,
        "both directions keep their next step"
    );
    // What is left still chains: every redo and then every undo applies.
    let mut redone = 0;
    while doc.redo().is_some() {
        redone += 1;
    }
    let mut undone = 0;
    while doc.undo().is_some() {
        undone += 1;
    }
    assert_eq!((redone, undone), (redo, undo + redo));
}
