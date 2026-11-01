//! An encoder that keeps failing (the heal policy's table): three rebuilds in ten minutes, then one restart one preset
//! lighter (`PipeCmd::Lower`), then the source stops for the take.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Harness, Src, cfg};
use klipolo_capture::backend::PresetId;
use klipolo_capture::pipe::PipeEvent;
use klipolo_capture::run::core::{Effect, PipeCmd};
use klipolo_capture::session::SourceSpec;

fn cmds(h: &Harness, source: &str) -> Vec<PipeCmd> {
    h.effects()
        .iter()
        .filter_map(|e| match e {
            Effect::Pipe { source: s, cmd } if s == source => Some(*cmd),
            _ => None,
        })
        .filter(|c| matches!(c, PipeCmd::Restart | PipeCmd::Lower | PipeCmd::Stop))
        .collect()
}

#[test]
fn a_failing_encoder_is_rebuilt_three_times_then_restarted_one_preset_lighter() {
    let mut c = cfg(&[Src::Screen, Src::Mic], 3);
    if let SourceSpec::Screen(v) = &mut c.sources[0].spec {
        v.preset = PresetId::Smooth;
    }
    let mut h = Harness::recording_cfg(c);
    for _ in 0..4 {
        h.feed_for(5.0);
        h.pipe_event("screen", PipeEvent::EncoderFailed { code: -12_903 });
    }
    h.feed_for(5.0);
    assert_eq!(
        cmds(&h, "screen"),
        [
            PipeCmd::Restart,
            PipeCmd::Restart,
            PipeCmd::Restart,
            PipeCmd::Lower
        ],
        "the fourth failure restarts at Light, not at Smooth again"
    );
    assert_eq!(h.run_count("screen"), 5, "each restart is a new run");
    h.pipe_event("screen", PipeEvent::EncoderFailed { code: -12_903 });
    assert_eq!(
        cmds(&h, "screen").last(),
        Some(&PipeCmd::Stop),
        "then it stops"
    );
}
