//! What a synced source may hold: two mics but one camera or screen, a run inside its file and no further, two runs of
//! one file apart in it, re-timed runs that keep their files, a recorder's file added to a take, a drift up to 1,500 ppm.

use klipolo_model::apply::apply_ops;
use klipolo_model::coded::CodedError;
use klipolo_model::ops::{op_risk, Op, OpRisk, Origin, SourcePatch, Transaction};
use klipolo_model::project::{
    AssetKind, Recording, SourceRole, SourceRun, SyncLevel, Synced, SyncedBy,
};
use klipolo_model::risk::Removals;
use klipolo_model::timeline::ItemContent;
use klipolo_model::{ModelError, Project};

use super::common::{self, secs};
use super::{file, run, source, synced, SURE};

fn add(recording: Recording) -> Op {
    Op::AddRecording { recording }
}

fn with_files(files: &[(&str, AssetKind, f64)]) -> Project {
    let mut p = common::imported();
    p.assets
        .extend(files.iter().map(|&(id, kind, s)| file(id, kind, s)));
    p
}

fn one_only(role: &str) -> ModelError {
    ModelError::Invalid(
        CodedError::new("model.recording.duplicateRole").with("role", CodedError::new(role)),
    )
}

fn retime(runs: Vec<SourceRun>, synced: Option<Synced>) -> Op {
    Op::UpdateRecordingSource {
        recording_id: "r9".into(),
        source_id: "mic".into(),
        patch: SourcePatch {
            runs: Some(runs),
            synced,
            nudge_us: Some(0),
            ..SourcePatch::default()
        },
    }
}

#[test]
fn a_second_mic_validates_but_a_second_camera_or_screen_doesnt() {
    let p = with_files(&[
        ("lav2", AssetKind::Audio, 300.0),
        ("cam2", AssetKind::Video, 100.0),
    ]);
    let mut two_mics = synced();
    two_mics
        .sources
        .push(source("lav2", SourceRole::Mic, "lav2", 300.0));
    apply_ops(&p, &[add(two_mics)]).unwrap();
    let mut two_cameras = synced();
    two_cameras
        .sources
        .push(source("cam2", SourceRole::Camera, "cam2", 100.0));
    assert_eq!(
        apply_ops(&p, &[add(two_cameras)]).unwrap_err(),
        one_only("model.role.camera")
    );
    let mut two_screens = synced();
    two_screens.kind = klipolo_model::project::RecordingKind::Screen;
    two_screens.sources[0].role = SourceRole::Screen;
    two_screens
        .sources
        .push(source("cam2", SourceRole::Screen, "cam2", 100.0));
    assert_eq!(
        apply_ops(&p, &[add(two_screens)]).unwrap_err(),
        one_only("model.role.screen")
    );
    let mut same_id = synced();
    same_id
        .sources
        .push(source("mic", SourceRole::Mic, "lav2", 300.0));
    let duplicate = CodedError::new("model.project.duplicateId").with("id", "mic");
    assert_eq!(
        apply_ops(&p, &[add(same_id)]).unwrap_err(),
        ModelError::Invalid(duplicate)
    );
}

#[test]
fn a_run_inside_its_file_plays_from_there_and_must_fit_its_file() {
    let r = synced();
    let mic = r.source("mic").unwrap();
    let run0 = &mic.runs[0];
    assert_eq!(run0.local_us(0, 0), secs(90.0));
    assert_eq!(mic.run_at(0).map(|(_, local)| local), Some(secs(90.0)));
    for t in [0, 1, 123_456_789, secs(600.0)] {
        assert_eq!(run0.rec_of(run0.local_us(t, 0), 0), t);
        assert_eq!(
            run0.rec_of(run0.local_us(t, 40_000), 40_000),
            t,
            "with a nudge"
        );
    }
    let mut past = synced();
    past.sources[1].runs[0] = run("mic", secs(650.0), secs(100.0), 50.0);
    let refused = common::refused("That stretch is past the end of its file.");
    assert_eq!(
        apply_ops(&common::imported(), &[add(past)]).unwrap_err(),
        refused
    );
}

#[test]
fn two_runs_of_one_file_may_share_it_but_never_overlap_in_it() {
    // A paused recorder: the file's parts before and after the pause, at their own places on the clock.
    let paused = |second_in: f64| {
        let mut r = synced();
        r.sources[1].runs = vec![
            run("mic", secs(90.0), secs(200.0), 50.0),
            SourceRun {
                offset_us: secs(250.0),
                ..run("mic", secs(second_in), secs(300.0), 50.0)
            },
        ];
        r
    };
    apply_ops(&common::imported(), &[add(paused(300.0))]).unwrap();
    let refused = common::refused("A source's recorded parts overlap.");
    assert_eq!(
        apply_ops(&common::imported(), &[add(paused(250.0))]).unwrap_err(),
        refused
    );
}

#[test]
fn a_retime_resets_the_nudge_keeps_the_files_and_moves_a_separated_layer() {
    // The hand-placed layer of the mic file is folded in by the same Sync, so nothing separate is left.
    let mut ops = vec![
        add(synced()),
        Op::UseRecording {
            clip_id: "c1".into(),
            recording_id: "r9".into(),
        },
        Op::RemoveItem { id: "i9".into() },
    ];
    let p = apply_ops(&common::imported(), &ops).unwrap();
    let nudge = Op::UpdateRecordingSource {
        recording_id: "r9".into(),
        source_id: "mic".into(),
        patch: SourcePatch {
            nudge_us: Some(120_000),
            ..SourcePatch::default()
        },
    };
    assert_eq!(
        op_risk(&nudge),
        OpRisk::Removal,
        "a nudge alone may cut a separated layer back"
    );
    let nudged = apply_ops(&p, &[nudge]).unwrap();
    let hand = Some(Synced {
        by: SyncedBy::Hand,
        level: SyncLevel::Check,
    });
    let runs = vec![run("mic", secs(90.5), secs(609.0), 48.0)];
    assert_eq!(op_risk(&retime(runs.clone(), hand)), OpRisk::Additive);
    let tx = Transaction {
        ops: vec![retime(runs.clone(), hand)],
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    let review = tx.review(&nudged).unwrap();
    assert_eq!(
        (review.risk, &review.removals),
        (OpRisk::Additive, &Removals::default())
    );
    let mic = review.after.recording("r9").unwrap().source("mic").unwrap();
    assert_eq!((mic.nudge_us, mic.synced, &mic.runs), (0, hand, &runs));

    let keeps = common::refused("Syncing again keeps the same files.");
    let other = vec![run("cam", 0, secs(100.0), 0.0)];
    assert_eq!(
        apply_ops(&nudged, &[retime(other, hand)]).unwrap_err(),
        keeps
    );
    let paused = vec![
        run("mic", secs(90.0), secs(200.0), 50.0),
        SourceRun {
            offset_us: secs(250.0),
            ..run("mic", secs(300.0), secs(300.0), 50.0)
        },
    ];
    apply_ops(&nudged, &[retime(paused, SURE)])
        .expect("a file split in two at a pause keeps its file");

    // A mic of two files can't leave one out.
    let mut two = synced();
    two.sources[1].runs = vec![
        run("mic", secs(90.0), secs(200.0), 50.0),
        SourceRun {
            offset_us: secs(300.0),
            ..run("zoom2", 0, secs(200.0), 50.0)
        },
    ];
    ops[0] = add(two);
    let mut p2 = common::imported();
    p2.assets.push(file("zoom2", AssetKind::Audio, 200.0));
    let p2 = apply_ops(&p2, &ops).unwrap();
    let first_only = vec![run("mic", secs(90.0), secs(200.0), 50.0)];
    assert_eq!(
        apply_ops(&p2, &[retime(first_only, SURE)]).unwrap_err(),
        keeps
    );

    let detach = Op::DetachSource {
        clip_id: "c1".into(),
        source_id: "mic".into(),
    };
    let separated = apply_ops(&nudged, &[detach]).unwrap();
    // The separated mic plays its file from where the run is at c1's in-point: 90 s in, plus the 9.88 s after its
    // nudge at 50 ppm.
    let layer = separated
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == "c1-mic")
        .unwrap();
    let ItemContent::Audio { source_in_us, .. } = layer.content else {
        panic!("{layer:?}")
    };
    assert_eq!(source_in_us, 90_000_000 + 9_880_494);
    // A re-time moves the separated layer with the mic: at 0:00 c1 plays the recording's 10 s,
    // which the new run plays at 10 s × (1 + 48 ppm) + 90.5 s, at its drift.
    let retimed = apply_ops(&separated, &[retime(runs, hand)]).unwrap();
    let layer = retimed
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == "c1-mic")
        .unwrap();
    let ItemContent::Audio {
        source_in_us,
        drift_ppm,
        ..
    } = layer.content
    else {
        panic!("{layer:?}")
    };
    assert_eq!((source_in_us, drift_ppm), (100_500_480, 48.0));
}

#[test]
fn add_recording_source_adds_a_mic_to_a_take() {
    // A recorder's file joins a take Klipolo recorded, placed against the take's clock: 5 s into its file at the zero.
    let p = common::fixture();
    let zoom = |id: &str, role| klipolo_model::project::RecordingSource {
        runs: vec![SourceRun {
            drift_ppm: -30.0,
            ..run("music", secs(5.0), secs(50.0), 0.0)
        }],
        ..source(id, role, "music", 50.0)
    };
    let op = |id: &str, role| Op::AddRecordingSource {
        recording_id: "rec".into(),
        source: zoom(id, role),
    };
    assert_eq!(op_risk(&op("zoom", SourceRole::Mic)), OpRisk::Additive);
    let q = apply_ops(&p, &[op("zoom", SourceRole::Mic)]).unwrap();
    let rec = q.recording("rec").unwrap();
    assert_eq!(
        (rec.sources.len(), rec.source("zoom").unwrap().synced),
        (4, SURE)
    );
    let tx = Transaction {
        ops: vec![op("zoom", SourceRole::Mic)],
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    assert_eq!(tx.review(&p).unwrap().removals, Removals::default());
    assert_eq!(
        apply_ops(&p, &[op("cam2", SourceRole::Camera)]).unwrap_err(),
        one_only("model.role.camera")
    );
    let missing = ModelError::NotFound {
        kind: "recording",
        id: "nope".into(),
    };
    let nowhere = Op::AddRecordingSource {
        recording_id: "nope".into(),
        source: zoom("zoom", SourceRole::Mic),
    };
    assert_eq!(apply_ops(&p, &[nowhere]).unwrap_err(), missing);
}

#[test]
fn a_sync_again_that_lines_up_less_of_the_sound_is_a_removal() {
    // Re-timed runs that cover more than a second less of the recording take recorded sound away
    // there, so review lists the recording and chat's batch is proposed; a re-placement's rounding doesn't count.
    let ops = vec![
        add(synced()),
        Op::UseRecording {
            clip_id: "c1".into(),
            recording_id: "r9".into(),
        },
        Op::RemoveItem { id: "i9".into() },
    ];
    let p = apply_ops(&common::imported(), &ops).unwrap();
    let chat = |runs: Vec<SourceRun>| Transaction {
        ops: vec![retime(runs, SURE)],
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    let shorter = chat(vec![run("mic", secs(90.0), secs(500.0), 50.0)]);
    let review = shorter.review(&p).unwrap();
    assert_eq!(
        (review.risk, review.removals.sounds.as_slice()),
        (OpRisk::Removal, ["r9".to_owned()].as_slice())
    );
    assert!(shorter.needs_review(Some(&p)));
    let moved = chat(vec![run("mic", secs(90.5), secs(609.5), 50.0)]);
    assert_eq!(moved.review(&p).unwrap().removals, Removals::default());
}
