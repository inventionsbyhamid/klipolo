mod common;
use common::config::config_with;
use klipolo_capture::drift::Anchor;
use klipolo_capture::english::english;
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::manifest;
use klipolo_capture::media::AudioFormat;
use klipolo_capture::session::SessionDir;
use klipolo_model::coded::CodedError;
use klipolo_model::project::SourceRole;
use std::collections::BTreeMap;

/// Drift is fitted per run, and a run whose anchors lie on two lines (an outage the run wasn't
/// split at) is placed by its first sample and marked approximate. A clean run with a few late anchors is not.
#[test]
fn drift_is_fitted_per_run_and_a_line_broken_by_an_outage_is_approximate() {
    let cfg = config_with(&[("screen", SourceRole::Screen), ("mic", SourceRole::Mic)]);
    let fmt = AudioFormat {
        rate: 48_000,
        channels: 1,
    };
    let mic_run = |run: u32, offset_us: i64| Entry::RunStarted {
        source: "mic".into(),
        run,
        file: SessionDir::run_key("mic", run, false),
        offset_us,
        video: None,
        audio: Some(fmt),
    };
    let fallback = CodedError::new("capture.health.micFallback")
        .with("device", "MV7")
        .with("fallback", "MacBook Pro Microphone");
    let entries = vec![
        Entry::Gate { host_ns: 0 },
        Entry::RunStarted {
            source: "screen".into(),
            run: 1,
            file: "screen/run-01.mp4".into(),
            offset_us: 0,
            video: None,
            audio: None,
        },
        mic_run(1, 2_000),
        Entry::health(&fallback, Some("mic".into()), true),
        Entry::health(
            &CodedError::new("capture.health.dropping"),
            Some("screen".into()),
            false,
        ),
        Entry::RunEnded {
            source: "mic".into(),
            run: 1,
            end_us: 60_002_000,
        },
        mic_run(2, 63_000_000),
        Entry::Finalized {
            duration_us: 130_000_000,
        },
    ];
    let read = journal::JournalRead {
        entries,
        torn: false,
        skipped: 0,
    };
    // Run 1: frame 0 at 1 ms, 80 ppm fast, ±1 ms of jitter, and three callbacks (5 %) 300 ms late.
    let run1: Vec<Anchor> = (0..60_i64)
        .map(|k| {
            let late = if [20, 33, 41].contains(&k) {
                300_000
            } else {
                0
            };
            Anchor {
                session_us: 1_000 + k * 1_000_000 + (k * 7_919 % 2_001 - 1_000) + late,
                frame: (48_000.0 * (1.0 + 80e-6) * k as f64).round() as u64,
            }
        })
        .collect();
    // Run 2: the device went quiet for 2 s after 30 s and the run went on (a run the session should have split).
    let run2: Vec<Anchor> = (0..60_i64)
        .map(|k| Anchor {
            session_us: 63_000_000 + k * 1_000_000 + if k >= 30 { 2_000_000 } else { 0 },
            frame: k as u64 * 48_000,
        })
        .collect();
    let anchors = BTreeMap::from([
        (("mic".to_string(), 1), run1),
        (("mic".to_string(), 2), run2),
    ]);
    let durations: BTreeMap<String, i64> = [
        ("screen/run-01.mp4", 130_000_000),
        ("mic/run-01.caf", 60_000_000),
        ("mic/run-02.caf", 66_000_000),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let m = manifest::derive(&cfg, &read, &anchors, &durations);

    let mic = &m.sources.iter().find(|s| s.id == "mic").unwrap().runs;
    assert!(!mic[0].approximate, "{:?}", mic[0]);
    assert!((mic[0].offset_us - 1_000).abs() <= 1_000, "{:?}", mic[0]);
    assert!((mic[0].drift_ppm - 80.0).abs() < 5.0, "{:?}", mic[0]);
    assert!(mic[1].approximate, "{:?}", mic[1]);
    assert_eq!((mic[1].offset_us, mic[1].drift_ppm), (63_000_000, 0.0));
    assert_eq!(m.duration_us, 130_000_000);
    let notes: Vec<String> = m.notes.iter().map(english).collect();
    assert_eq!(
        notes,
        [
            "MV7 disconnected. Recording with MacBook Pro Microphone instead.",
            "Mic stopped for 3 s at 1:00. Everything else was recorded.",
            "Some timing in this recording is approximate.",
        ]
    );
}

fn mic_only(entries: Vec<Entry>) -> journal::JournalRead {
    let mut all = vec![Entry::Gate { host_ns: 0 }];
    all.extend(entries);
    journal::JournalRead {
        entries: all,
        torn: false,
        skipped: 0,
    }
}

fn mic_run(run: u32, offset_us: i64) -> Entry {
    Entry::RunStarted {
        source: "mic".into(),
        run,
        file: SessionDir::run_key("mic", run, false),
        offset_us,
        video: None,
        audio: Some(AudioFormat {
            rate: 48_000,
            channels: 1,
        }),
    }
}

fn lengths(runs: &[(u32, i64)]) -> BTreeMap<String, i64> {
    runs.iter()
        .map(|&(run, us)| (SessionDir::run_key("mic", run, false), us))
        .collect()
}

/// The MV7 goes at 1:00, the loss is noticed and the run ended at 1:02.5, and the fallback's first
/// sample comes at 1:02.8. The hole is the 2.8 s without sound, not the 0.3 s after `RunEnded`.
#[test]
fn a_lost_devices_hole_is_measured_from_where_its_sound_ends() {
    let cfg = config_with(&[("mic", SourceRole::Mic)]);
    let read = mic_only(vec![
        mic_run(1, 0),
        Entry::RunEnded {
            source: "mic".into(),
            run: 1,
            end_us: 62_500_000,
        },
        mic_run(2, 62_800_000),
    ]);
    let durations = lengths(&[(1, 60_000_000), (2, 10_000_000)]);
    let m = manifest::derive(&cfg, &read, &BTreeMap::new(), &durations);
    let notes: Vec<String> = m.notes.iter().map(english).collect();
    assert_eq!(
        notes,
        ["Mic stopped for 2.8 s at 1:00. Everything else was recorded."]
    );
    assert_eq!(m.duration_us, 72_800_000);
}

/// A 100 ms step in the middle of a 5-minute run is two lines, not one (the old 250 ms margin took
/// it as one, leaving half the run 100 ms off); one stray anchor in a run of ten isn't a second line.
#[test]
fn a_100_ms_step_is_not_one_line_but_one_stray_anchor_is() {
    let cfg = config_with(&[("mic", SourceRole::Mic)]);
    let read = mic_only(vec![mic_run(1, 0), mic_run(2, 400_000_000)]);
    let line = |n: i64, start: i64, off: &dyn Fn(i64) -> i64| -> Vec<Anchor> {
        (0..n)
            .map(|k| Anchor {
                session_us: start + k * 1_000_000 + off(k),
                frame: k as u64 * 48_000,
            })
            .collect()
    };
    let step = |k| if k >= 150 { 100_000 } else { 0 };
    let stray = |k| if k == 5 { 300_000 } else { 0 };
    let anchors = BTreeMap::from([
        (("mic".to_string(), 1), line(300, 0, &step)),
        (("mic".to_string(), 2), line(10, 400_000_000, &stray)),
    ]);
    let durations = lengths(&[(1, 300_000_000), (2, 10_000_000)]);
    let m = manifest::derive(&cfg, &read, &anchors, &durations);
    let runs = &m.sources[0].runs;
    assert!(runs[0].approximate, "{:?}", runs[0]);
    assert!(!runs[1].approximate, "{:?}", runs[1]);
    assert_eq!(runs[1].offset_us, 400_000_000);
}
