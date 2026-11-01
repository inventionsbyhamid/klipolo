//! Separate camera (`DetachSource` of a recording clip's camera) is one undo step: undone, the clip has its camera back
//! as it was in every format and the layers are gone, so it can be separated again; redone, the same layers return.

mod common;
use common::*;
use klipolo_model::history::Document;
use klipolo_model::timeline::{CameraMode, ItemContent};
use klipolo_model::{Op, Project, Transaction};

fn separate() -> Transaction {
    Transaction {
        label: "undo.separateCamera".into(),
        ops: vec![Op::DetachSource {
            clip_id: "a".into(),
            source_id: "camera".into(),
        }],
        origin: None,
        ..Default::default()
    }
}

/// The ids of the layers that play the camera's file.
fn camera_layers(p: &Project) -> Vec<&str> {
    (p.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .filter(
            |i| matches!(&i.content, ItemContent::Video { asset_id, .. } if asset_id == "camera"),
        )
        .map(|i| i.id.as_str())
        .collect()
}

#[test]
fn separating_the_camera_is_one_step_and_undo_takes_it_back_whole() {
    let mut doc = Document::new(fixture()).unwrap();
    let before = doc.project().clone();
    doc.apply(&separate()).unwrap();
    let after = doc.project().clone();
    // The clip's own camera is hidden in every format; a layer plays it.
    let camera = after.timeline.main[0].camera.as_ref().unwrap();
    assert_eq!(camera.mode, CameraMode::Hidden);
    assert!(camera
        .mode_overrides
        .values()
        .all(|&m| m == CameraMode::Hidden));
    assert_eq!(camera_layers(&after), ["a-camera"]);
    assert_eq!(
        doc.history().undo_label.as_deref(),
        Some("undo.separateCamera")
    );

    doc.undo().unwrap();
    assert_eq!(doc.project(), &before);
    assert!(camera_layers(doc.project()).is_empty());
    // It can be separated again.
    assert!(klipolo_model::apply_ops(doc.project(), &separate().ops).is_ok());

    doc.redo().unwrap();
    assert_eq!(doc.project(), &after);
}
