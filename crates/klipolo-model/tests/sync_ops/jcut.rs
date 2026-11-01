//! Layers tied by where they came from follow the sync: a separated layer the person extended
//! back into the previous clip (a J cut) follows a nudge and a re-time, though it starts over another clip.

use klipolo_model::apply::apply_ops;
use klipolo_model::ops::{Edge, Op, SourcePatch};
use klipolo_model::timeline::{Item, ItemContent};
use klipolo_model::Project;

use super::common::{self, secs};
use super::{run, synced, SURE};

fn layer<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap()
}

fn from(item: &Item) -> i64 {
    match &item.content {
        ItemContent::Audio { source_in_us, .. } => *source_in_us,
        other => panic!("{other:?}"),
    }
}

/// The imported interview synced with c2 at normal speed, the mic separated on c1 and c2, and a J cut at 1:29: c1's
/// sound ends a second early and c2's starts a second early, under c1.
fn j_cut() -> Project {
    let mut p = common::imported();
    p.timeline.main[1].speed = 1.0;
    p.timeline.tracks[1].items.clear();
    let ops = vec![
        Op::AddRecording {
            recording: synced(),
        },
        Op::UseRecording {
            clip_id: "c1".into(),
            recording_id: "r9".into(),
        },
        Op::UseRecording {
            clip_id: "c2".into(),
            recording_id: "r9".into(),
        },
        Op::DetachSource {
            clip_id: "c1".into(),
            source_id: "mic".into(),
        },
        Op::DetachSource {
            clip_id: "c2".into(),
            source_id: "mic".into(),
        },
        Op::TrimItem {
            id: "c1-mic".into(),
            edge: Edge::End,
            at_us: secs(89.0),
        },
        Op::TrimItem {
            id: "c2-mic".into(),
            edge: Edge::Start,
            at_us: secs(89.0),
        },
    ];
    apply_ops(&p, &ops).unwrap()
}

#[test]
fn a_j_cut_extension_follows_a_nudge_and_a_retime() {
    let p = j_cut();
    assert_eq!(layer(&p, "c2-mic").start_us, secs(89.0));
    let before = from(layer(&p, "c2-mic"));
    let nudge = Op::UpdateRecordingSource {
        recording_id: "r9".into(),
        source_id: "mic".into(),
        patch: SourcePatch {
            nudge_us: Some(100_000),
            ..SourcePatch::default()
        },
    };
    let nudged = apply_ops(&p, &[nudge]).unwrap();
    // 100 ms later at its 50 ppm: it plays 100,005 µs earlier in its file.
    assert_eq!(from(layer(&nudged, "c2-mic")), before - 100_005);
    // A re-time moves it to where the new run plays the moment it played.
    let old = nudged.recording("r9").unwrap().source("mic").unwrap();
    let rec = old.runs[0].rec_of(from(layer(&nudged, "c2-mic")), old.nudge_us);
    let new_run = run("mic", secs(90.3), secs(609.7), 50.0);
    let retime = Op::UpdateRecordingSource {
        recording_id: "r9".into(),
        source_id: "mic".into(),
        patch: SourcePatch {
            runs: Some(vec![new_run.clone()]),
            synced: SURE,
            ..SourcePatch::default()
        },
    };
    let q = apply_ops(&nudged, &[retime]).unwrap();
    let moved = from(layer(&q, "c2-mic"));
    assert!((moved - new_run.local_us(rec, 0)).abs() <= 1, "{moved}");
}

#[test]
fn a_tied_layer_on_a_locked_track_names_its_time() {
    // The first tied layer on the locked track is c1's, at 0:00.
    let mut p = j_cut();
    for track in &mut p.timeline.tracks {
        if track.items.iter().any(|i| i.id == "c2-mic") {
            track.locked = true;
        }
    }
    let nudge = Op::UpdateRecordingSource {
        recording_id: "r9".into(),
        source_id: "mic".into(),
        patch: SourcePatch {
            nudge_us: Some(100_000),
            ..SourcePatch::default()
        },
    };
    let e = apply_ops(&p, &[nudge]).unwrap_err();
    let words = klipolo_model::coded::english(&[klipolo_model::messages::MESSAGES], &e.coded());
    assert_eq!(
        words,
        "The layer at 0:00 is on a locked track, so it can't follow that sync change."
    );
}
