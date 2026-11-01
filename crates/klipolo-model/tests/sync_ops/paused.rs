//! A recorder paused inside one file: Sync makes two runs of that file, the second starting inside it where
//! the first ends, on the clock 7 s later. They validate and reopen as they were, the skipped stretch plays nothing,
//! and captions follow the mic through both.

use klipolo_model::apply::apply_ops;
use klipolo_model::ops::Op;
use klipolo_model::project::SourceRun;
use klipolo_model::query::clip_starts;
use klipolo_model::speech::{run_spans, voice_runs};
use klipolo_model::time::Us;
use klipolo_model::{load_project, Project};

use super::common::{self, secs};
use super::{run, synced};

/// The imported interview synced with its mic paused at 290 s of its own file (200 s on the clock) for 7 s: the mic's
/// first run plays 90–290 s of the file from 0:00, its second the rest (290–700 s) from 207 s; c1 and c2 converted.
fn paused() -> Project {
    let mut r = synced();
    r.sources[1].runs = vec![
        run("mic", secs(90.0), secs(200.0), 50.0),
        SourceRun {
            offset_us: secs(207.0),
            ..run("mic", secs(290.0), secs(410.0), 50.0)
        },
    ];
    let ops = [
        Op::AddRecording { recording: r },
        Op::UseRecording {
            clip_id: "c1".into(),
            recording_id: "r9".into(),
        },
        Op::UseRecording {
            clip_id: "c2".into(),
            recording_id: "r9".into(),
        },
    ];
    apply_ops(&common::imported(), &ops).expect("two runs of one file after a pause validate")
}

#[test]
fn two_runs_of_one_file_after_a_pause_validate_and_round_trip() {
    let p = paused();
    let json = serde_json::to_string(&p).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let runs = &value["recordings"][0]["sources"][1]["runs"];
    assert_eq!(
        (&runs[0]["fileInUs"], &runs[1]["fileInUs"]),
        (&secs(90.0).into(), &secs(290.0).into())
    );
    assert_eq!(load_project(&json).expect("it reopens"), p);
    let mic = p.recording("r9").unwrap().source("mic").unwrap();
    assert_eq!(
        mic.run_at(secs(203.0)),
        None,
        "the skipped stretch is silent"
    );
    let (second, local) = mic
        .run_at(secs(210.0))
        .expect("the part after the pause plays");
    assert_eq!(
        (second.file_in_us, local),
        (
            secs(290.0),
            secs(290.0) + (3e6 * (1.0 + 50.0 * 1e-6_f64)).round() as Us
        )
    );
}

#[test]
fn captions_map_both_runs_of_one_file() {
    // c2 (the recording's 150–400 s at 1.5×) plays the mic across its pause: one voice run before it, one after, each
    // from its own place in the file. A word at file time 250 s is in the first (recording 160 / (1 + 50 ppm) s), one
    // at 300 s in the second (207 + 10 / (1 + 50 ppm) s); each plays where c2 shows that recording time.
    let p = paused();
    let rate = 1.0 + 50.0 * 1e-6;
    let runs = voice_runs(&p, common::clip(&p, "c2"));
    let got: Vec<(&str, Us, Us, Us)> = runs
        .iter()
        .map(|r| (r.asset.as_str(), r.rec_from, r.rec_to, r.file_in_us))
        .collect();
    let first_end = (secs(200.0) as f64 / rate).round() as Us;
    assert_eq!(
        got,
        [
            ("mic", secs(150.0), first_end, secs(90.0)),
            ("mic", secs(207.0), secs(400.0), secs(290.0))
        ]
    );
    let start = clip_starts(&p.timeline.main)[1];
    let spans: Vec<_> = run_spans(&p)
        .into_iter()
        .filter(|s| s.clip == "c2")
        .collect();
    assert_eq!(spans.len(), 2, "{spans:?}");
    for (span, word, rec) in [
        (&spans[0], secs(250.0), 160.0 / rate),
        (&spans[1], secs(300.0), 207.0 + 10.0 / rate),
    ] {
        let want = start + ((secs(rec) - secs(150.0)) as f64 / 1.5).round() as Us;
        assert!(
            (span.at(word) - want).abs() <= 1,
            "{} vs {want}",
            span.at(word)
        );
        assert!((span.local_of(want) - word).abs() <= 2);
    }
}
