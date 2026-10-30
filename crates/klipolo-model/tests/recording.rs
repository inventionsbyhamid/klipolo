mod common;
use common::*;
use klipolo_model::coded::CodedError;
use klipolo_model::project::*;
use klipolo_model::query::{run_in_clip, RunInClip};
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_model::validate::validate;
use klipolo_model::{load_project, ModelError};

fn run(asset: &str, from: f64, len: f64) -> SourceRun {
    SourceRun {
        asset_id: asset.into(),
        offset_us: secs(from),
        duration_us: secs(len),
        drift_ppm: 0.0,
        file_in_us: 0,
    }
}

fn source(id: &str, role: SourceRole, runs: Vec<SourceRun>) -> RecordingSource {
    RecordingSource {
        id: id.into(),
        role,
        label: id.into(),
        runs,
        nudge_us: 0,
        mix: SourceMix::default(),
        synced: None,
    }
}

#[test]
fn a_run_maps_recording_time_to_its_own_time_with_drift_and_nudge() {
    let mut mic = source(
        "mic",
        SourceRole::Mic,
        vec![run("m1", 0.0, 60.0), run("m2", 70.0, 30.0)],
    );
    mic.runs[1].drift_ppm = 100.0;
    assert_eq!(
        mic.run_at(secs(10.0))
            .map(|(r, t)| (r.asset_id.as_str(), t)),
        Some(("m1", secs(10.0)))
    );
    assert!(
        mic.run_at(secs(65.0)).is_none(),
        "a gap between runs plays nothing"
    );
    // 10 s into the second run at +100 ppm: the device clock ran 1 ms ahead.
    assert_eq!(
        mic.run_at(secs(80.0)).map(|(_, t)| t),
        Some(secs(10.0) + 1_000)
    );
    mic.nudge_us = -50_000; // the person moved the mic 50 ms earlier
    assert_eq!(mic.run_at(secs(10.0)).map(|(_, t)| t), Some(secs(10.05)));
}

#[test]
fn video_holds_its_nearest_frame_across_a_gap() {
    let cam = source(
        "cam",
        SourceRole::Camera,
        vec![run("c1", 0.0, 60.0), run("c2", 70.0, 30.0)],
    );
    assert_eq!(
        cam.run_or_nearest(secs(65.0))
            .map(|(r, t)| (r.asset_id.as_str(), t)),
        Some(("c1", secs(60.0)))
    );
    assert_eq!(
        cam.run_or_nearest(secs(69.0))
            .map(|(r, t)| (r.asset_id.as_str(), t)),
        Some(("c2", 0))
    );
}

#[test]
fn the_fixture_recording_reads_by_role() {
    let p = fixture();
    let rec = p.recording("rec").unwrap();
    assert_eq!(rec.duration_us(), secs(60.0));
    assert_eq!(rec.screen().map(|s| s.id.as_str()), Some("screen"));
    assert_eq!(rec.camera().map(|s| s.id.as_str()), Some("camera"));
    assert_eq!(
        rec.audio_sources()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["mic"]
    );
    assert_eq!(
        rec.asset_ids().cloned().collect::<Vec<_>>(),
        ["screen", "camera", "mic"]
    );
}

#[test]
fn a_run_inside_a_clip_starts_and_ends_with_its_footage() {
    // The clip plays recording time 10–40 s from timeline 5 s at 2×; the run covers recording time 20–30 s.
    let mut clip = Clip::new(
        ClipSource::Recording { id: "rec".into() },
        secs(10.0),
        secs(40.0),
    );
    clip.speed = 2.0;
    let mic = source("mic", SourceRole::Mic, vec![run("m", 20.0, 10.0)]);
    assert_eq!(
        run_in_clip(&clip, secs(5.0), &mic, &mic.runs[0]),
        Some(RunInClip {
            start_us: secs(10.0),
            end_us: secs(15.0),
            run_in_us: 0
        })
    );
    let later = source("mic", SourceRole::Mic, vec![run("m", 50.0, 10.0)]);
    assert_eq!(run_in_clip(&clip, secs(5.0), &later, &later.runs[0]), None);
}

#[test]
fn validation_refuses_broken_sources_and_markers() {
    type BreakIt = fn(&mut Recording);
    let cases: [(&str, BreakIt); 8] = [
        // A run never outlasts its file.
        ("That stretch is past the end of its file.", |r| {
            r.sources[2].runs[0].duration_us = secs(61.0)
        }),
        ("A source's recorded parts overlap.", |r| {
            r.sources[2].runs.push(SourceRun {
                asset_id: "mic".into(),
                offset_us: secs(30.0),
                duration_us: secs(10.0),
                drift_ppm: 0.0,
                file_in_us: 0,
            })
        }),
        ("Each recorded source needs its footage.", |r| {
            r.sources[2].runs.clear()
        }),
        ("Sources can be shifted by at most 5 seconds.", |r| {
            r.sources[2].nudge_us = secs(6.0)
        }),
        (
            "A recording needs a screen or camera as its main footage.",
            |r| r.primary = "mic".into(),
        ),
        ("A source's clock correction is out of range.", |r| {
            r.sources[2].runs[0].drift_ppm = 5_000.0
        }),
        ("A marker must be inside its recording.", |r| {
            r.markers.push(Marker {
                id: "m1".into(),
                at_us: secs(61.0),
                kind: MarkerKind::Hotkey,
                label: None,
            })
        }),
        // Import sorts them; `AddMarker` keeps them sorted.
        ("A recording's markers must be in time order.", |r| {
            for (id, s) in [("m1", 20.0), ("m2", 10.0)] {
                r.markers.push(Marker {
                    id: id.into(),
                    at_us: secs(s),
                    kind: MarkerKind::Hotkey,
                    label: None,
                });
            }
        }),
    ];
    for (english, break_it) in cases {
        let mut p = fixture();
        break_it(&mut p.recordings[0]);
        assert_eq!(validate(&p), Err(refused(english)), "{english}");
    }
    // A second camera is refused by name; a second mic is a second lav (`sync_ops`).
    let mut p = fixture();
    let mut extra = p.recordings[0].sources[1].clone();
    extra.id = "camera2".into();
    p.recordings[0].sources.push(extra);
    let one_camera = CodedError::new("model.recording.duplicateRole")
        .with("role", CodedError::new("model.role.camera"));
    assert_eq!(validate(&p), Err(ModelError::Invalid(one_camera)));
}

#[test]
fn a_clip_mixes_only_its_recording_s_sound_sources() {
    let mut p = fixture();
    p.timeline.main[0].source_mix.insert(
        "mic".into(),
        SourceMix {
            volume: 0.5,
            ..SourceMix::default()
        },
    );
    assert!(validate(&p).is_ok());
    p.timeline.main[0]
        .source_mix
        .insert("camera".into(), SourceMix::default());
    assert_eq!(
        validate(&p),
        Err(refused(
            "Only a recording's own sound sources can be mixed per clip."
        ))
    );
}

#[test]
fn a_format_3_file_is_refused_not_migrated() {
    let mut json = serde_json::to_value(fixture()).unwrap();
    json["version"] = 3.into();
    assert_eq!(
        load_project(&json.to_string()),
        Err(ModelError::OlderVersion)
    );
    assert_eq!(PROJECT_VERSION, 7);
}

#[test]
fn a_source_s_json_is_camel_case() {
    let json = serde_json::to_value(&fixture().recordings[0].sources[2]).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "id": "mic", "role": "mic", "label": "capture.source.mic",
            "runs": [{ "assetId": "mic", "offsetUs": 0, "durationUs": 60_000_000, "driftPpm": 0.0 }],
            "nudgeUs": 0, "mix": { "volume": 1.0, "muted": false, "effects": [] }
        })
    );
}
