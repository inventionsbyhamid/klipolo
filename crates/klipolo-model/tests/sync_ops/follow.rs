//! Layers tied to a synced source follow its sync: a re-time moves each layer tied to the source
//! to where the new runs play the moment it played, keeping its cuts, fades and volume; a nudge slips only tied
//! layers. A layer nothing tied, such as a reused stretch of the file, is left alone.

use klipolo_model::apply::apply_ops;
use klipolo_model::ops::{Op, SourcePatch};
use klipolo_model::timeline::{Item, ItemContent, Tie};
use klipolo_model::Project;

use super::common::{self, secs};
use super::{run, synced, SURE};

/// The imported interview synced: c1 and c2 play r9; i9 (the mic from its 100 s under c1, at 70 % with a 0.5 s fade in)
/// is tied to the mic, as Sync leaves a layer it lined up; i10 plays the mic from its 500 s over c2 (a reused stretch),
/// tied to nothing.
fn with_layers() -> Project {
    let mut p = common::imported();
    let i9 = &mut p.timeline.tracks[1].items[0];
    if let ItemContent::Audio {
        volume, fade_in_us, ..
    } = &mut i9.content
    {
        *volume = 0.7;
        *fade_in_us = secs(0.5);
    }
    i9.content.set_tie(Some(Tie {
        recording: "r9".into(),
        source: "mic".into(),
    }));
    let mut i10 = p.timeline.tracks[1].items[0].clone();
    i10.id = "i10".into();
    i10.content.set_tie(None);
    (i10.start_us, i10.duration_us) = (secs(100.0), secs(30.0));
    if let ItemContent::Audio { source_in_us, .. } = &mut i10.content {
        *source_in_us = secs(500.0);
    }
    p.timeline.tracks[1].items.push(i10);
    let use_recording = |clip: &str| Op::UseRecording {
        clip_id: clip.into(),
        recording_id: "r9".into(),
    };
    let ops = [
        Op::AddRecording {
            recording: synced(),
        },
        use_recording("c1"),
        use_recording("c2"),
    ];
    apply_ops(&p, &ops).unwrap()
}

fn layer<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap()
}

/// (start, length, file time, drift, volume, fade in) of a sound layer.
fn audio(item: &Item) -> (i64, i64, i64, f64, f64, i64) {
    match &item.content {
        ItemContent::Audio {
            source_in_us,
            drift_ppm,
            volume,
            fade_in_us,
            ..
        } => (
            item.start_us,
            item.duration_us,
            *source_in_us,
            *drift_ppm,
            *volume,
            *fade_in_us,
        ),
        other => panic!("{other:?}"),
    }
}

fn mic_patch(patch: SourcePatch) -> Op {
    Op::UpdateRecordingSource {
        recording_id: "r9".into(),
        source_id: "mic".into(),
        patch,
    }
}

#[test]
fn a_retime_moves_the_layers_tied_to_its_clips_and_leaves_a_reused_stretch() {
    let p = with_layers();
    let reused = audio(layer(&p, "i10"));
    let retime = mic_patch(SourcePatch {
        runs: Some(vec![run("mic", secs(90.3), secs(609.7), 50.0)]),
        synced: SURE,
        ..SourcePatch::default()
    });
    let q = apply_ops(&p, &[retime]).expect("a re-time with layers of the file on the timeline");
    // i9 played the mic's 100 s, the recording's 9.9995 s; the new run plays that moment at 100.3 s.
    assert_eq!(
        audio(layer(&q, "i9")),
        (0, secs(90.0), 100_300_000, 50.0, 0.7, secs(0.5))
    );
    assert_eq!(audio(layer(&q, "i10")), reused);
    assert_eq!(
        q.recording("r9").unwrap().source("mic").unwrap().mix,
        p.recording("r9").unwrap().source("mic").unwrap().mix
    );
}

#[test]
fn a_nudge_slips_only_the_layers_tied_to_the_clips() {
    let p = with_layers();
    let reused = audio(layer(&p, "i10"));
    let nudge = mic_patch(SourcePatch {
        nudge_us: Some(100_000),
        ..SourcePatch::default()
    });
    let q = apply_ops(&p, &[nudge]).unwrap();
    assert_eq!(audio(layer(&q, "i9")).2, secs(100.0) - 100_000);
    assert_eq!(audio(layer(&q, "i10")), reused);
}
