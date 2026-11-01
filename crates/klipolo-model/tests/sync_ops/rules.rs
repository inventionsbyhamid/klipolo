//! Sync's rules: `useRecording` moves an anchor exactly when it resolves to the clip it converts
//! and refuses a clip whose stretch of its file isn't inside the run's; a re-time refuses a picture source, the
//! recording's clock.

use klipolo_model::apply::apply_ops;
use klipolo_model::ops::{Op, SourcePatch};
use klipolo_model::query::resolve_attach;
use klipolo_model::timeline::{Attach, ClipSource};

use super::common::{self, secs};
use super::{run, synced, SURE};

fn use_recording(clip: &str) -> Op {
    Op::UseRecording {
        clip_id: clip.into(),
        recording_id: "r9".into(),
    }
}

#[test]
fn an_anchor_moves_with_the_clip_it_resolves_to() {
    // A title tied to c2 at the file's 470 s, which c2 (150–400 s) doesn't play: it resolves to the copy c9 (450–500 s).
    let mut p = common::imported();
    let mut title = common::text_item("t7", 0.0, 2.0);
    let attach = Attach {
        clip_id: "c2".into(),
        source: ClipSource::Asset { id: "cam".into() },
        source_us: secs(470.0),
    };
    title.start_us = resolve_attach(&p.timeline.main, &attach).unwrap();
    title.attach = Some(attach);
    p.timeline
        .tracks
        .push(common::overlay_track("seven", vec![title]));
    let at = |p: &klipolo_model::Project| {
        let item = p
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.items)
            .find(|i| i.id == "t7")
            .unwrap();
        let a = item.attach.clone().unwrap();
        (a.source.clone(), resolve_attach(&p.timeline.main, &a))
    };
    let start = at(&p).1;
    let add = Op::AddRecording {
        recording: synced(),
    };
    // Converting c2 alone leaves it on c9, which still shows the file.
    let q = apply_ops(&p, &[add.clone(), use_recording("c2")]).unwrap();
    assert_eq!(at(&q), (ClipSource::Asset { id: "cam".into() }, start));
    // Converting c9 as well moves it onto the recording, where it plays at the same place, in either order.
    for order in [["c2", "c9"], ["c9", "c2"]] {
        let ops = [
            add.clone(),
            use_recording(order[0]),
            use_recording(order[1]),
        ];
        let q = apply_ops(&p, &ops).unwrap();
        assert_eq!(
            at(&q),
            (ClipSource::Recording { id: "r9".into() }, start),
            "{order:?}"
        );
    }
}

#[test]
fn a_clip_outside_its_run_s_stretch_of_the_file_is_refused() {
    // The camera's run holds only the file's first 300 s: c2 (150–400 s) runs past it.
    let p = common::imported();
    let mut short = synced();
    short.sources[0].runs = vec![run("cam", 0, secs(300.0), 0.0)];
    let add = Op::AddRecording { recording: short };
    apply_ops(&p, &[add.clone(), use_recording("c1")]).expect("c1 (10–100 s) is inside it");
    assert_eq!(
        apply_ops(&p, &[add, use_recording("c2")]).unwrap_err(),
        common::refused("That clip's file isn't part of this recording.")
    );
}

#[test]
fn a_re_time_refuses_the_pictures() {
    let p = apply_ops(
        &common::imported(),
        &[Op::AddRecording {
            recording: synced(),
        }],
    )
    .unwrap();
    let retime = Op::UpdateRecordingSource {
        recording_id: "r9".into(),
        source_id: "cam".into(),
        patch: SourcePatch {
            runs: Some(vec![run("cam", secs(1.0), secs(599.0), 0.0)]),
            synced: SURE,
            ..SourcePatch::default()
        },
    };
    assert_eq!(
        apply_ops(&p, &[retime]).unwrap_err(),
        common::refused("Sync lines up sound only: the pictures keep their timing.")
    );
}

#[test]
fn a_recorder_at_1_200_ppm_validates() {
    // Sync searches ±1,200 ppm; 1,500 is the cap.
    let at = |ppm: f64| {
        let mut r = synced();
        r.sources[1].runs[0].drift_ppm = ppm;
        apply_ops(&common::imported(), &[Op::AddRecording { recording: r }])
    };
    at(1_200.0).unwrap();
    at(-1_500.0).unwrap();
    assert_eq!(
        at(1_600.0).unwrap_err(),
        common::refused("A source's clock correction is out of range.")
    );
}
