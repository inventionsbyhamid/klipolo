//! `useRecording`: the video's clips move onto the synced recording with every frame, layer, place and transition kept,
//! so review finds nothing removed; the camera's own sound can be mixed per clip; captions follow the synced mic.

use klipolo_model::apply::apply_ops;
use klipolo_model::ops::{Op, OpRisk, Origin, Transaction};
use klipolo_model::project::{AssetKind, SourceMix};
use klipolo_model::query::{clip_starts, run_in_clip};
use klipolo_model::risk::Removals;
use klipolo_model::speech::{run_spans, voice_runs};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Attach, Clip, ClipSource};
use klipolo_model::{ModelError, Project};

use super::common::{self, secs};
use super::{file, run, synced};

fn sync_ops() -> Vec<Op> {
    vec![
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
    ]
}

fn use_recording(clip: &str) -> Op {
    Op::UseRecording {
        clip_id: clip.into(),
        recording_id: "r9".into(),
    }
}

fn recording(id: &str) -> ClipSource {
    ClipSource::Recording { id: id.into() }
}

fn not_this_file() -> ModelError {
    common::refused("That clip's file isn't part of this recording.")
}

#[test]
fn use_recording_keeps_every_frame_and_every_attached_layer_where_it_was() {
    let p = common::imported();
    let starts = clip_starts(&p.timeline.main);
    let q = apply_ops(&p, &sync_ops()).unwrap();
    assert_eq!(
        clip_starts(&q.timeline.main),
        starts,
        "nothing moves on the timeline"
    );
    let cam = q.recording("r9").unwrap().source("cam").unwrap();
    for id in ["c1", "c2"] {
        let (before, after) = (common::clip(&p, id), common::clip(&q, id));
        assert_eq!(
            after.source_in_us, before.source_in_us,
            "the clock is the camera's: times are kept"
        );
        let span = run_in_clip(after, 0, cam, &cam.runs[0]).unwrap();
        assert_eq!(
            span.run_in_us, before.source_in_us,
            "the same frame of the same file"
        );
        assert_eq!(
            (after.speed, &after.transition_out),
            (before.speed, &before.transition_out)
        );
    }
    let title = common::title_on(&q, "c2");
    assert_eq!(title.attach.as_ref().unwrap().source, recording("r9"));
    assert_eq!(common::title_start(&q, "c2"), common::title_start(&p, "c2"));
}

#[test]
fn use_recording_moves_only_this_clip_s_anchors() {
    // c9 stays a clip of the file, so its title keeps its anchor. Of two layers whose clip is gone, the one inside
    // c1's stretch of the file moves with c1; the one inside c9's stays.
    let mut p = common::imported();
    let orphan = |id: &str, at: f64| {
        let mut t = common::text_item(id, 0.0, 2.0);
        t.attach = Some(Attach {
            clip_id: "gone".into(),
            source: ClipSource::Asset { id: "cam".into() },
            source_us: secs(at),
        });
        t.start_us =
            klipolo_model::query::resolve_attach(&p.timeline.main, t.attach.as_ref().unwrap())
                .unwrap();
        t
    };
    let (in_c1, in_c9) = (orphan("o1", 50.0), orphan("o9", 480.0));
    p.timeline
        .tracks
        .push(common::overlay_track("orphans", vec![in_c1, in_c9]));
    let q = apply_ops(&p, &sync_ops()).unwrap();
    let anchor = |p: &Project, id: &str| {
        let item = p
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.items)
            .find(|i| i.id == id)
            .unwrap();
        (item.attach.clone().unwrap(), item.start_us)
    };
    let t9 = common::title_on(&q, "c9").attach.clone().unwrap();
    assert_eq!(
        (t9.source, t9.source_us),
        (ClipSource::Asset { id: "cam".into() }, secs(470.0))
    );
    assert_eq!(common::title_start(&q, "c9"), common::title_start(&p, "c9"));
    let (o1, at1) = anchor(&q, "o1");
    assert_eq!(
        (o1.source, o1.source_us, at1),
        (recording("r9"), secs(50.0), anchor(&p, "o1").1)
    );
    assert_eq!(anchor(&q, "o9"), anchor(&p, "o9"));
}

#[test]
fn use_recording_maps_a_second_camera_file_by_its_run() {
    // A second picture file, cam2, runs from 700 s on the clock: its clip's source times become 700 s + file time.
    let mut p = common::imported();
    p.assets.push(file("cam2", AssetKind::Video, 100.0));
    let mut c3 = Clip::new(
        ClipSource::Asset { id: "cam2".into() },
        secs(20.0),
        secs(60.0),
    );
    c3.id = "c3".into();
    p.timeline.main.push(c3);
    let mut r = synced();
    r.sources[0].runs.push(klipolo_model::project::SourceRun {
        offset_us: secs(700.0),
        ..run("cam2", 0, secs(100.0), 0.0)
    });
    let ops = [Op::AddRecording { recording: r }, use_recording("c3")];
    let q = apply_ops(&p, &ops).unwrap();
    let c3 = common::clip(&q, "c3");
    assert_eq!(
        (c3.source_in_us, c3.source_out_us),
        (secs(720.0), secs(760.0))
    );
    assert_eq!(clip_starts(&q.timeline.main), clip_starts(&p.timeline.main));
    let cam = q.recording("r9").unwrap().source("cam").unwrap();
    assert_eq!(
        run_in_clip(c3, 0, cam, &cam.runs[1]).unwrap().run_in_us,
        secs(20.0)
    );
    let tx = Transaction {
        ops: ops.to_vec(),
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    assert_eq!(tx.review(&p).unwrap().removals, Removals::default());
}

#[test]
fn use_recording_refuses_a_file_the_recording_doesnt_hold() {
    let mut p = common::imported();
    p.assets.push(file("broll", AssetKind::Video, 30.0));
    let mut other = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(10.0));
    other.id = "c4".into();
    p.timeline.main.push(other);
    let add = Op::AddRecording {
        recording: synced(),
    };
    assert_eq!(
        apply_ops(&p, &[add.clone(), use_recording("c4")]).unwrap_err(),
        not_this_file()
    );
    let twice = [add, use_recording("c1"), use_recording("c1")];
    assert_eq!(
        apply_ops(&p, &twice).unwrap_err(),
        not_this_file(),
        "c1 already plays the recording"
    );
}

#[test]
fn sync_is_additive_until_it_removes_a_layer() {
    // No lost footage, no pieces, no transition lost. Removing the hand-placed mic layer lists it alone.
    let p = common::imported();
    let tx = Transaction {
        label: "edit.lineUpSound".into(),
        ops: sync_ops(),
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    let review = tx.review(&p).unwrap();
    assert_eq!(review.removals, Removals::default());
    assert_eq!(review.risk, OpRisk::Additive);
    assert!(!tx.needs_review(Some(&p)));
    let mut ops = sync_ops();
    ops.push(Op::RemoveItem { id: "i9".into() });
    let review = Transaction { ops, ..tx }.review(&p).unwrap();
    let only_i9 = Removals {
        items: vec!["i9".into()],
        ..Removals::default()
    };
    assert_eq!(review.removals, only_i9);
}

#[test]
fn a_clip_can_set_the_camera_sound_s_mix_when_its_file_has_sound() {
    // The imported camera's file has sound, so its clip may mix it; a take's camera file has none.
    let p = apply_ops(&common::imported(), &sync_ops()).unwrap();
    let mix = SourceMix {
        muted: false,
        volume: 0.5,
        ..SourceMix::default()
    };
    let unmute = |clip: &str, source: &str| Op::SetClipSourceMix {
        clip_id: clip.into(),
        source_id: source.into(),
        mix: Some(mix.clone()),
    };
    let q = apply_ops(&p, &[unmute("c1", "cam")]).unwrap();
    assert_eq!(common::clip(&q, "c1").source_mix["cam"], mix);
    let refused = common::refused("Only a recording's own sound sources can be mixed per clip.");
    assert_eq!(
        apply_ops(&common::fixture(), &[unmute("a", "camera")]).unwrap_err(),
        refused
    );
}

#[test]
fn captions_follow_a_mic_that_starts_inside_its_file() {
    // c1's voice is the mic now; a word at mic file time 90 s + t(1 + 50 ppm) is recording time t, which c1
    // (in-point 10 s, at 0 on the timeline) plays at t − 10 s.
    let q = apply_ops(&common::imported(), &sync_ops()).unwrap();
    let runs = voice_runs(&q, common::clip(&q, "c1"));
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0].asset.as_str(), runs[0].file_in_us),
        ("mic", secs(90.0))
    );
    let t = secs(20.0);
    let word = runs[0].local_us(t);
    assert_eq!(word, secs(90.0) + (t as f64 * 1.000_05).round() as Us);
    let span = run_spans(&q).into_iter().find(|s| s.clip == "c1").unwrap();
    assert!((span.at(word) - secs(10.0)).abs() <= 1, "{}", span.at(word));
    assert!((span.local_of(secs(10.0)) - word).abs() <= 1);
}
