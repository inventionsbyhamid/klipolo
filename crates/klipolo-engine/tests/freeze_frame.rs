//! A freeze frame plays as its frame held still: every picture of its clip at that moment, and no sound. An imported
//! clip's sound on a layer of its own plays as the clip did.
mod support;
use std::sync::Arc;

use klipolo_engine::audio::plan::audio_plan;
use klipolo_engine::plan::frame_plan;
use klipolo_engine::registry::Registry;
use klipolo_model::apply_ops;
use klipolo_model::ops::Op;
use klipolo_model::project::FormatKind::*;
use support::*;

fn freeze(at: f64, seconds: f64) -> Op {
    Op::FreezeFrame {
        at_us: secs(at),
        duration_us: secs(seconds),
    }
}

#[test]
fn a_frozen_recording_holds_its_screen_and_camera_at_the_frame() {
    let c = ctx(apply_ops(&project(), &[freeze(5.0, 2.0)]).unwrap());
    for at in [secs(5.0), secs(6.0), secs(7.0) - 1] {
        let plan = frame_plan(&c, at, Landscape);
        let v = videos(&plan);
        assert_eq!(
            (v[0].id.as_ref(), v[0].source.time_us, v[1].source.time_us),
            ("a-freeze", secs(5.0), secs(4.8)),
            "at {at}"
        );
    }
    // After it the take plays on from the frame.
    let plan = frame_plan(&c, secs(7.0), Landscape);
    assert_eq!(
        (
            videos(&plan)[0].id.as_ref(),
            videos(&plan)[0].source.time_us
        ),
        ("a-2", secs(5.0))
    );
}

#[test]
fn a_freeze_of_an_imported_clip_holds_past_its_file_s_end() {
    // The b-roll's last frame, held for 4 s after its 10 s.
    let c = ctx(apply_ops(&project(), &[freeze(30.0, 4.0)]).unwrap());
    let plan = frame_plan(&c, secs(33.0), Landscape);
    assert_eq!(
        (
            videos(&plan)[0].id.as_ref(),
            videos(&plan)[0].source.time_us
        ),
        ("b-freeze", secs(10.0) - 1)
    );
}

#[test]
fn a_freeze_frame_plays_no_sound() {
    let p = apply_ops(&project(), &[freeze(25.0, 2.0)]).unwrap();
    let plan = audio_plan(&p, &Registry::builtin(), Arc::default());
    assert!(!plan.sources.is_empty());
    for s in &plan.sources {
        assert!(
            s.end_us <= secs(25.0) || s.start_us >= secs(27.0),
            "{} to {} plays during the freeze",
            s.start_us,
            s.end_us
        );
    }
}

#[test]
fn an_imported_clip_s_separated_sound_plays_as_the_clip_played_it() {
    let before = audio_plan(&project(), &Registry::builtin(), Arc::default());
    let p = apply_ops(
        &project(),
        &[Op::DetachSound {
            clip_id: "b".into(),
        }],
    )
    .unwrap();
    let after = audio_plan(&p, &Registry::builtin(), Arc::default());
    let broll = |plan: &klipolo_engine::audio::plan::AudioPlan| {
        plan.sources
            .iter()
            .filter(|s| s.path.ends_with("broll"))
            .map(|s| (s.start_us, s.end_us, s.source_in_us, s.gain, s.speed))
            .collect::<Vec<_>>()
    };
    assert_eq!(broll(&after), broll(&before));
    assert_eq!(broll(&after).len(), 1);
}
