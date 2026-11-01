//! The camera's sound fills true gaps: only a picture source whose files carry sound may fill them, and the
//! voice runs (what captions read) take the camera's sound where it fills a gap, never where the mic is only muted.

use klipolo_model::apply::apply_ops;
use klipolo_model::ops::{Op, SourcePatch};
use klipolo_model::project::{SourceMix, SourceRun};
use klipolo_model::speech::voice_runs;

use super::common::{self, secs};
use super::synced;

const NO_FILL: &str = "Only a video's own sound can fill gaps.";

fn fills() -> SourceMix {
    SourceMix {
        muted: true,
        fills_gaps: true,
        ..SourceMix::default()
    }
}

fn mix_of(recording: &str, source: &str, mix: SourceMix) -> Op {
    Op::UpdateRecordingSource {
        recording_id: recording.into(),
        source_id: source.into(),
        patch: SourcePatch {
            mix: Some(mix),
            ..SourcePatch::default()
        },
    }
}

#[test]
fn fills_gaps_is_only_for_a_picture_source_with_sound() {
    let p = common::imported();
    let mut rec = synced();
    rec.sources[0].mix = fills();
    let q =
        apply_ops(&p, &[Op::AddRecording { recording: rec }]).expect("a camera with sound fills");
    assert!(q.recordings[0].sources[0].mix.fills_gaps);
    // The mic, by default or in a clip of its own; a take's camera, whose file has no sound.
    let refused = common::refused(NO_FILL);
    assert_eq!(
        apply_ops(&q, &[mix_of("r9", "mic", fills())]).unwrap_err(),
        refused
    );
    let converted = apply_ops(
        &q,
        &[Op::UseRecording {
            clip_id: "c1".into(),
            recording_id: "r9".into(),
        }],
    )
    .unwrap();
    let own = Op::SetClipSourceMix {
        clip_id: "c1".into(),
        source_id: "mic".into(),
        mix: Some(fills()),
    };
    assert_eq!(apply_ops(&converted, &[own]).unwrap_err(), refused);
    let take = common::fixture();
    assert_eq!(
        apply_ops(&take, &[mix_of("rec", "camera", fills())]).unwrap_err(),
        refused
    );
}

#[test]
fn captions_follow_the_camera_over_a_true_gap() {
    // The mic recorded from the recording's 20 s on: c1 (10–100 s) reads the camera's transcript over 10–20 s, then
    // the mic's. Muting the mic in c1 is the person's choice, not a gap: the mic stays the voice there.
    let mut rec = synced();
    rec.sources[0].mix = fills();
    rec.sources[1].runs = vec![SourceRun {
        asset_id: "mic".into(),
        offset_us: secs(20.0),
        duration_us: secs(480.0),
        drift_ppm: 0.0,
        file_in_us: 0,
    }];
    let ops = [
        Op::AddRecording { recording: rec },
        Op::UseRecording {
            clip_id: "c1".into(),
            recording_id: "r9".into(),
        },
    ];
    let q = apply_ops(&common::imported(), &ops).unwrap();
    let voice = |p: &klipolo_model::Project| -> Vec<(String, i64, i64)> {
        voice_runs(p, common::clip(p, "c1"))
            .into_iter()
            .map(|r| (r.asset, r.rec_from, r.rec_to))
            .collect()
    };
    let gapped = [
        ("cam".to_owned(), secs(10.0), secs(20.0)),
        ("mic".to_owned(), secs(20.0), secs(100.0)),
    ];
    assert_eq!(voice(&q), gapped);
    // The camera's words over the gap map through its sound time: its picture's time less the file's sound lead;
    // 40 ms here.
    let mut led = common::imported();
    let cam = led.assets.iter_mut().find(|a| a.id == "cam").unwrap();
    cam.audio_lead_us = Some(40_000);
    let led = apply_ops(&led, &ops).unwrap();
    let runs = voice_runs(&led, common::clip(&led, "c1"));
    let camera = runs.iter().find(|r| r.asset == "cam").unwrap();
    assert_eq!(camera.local_us(secs(15.0)), secs(15.0) - 40_000);
    let muted = Op::SetClipSourceMix {
        clip_id: "c1".into(),
        source_id: "mic".into(),
        mix: Some(SourceMix {
            muted: true,
            ..SourceMix::default()
        }),
    };
    assert_eq!(
        voice(&apply_ops(&q, &[muted]).unwrap()),
        gapped,
        "a mute isn't a gap"
    );
    // Without the fill, nothing plays over 10–20 s: the mic alone.
    let silent = apply_ops(
        &q,
        &[mix_of(
            "r9",
            "cam",
            SourceMix {
                muted: true,
                ..SourceMix::default()
            },
        )],
    )
    .unwrap();
    assert_eq!(
        voice(&silent),
        [("mic".to_owned(), secs(20.0), secs(100.0))]
    );
}
