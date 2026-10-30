//! What a project file or an outside app can send as word looks: hand-edited lists that break the order, too many
//! looks, misspelled keys, and looks that only touch.

use klipolo_model::captions::looks::{Look, LookIndex, WordLook};
use klipolo_model::ops::Op;
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::time::{secs_to_us as s, RangeUs};
use klipolo_model::validate::MAX_WORD_LOOKS;

fn red() -> Look {
    Look {
        color: Some("#FF3B30".into()),
        ..Look::default()
    }
}

fn look(asset: &str, from: f64, to: f64) -> WordLook {
    WordLook {
        asset_id: asset.into(),
        start_us: s(from),
        end_us: s(to),
        look: red(),
    }
}

/// A project with files `v` and `w`.
fn project() -> Project {
    let mut p = Project::new("Looks");
    p.assets = ["v", "w"]
        .map(|id| Asset {
            id: id.into(),
            kind: AssetKind::Video,
            path: format!("/m/{id}.mp4"),
            name: id.into(),
            duration_us: s(60.0),
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
        })
        .to_vec();
    p
}

fn checked(looks: Vec<WordLook>) -> Result<(), String> {
    let mut p = project();
    p.word_looks = looks.into();
    klipolo_model::validate(&p).map_err(|e| e.coded().code)
}

/// `LookIndex` and `set_word_looks` need the list sorted by file and start and never overlapping: a file edited by hand
/// that breaks either is refused when it opens, and so is one over the cap.
#[test]
fn a_list_out_of_order_overlapping_or_over_the_cap_is_refused() {
    let overlap = Err("model.wordLook.overlap".to_string());
    assert_eq!(
        checked(vec![look("v", 1.0, 3.0), look("v", 2.0, 4.0)]),
        overlap
    );
    assert_eq!(
        checked(vec![look("v", 3.0, 4.0), look("v", 1.0, 2.0)]),
        overlap
    );
    assert_eq!(
        checked(vec![look("w", 1.0, 2.0), look("v", 3.0, 4.0)]),
        overlap
    );
    assert_eq!(
        checked(vec![
            look("v", 1.0, 2.0),
            look("v", 2.0, 3.0),
            look("w", 0.0, 1.0)
        ]),
        Ok(())
    );
    let many = (0..=MAX_WORD_LOOKS as i64)
        .map(|i| WordLook {
            start_us: i * 10,
            end_us: i * 10 + 5,
            ..look("v", 0.0, 1.0)
        })
        .collect();
    assert_eq!(checked(many), Err("model.wordLook.tooMany".to_string()));
}

/// An outside app's misspelled key (`colour`, `line_break`) is refused, never read as an empty look, which would
/// clear the range.
#[test]
fn a_misspelled_key_is_refused_not_read_as_an_empty_look() {
    let op = |look: serde_json::Value| {
        serde_json::from_value::<Op>(serde_json::json!({
            "type": "setWordLook", "assetId": "v", "ranges": [{ "start": 0, "end": 1_000_000 }], "look": look,
        }))
    };
    assert!(op(serde_json::json!({ "colour": "#FF0000" })).is_err());
    assert!(op(serde_json::json!({ "line_break": true })).is_err());
    let Op::SetWordLook { look, .. } = op(serde_json::json!({ "lineBreak": true })).unwrap() else {
        panic!("not a setWordLook");
    };
    assert_eq!(look.line_break, Some(true));
}

/// Ranges are half-open: looks that only touch both stay, and a look ends where its range does.
#[test]
fn looks_that_only_touch_both_stay() {
    let set = |p: &Project, from: f64, to: f64| {
        let op = Op::SetWordLook {
            asset_id: "v".into(),
            ranges: vec![RangeUs {
                start: s(from),
                end: s(to),
            }],
            look: red(),
        };
        klipolo_model::apply_ops(p, &[op]).unwrap()
    };
    let p = set(&set(&project(), 1.0, 2.0), 2.0, 3.0);
    assert_eq!(*p.word_looks, [look("v", 1.0, 2.0), look("v", 2.0, 3.0)]);
    let index = LookIndex::new(&p.word_looks);
    assert!(index.at("v", s(3.0)).is_none());
    assert!(index.at("v", s(3.0) - 1).is_some());
}
