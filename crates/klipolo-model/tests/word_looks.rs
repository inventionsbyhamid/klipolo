use std::sync::Arc;

use klipolo_model::captions::looks::{Look, LookIndex, WordLook};
use klipolo_model::ops::Op;
use klipolo_model::time::{secs_to_us as s, RangeUs};

fn red() -> Look {
    Look {
        color: Some("#FF3B30".into()),
        ..Look::default()
    }
}

fn asset(id: &str) -> klipolo_model::project::Asset {
    klipolo_model::project::Asset {
        id: id.into(),
        kind: klipolo_model::project::AssetKind::Video,
        path: format!("/m/{id}.mp4"),
        name: id.into(),
        duration_us: s(60.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
    }
}

fn with_asset() -> klipolo_model::project::Project {
    let mut p = klipolo_model::project::Project::new("Looks");
    p.assets = vec![asset("v")];
    p
}

fn set_on(
    p: &klipolo_model::project::Project,
    on: &str,
    from: f64,
    to: f64,
    look: Look,
) -> klipolo_model::project::Project {
    klipolo_model::apply_ops(
        p,
        &[Op::SetWordLook {
            asset_id: on.into(),
            ranges: vec![RangeUs {
                start: s(from),
                end: s(to),
            }],
            look,
        }],
    )
    .unwrap()
}

fn set(
    p: &klipolo_model::project::Project,
    from: f64,
    to: f64,
    look: Look,
) -> klipolo_model::project::Project {
    set_on(p, "v", from, to, look)
}

#[test]
fn setting_a_range_replaces_and_splits_what_it_overlaps() {
    let p = set(&with_asset(), 1.0, 5.0, red());
    let p = set(
        &p,
        2.0,
        3.0,
        Look {
            emphasis: Some(true),
            ..Look::default()
        },
    );
    let spans: Vec<(i64, i64, bool)> = p
        .word_looks
        .iter()
        .map(|l| (l.start_us, l.end_us, l.look.emphasis == Some(true)))
        .collect();
    assert_eq!(
        spans,
        [
            (s(1.0), s(2.0), false),
            (s(2.0), s(3.0), true),
            (s(3.0), s(5.0), false)
        ]
    );
    let index = LookIndex::new(&p.word_looks);
    assert_eq!(
        index.at("v", s(2.5)).unwrap().color,
        None,
        "replaced, not merged"
    );
    assert_eq!(
        index.at("v", s(4.0)).unwrap().color.as_deref(),
        Some("#FF3B30")
    );
    assert!(index.at("v", s(6.0)).is_none());
}

/// A range over several looks replaces every one inside it and trims the two at its edges; another file's looks, on
/// either side of it in the list, stay as they were.
#[test]
fn a_range_touches_only_its_own_files_looks() {
    let mut p = with_asset();
    p.assets.push(asset("a"));
    p.assets.push(asset("w"));
    let p = set_on(&p, "a", 0.0, 10.0, red());
    let p = set_on(&p, "w", 0.0, 10.0, red());
    let p = set(&p, 1.0, 2.0, red());
    let p = set(&p, 3.0, 4.0, red());
    let p = set(&p, 5.0, 6.0, red());
    let big = Look {
        size: Some(2),
        ..Look::default()
    };
    let p = set(&p, 1.5, 5.5, big.clone());
    let spans: Vec<(&str, i64, i64, bool)> = p
        .word_looks
        .iter()
        .map(|l| (l.asset_id.as_str(), l.start_us, l.end_us, l.look == big))
        .collect();
    assert_eq!(
        spans,
        [
            ("a", s(0.0), s(10.0), false),
            ("v", s(1.0), s(1.5), false),
            ("v", s(1.5), s(5.5), true),
            ("v", s(5.5), s(6.0), false),
            ("w", s(0.0), s(10.0), false)
        ]
    );
}

#[test]
fn an_empty_look_clears_its_range() {
    let p = set(&with_asset(), 1.0, 5.0, red());
    let p = set(&p, 0.0, 10.0, Look::default());
    assert!(p.word_looks.is_empty());
}

#[test]
fn a_look_is_one_undo_step_that_undo_and_redo_swap() {
    let mut h = klipolo_model::history::Document::new(with_asset()).unwrap();
    h.apply(&klipolo_model::ops::Transaction {
        label: "edit.captionWordLook".into(),
        ops: vec![Op::SetWordLook {
            asset_id: "v".into(),
            ranges: vec![RangeUs {
                start: s(1.0),
                end: s(2.0),
            }],
            look: red(),
        }],
        ..Default::default()
    })
    .unwrap();
    assert_eq!(h.project().word_looks.len(), 1);
    h.undo().unwrap();
    assert!(h.project().word_looks.is_empty());
    h.redo().unwrap();
    assert_eq!(h.project().word_looks.len(), 1);
    assert_eq!(h.project().word_looks[0].look, red());
}

/// Copy on write: an op that touches neither list leaves both shared with the project it came from, so a drag's
/// dry runs copy no looks or fixes, and the commit's diff skips both by pointer.
#[test]
fn an_unrelated_op_shares_the_looks_and_fixes() {
    let p = set(&with_asset(), 1.0, 2.0, red());
    let fix = Op::FixWords {
        asset_id: "v".into(),
        start_us: s(3.0),
        end_us: s(4.0),
        text: Some("Notion".into()),
    };
    let p = klipolo_model::apply_ops(&p, &[fix]).unwrap();
    let rename = Op::Rename {
        name: "Renamed".into(),
    };
    let next = klipolo_model::apply_ops(&p, &[rename]).unwrap();
    assert!(
        Arc::ptr_eq(&p.word_looks, &next.word_looks),
        "the looks were copied"
    );
    assert!(
        Arc::ptr_eq(&p.word_fixes, &next.word_fixes),
        "the fixes were copied"
    );
    let patch = klipolo_model::patch::diff(&p, &next);
    assert!(patch.word_looks.is_none() && patch.word_fixes.is_none());
}

#[test]
fn bad_looks_are_refused_with_their_codes() {
    let p = with_asset();
    let code = |asset_id: &str, start_us: i64, end_us: i64, look: Look| {
        let op = Op::SetWordLook {
            asset_id: asset_id.into(),
            ranges: vec![RangeUs {
                start: start_us,
                end: end_us,
            }],
            look,
        };
        klipolo_model::apply_ops(&p, &[op])
            .unwrap_err()
            .coded()
            .code
    };
    let bad = |look: Look| code("v", s(1.0), s(2.0), look);
    assert_eq!(
        bad(Look {
            color: Some("red".into()),
            ..Look::default()
        }),
        "model.wordLook.color"
    );
    assert_eq!(
        bad(Look {
            size: Some(4),
            ..Look::default()
        }),
        "model.wordLook.size"
    );
    assert_eq!(code("nope", 0, 1, red()), "model.wordLook.asset");
    assert_eq!(code("v", s(2.0), s(2.0), red()), "model.wordLook.range");
}

/// A file with looks that isn't used anywhere can still be removed: its looks go with it.
#[test]
fn removing_a_file_removes_its_looks() {
    let p = set(&with_asset(), 1.0, 2.0, red());
    let p = klipolo_model::apply_ops(&p, &[Op::RemoveAsset { id: "v".into() }]).unwrap();
    assert!(p.word_looks.is_empty());
}

#[test]
fn the_index_finds_a_look_by_file_and_source_time() {
    // The index is by asset and source time, so a look never depends on where its word plays:
    // `looks_follow_their_words_through_cuts_and_undo` checks that through a real cut and its undo.
    let looks = vec![WordLook {
        asset_id: "v".into(),
        start_us: s(10.0),
        end_us: s(10.4),
        look: red(),
    }];
    let index = LookIndex::new(&looks);
    assert!(index.at("v", s(10.2)).is_some());
    assert!(index.at("other", s(10.2)).is_none());
}
