mod common;
use common::*;
use klipolo_model::history::Document;
use klipolo_model::summary::ProjectSummary;
use klipolo_model::{Op, Transaction};

#[test]
fn snapshot_carries_project_and_history() {
    let mut doc = Document::new(fixture()).unwrap();
    doc.apply(&Transaction {
        label: "Rename".into(),
        ops: vec![Op::Rename {
            name: "Demo".into(),
        }],
        origin: None,
        ..Default::default()
    })
    .unwrap();
    let snap = doc.snapshot();
    assert_eq!(snap.project.name, "Demo");
    assert_eq!(snap.history.undo_label.as_deref(), Some("Rename"));
    let json = serde_json::to_value(&snap).unwrap();
    assert_eq!(json["history"]["canUndo"], true);
}

#[test]
fn summary_describes_project() {
    let s = ProjectSummary::of(&fixture(), 1_700_000_000_000);
    assert_eq!((s.id.as_str(), s.name.as_str()), ("project", "Fixture"));
    assert_eq!(s.duration_us, secs(30.0));
    assert_eq!(s.clip_count, 2);
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["updatedAtMs"], 1_700_000_000_000u64);
    // The first clip is a recording: its screen's first frame.
    let picture = s.thumbnail.unwrap();
    assert_eq!(picture.at_us, 0);
    assert_eq!(
        Some(picture.path.as_str()),
        fixture().asset("screen").map(|a| a.path.as_str())
    );
}
