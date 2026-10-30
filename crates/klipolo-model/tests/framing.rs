mod common;
use std::collections::BTreeMap;

use common::*;
use klipolo_model::ops::ClipPatch;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::timeline::{Framing, FramingMode};
use klipolo_model::{apply_ops, Op};

fn set(framing: BTreeMap<FormatKind, Framing>) -> Op {
    Op::UpdateClip {
        id: "a".into(),
        patch: ClipPatch {
            framing: Some(framing),
            ..Default::default()
        },
    }
}

#[test]
fn framing_is_per_format_and_validated() {
    let p = fixture();
    let mut framing = BTreeMap::new();
    framing.insert(
        FormatKind::Portrait,
        Framing {
            mode: FramingMode::Fill,
            focus_x: 0.2,
            focus_y: 0.5,
            follow: None,
        },
    );
    let next = apply_ops(&p, &[set(framing.clone())]).unwrap();
    assert_eq!(next.timeline.main[0].framing, framing);
    framing.insert(
        FormatKind::Square,
        Framing {
            mode: FramingMode::Fit,
            focus_x: 1.5,
            focus_y: 0.5,
            follow: None,
        },
    );
    assert!(apply_ops(&p, &[set(framing)]).is_err());
}

#[test]
fn projects_saved_before_framing_still_load() {
    let mut json = serde_json::to_value(fixture()).unwrap();
    json["timeline"]["main"][0]
        .as_object_mut()
        .unwrap()
        .remove("framing");
    let p: Project = serde_json::from_value(json).unwrap();
    assert!(p.timeline.main[0].framing.is_empty());
}
