//! A Sync again's removal alarm measured where the sound is heard: inside the
//! recording's pictures, with the drift's own slack.

use klipolo_model::apply::apply_ops;
use klipolo_model::ops::{Op, OpRisk, Origin, SourcePatch, Transaction};
use klipolo_model::project::{AssetKind, SourceRun};
use klipolo_model::risk::Removals;
use klipolo_model::Project;

use super::common::{self, secs};
use super::{file, run, synced, SURE};

/// Chat's batch re-timing r9's mic to `runs`, reviewed against `p`.
fn review(p: &Project, runs: Vec<SourceRun>) -> (OpRisk, Removals) {
    let tx = Transaction {
        ops: vec![Op::UpdateRecordingSource {
            recording_id: "r9".into(),
            source_id: "mic".into(),
            patch: SourcePatch {
                runs: Some(runs),
                synced: SURE,
                ..SourcePatch::default()
            },
        }],
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    let review = tx.review(p).unwrap();
    (review.risk, review.removals)
}

#[test]
fn a_re_fit_of_the_drift_alone_takes_no_sound_away() {
    // A recorder lined up by marks at drift 0, 20 minutes inside an hour of pictures, then synced again at 1,000 ppm
    // (a 48.048 kHz clock, inside the search): it covers 1.2 s less of the clock, but every recorded second still plays.
    let mut p = common::imported();
    p.assets.extend([
        file("cam60", AssetKind::Video, 3600.0),
        file("mic20", AssetKind::Audio, 1200.0),
    ]);
    let at = |ppm: f64| SourceRun {
        offset_us: secs(60.0),
        ..run("mic20", 0, secs(1200.0), ppm)
    };
    let mut r = synced();
    r.sources[0].runs = vec![run("cam60", 0, secs(3600.0), 0.0)];
    r.sources[1].runs = vec![at(0.0)];
    let p = apply_ops(&p, &[Op::AddRecording { recording: r }]).unwrap();
    let (risk, removals) = review(&p, vec![at(1000.0)]);
    assert_eq!(removals, Removals::default());
    assert_ne!(risk, OpRisk::Removal);
}

#[test]
fn a_run_slid_past_the_pictures_end_takes_sound_away() {
    // The recorder lined up 30 s later keeps its clock length, but its last 30 s now play past the camera's end and the
    // first 30 s of pictures have no recorded sound: review lists the recording, and chat's batch is proposed.
    let p = apply_ops(
        &common::imported(),
        &[Op::AddRecording {
            recording: synced(),
        }],
    )
    .unwrap();
    let later = SourceRun {
        offset_us: secs(30.0),
        ..run("mic", secs(90.0), secs(610.0), 50.0)
    };
    let (risk, removals) = review(&p, vec![later]);
    assert_eq!(
        (risk, removals.sounds.as_slice()),
        (OpRisk::Removal, ["r9".to_owned()].as_slice())
    );
}
