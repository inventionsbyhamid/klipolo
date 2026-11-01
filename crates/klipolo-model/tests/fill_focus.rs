mod common;
use std::collections::BTreeMap;

use common::*;
use klipolo_model::fill_focus::{FillFocus, FocusKey, FocusPath};
use klipolo_model::ops::ClipPatch;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::{apply_ops, Op};

fn key(at_s: f64, x: f64, y: f64) -> FocusKey {
    FocusKey {
        at_us: secs(at_s),
        x,
        y,
        cut: false,
    }
}

fn cut(at_s: f64, x: f64, y: f64) -> FocusKey {
    FocusKey {
        cut: true,
        ..key(at_s, x, y)
    }
}

fn speaker(keys: Vec<FocusKey>) -> FillFocus {
    FillFocus::Speaker {
        path: FocusPath { keys },
    }
}

fn set(id: &str, focus: BTreeMap<FormatKind, FillFocus>) -> Op {
    Op::UpdateClip {
        id: id.into(),
        patch: ClipPatch {
            fill_focus: Some(focus),
            ..Default::default()
        },
    }
}

fn close(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
}

#[test]
fn fill_focus_roundtrips_and_old_projects_load() {
    let p = fixture();
    let focus = BTreeMap::from([
        (
            FormatKind::Portrait,
            speaker(vec![key(0.0, 0.3, 0.4), key(1.0, 0.35, 0.4)]),
        ),
        (FormatKind::Square, FillFocus::Center),
    ]);
    let next = apply_ops(&p, &[set("b", focus.clone())]).unwrap();
    assert_eq!(next.timeline.main[1].fill_focus, focus);

    let json = serde_json::to_value(&next).unwrap();
    let back: Project = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back.timeline.main[1].fill_focus, focus);
    assert_eq!(
        json["timeline"]["main"][1]["fillFocus"]["square"],
        serde_json::json!({"kind": "center"})
    );
    // A key that isn't a cut doesn't write `cut`.
    let first = &json["timeline"]["main"][1]["fillFocus"]["portrait"]["path"]["keys"][0];
    assert!(first.get("cut").is_none(), "{first}");

    // A clip with no focus of its own writes nothing, and a project saved before the field loads with none: every
    // format keeps its framing's fixed focus.
    let old = serde_json::to_value(fixture()).unwrap();
    assert!(old["timeline"]["main"][1].get("fillFocus").is_none());
    let loaded: Project = serde_json::from_value(old).unwrap();
    assert!(loaded.timeline.main.iter().all(|c| c.fill_focus.is_empty()));
}

#[test]
fn the_path_eases_between_keys_and_jumps_at_a_cut() {
    let path = FocusPath {
        keys: vec![
            key(1.0, 0.2, 0.5),
            key(2.0, 0.4, 0.5),
            cut(3.0, 0.8, 0.3),
            key(4.0, 0.8, 0.3),
        ],
    };
    // Before the first key and after the last, the nearest one holds.
    assert!(close(path.centre_at(0).unwrap(), (0.2, 0.5)));
    assert!(close(path.centre_at(secs(9.0)).unwrap(), (0.8, 0.3)));
    // Between two keys of a shot it moves straight from one to the next.
    assert!(close(path.centre_at(secs(1.5)).unwrap(), (0.3, 0.5)));
    // Up to a cut it holds the shot's last key, then jumps: it never pans across a cut.
    assert!(close(path.centre_at(secs(2.5)).unwrap(), (0.4, 0.5)));
    assert!(close(path.centre_at(secs(3.0) - 1).unwrap(), (0.4, 0.5)));
    assert!(close(path.centre_at(secs(3.0)).unwrap(), (0.8, 0.3)));
    assert_eq!(FocusPath::default().centre_at(0), None);
}

#[test]
fn a_bad_path_is_refused_with_its_code() {
    let p = fixture();
    let code = |op: Op| apply_ops(&p, &[op]).unwrap_err().coded().code;
    let one = |keys| BTreeMap::from([(FormatKind::Portrait, speaker(keys))]);
    // Out of time order, the same time twice, outside the picture, not a number.
    for keys in [
        vec![key(2.0, 0.5, 0.5), key(1.0, 0.5, 0.5)],
        vec![key(1.0, 0.5, 0.5), key(1.0, 0.6, 0.5)],
        vec![key(1.0, 1.2, 0.5)],
        vec![key(1.0, 0.5, f64::NAN)],
    ] {
        assert_eq!(code(set("b", one(keys))), "model.clip.fillFocus");
    }
    let many = (0..=FocusPath::MAX_KEYS as i64)
        .map(|i| FocusKey {
            at_us: i * 1000,
            x: 0.5,
            y: 0.5,
            cut: false,
        })
        .collect();
    assert_eq!(code(set("b", one(many))), "model.clip.fillFocus");
    // An empty path is fine: the framing's focus holds until one is made.
    assert!(apply_ops(&p, &[set("b", one(vec![]))]).is_ok());
}

#[test]
fn a_split_keeps_the_path_on_both_halves_in_source_time() {
    let p = fixture();
    let focus = BTreeMap::from([(
        FormatKind::Portrait,
        speaker(vec![key(0.0, 0.2, 0.5), key(9.0, 0.7, 0.5)]),
    )]);
    let p = apply_ops(&p, &[set("b", focus.clone())]).unwrap();
    // Clip b plays the b-roll from 0 at 20 s on the timeline: split it 4 s in.
    let p = apply_ops(&p, &[Op::Split { at_us: secs(24.0) }]).unwrap();
    let (left, right) = (&p.timeline.main[1], &p.timeline.main[2]);
    assert_eq!(left.fill_focus, focus);
    assert_eq!(right.fill_focus, focus);
    assert_eq!(right.source_in_us, secs(4.0));
    // The right half's first frame shows the path where it was at source 4 s.
    let FillFocus::Speaker { path } = &right.fill_focus[&FormatKind::Portrait] else {
        panic!()
    };
    let x = path.centre_at(right.source_in_us).unwrap().0;
    assert!((x - (0.2 + 0.5 * 4.0 / 9.0)).abs() < 1e-9, "{x}");
}

/// A 10-minute talk followed from the card: a key every quarter second, the speaker always moving a little.
fn followed_talk() -> Project {
    let mut p = fixture();
    let broll = p.assets.iter_mut().find(|a| a.id == "broll").unwrap();
    broll.duration_us = secs(600.0);
    let b = &mut p.timeline.main[1];
    b.source_out_us = secs(600.0);
    let keys = (0..2400)
        .map(|i| {
            key(
                f64::from(i) * 0.25,
                0.3 + 0.2 * (f64::from(i) / 40.0).sin().abs(),
                0.5,
            )
        })
        .collect();
    b.fill_focus.insert(FormatKind::Portrait, speaker(keys));
    p
}

#[test]
fn split_pieces_keep_only_their_own_part_of_the_path() {
    let p = followed_talk();
    // Clip b plays from 20 s on the timeline: cut it into 80 pieces.
    let splits: Vec<Op> = (1..80)
        .map(|k| Op::Split {
            at_us: secs(20.0 + 7.5 * f64::from(k)),
        })
        .collect();
    let cut = apply_ops(&p, &splits).unwrap();
    assert_eq!(cut.timeline.main.len(), 81);
    let json = serde_json::to_string(&cut).unwrap();
    assert!(json.len() < 1_000_000, "{} bytes", json.len());
    // Each piece still draws the crop it did: its part of the path, from the key before its first frame.
    let FillFocus::Speaker { path: whole } = &p.timeline.main[1].fill_focus[&FormatKind::Portrait]
    else {
        panic!()
    };
    for piece in &cut.timeline.main[1..] {
        let FillFocus::Speaker { path } = &piece.fill_focus[&FormatKind::Portrait] else {
            panic!()
        };
        assert!(path.keys.len() <= 32, "{}", path.keys.len());
        assert!(!path.keys[0].cut);
        for at in [
            piece.source_in_us,
            (piece.source_in_us + piece.source_out_us) / 2,
            piece.source_out_us - 1,
        ] {
            let (a, b) = (path.centre_at(at).unwrap(), whole.centre_at(at).unwrap());
            assert!((a.0 - b.0).abs() < 1e-9, "at {at}: {a:?} vs {b:?}");
        }
    }
}
