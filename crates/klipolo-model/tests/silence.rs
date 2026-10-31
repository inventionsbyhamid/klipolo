mod common;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use common::secs;
use klipolo_model::apply_ops;
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::speech::silence::{pauses, threshold, timeline_pauses};
use klipolo_model::speech::{Levels, Speech};
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::{Clip, ClipSource};

/// Levels from (seconds, dBFS) runs, ten windows a second.
fn levels(runs: &[(f64, f32)]) -> Levels {
    let db: Vec<f32> = runs
        .iter()
        .flat_map(|&(s, db)| std::iter::repeat_n(db, (s * 10.0).round() as usize))
        .collect();
    Levels { db: db.into() }
}

fn r(a: f64, b: f64) -> RangeUs {
    RangeUs {
        start: secs(a),
        end: secs(b),
    }
}

#[test]
fn pauses_come_from_quiet_runs_and_keep_a_breath() {
    let l = levels(&[
        (3.0, -20.0),
        (1.0, -65.0),
        (2.0, -20.0),
        (0.5, -65.0),
        (1.0, -20.0),
    ]);
    assert_eq!(
        pauses(&l, secs(0.7)),
        [r(3.15, 3.85)],
        "the 0.5 s gap is shorter than asked"
    );
    assert_eq!(pauses(&l, secs(0.4)), [r(3.15, 3.85), r(6.15, 6.35)]);
}

#[test]
fn the_threshold_adapts_to_the_microphone() {
    // Quiet for a sixth of the time, so the 10th percentile is the quiet level.
    let quiet_mic = levels(&[(5.0, -40.0), (2.0, -70.0), (5.0, -40.0)]);
    assert_eq!(
        threshold(&quiet_mic),
        Some(-55.0),
        "the middle of -70 and -40, kept within -55…-30"
    );
    assert_eq!(pauses(&quiet_mic, secs(0.7)), [r(5.15, 6.85)]);
    let noisy_room = levels(&[(5.0, -15.0), (2.0, -45.0), (5.0, -15.0)]);
    assert_eq!(threshold(&noisy_room), Some(-30.0));
    assert_eq!(
        pauses(&noisy_room, secs(0.7)),
        [r(5.15, 6.85)],
        "room noise under speech is still a pause"
    );
}

#[test]
fn a_silent_source_has_no_pauses_to_remove() {
    let l = levels(&[(60.0, -62.0)]);
    assert_eq!(threshold(&l), None);
    assert!(pauses(&l, secs(0.7)).is_empty());
}

#[test]
fn timeline_pauses_follow_the_clips_and_merge() {
    let mut p = Project::new("Pauses");
    p.assets = vec![Asset {
        id: "v".into(),
        kind: AssetKind::Video,
        path: "/media/v".into(),
        name: "v".into(),
        duration_us: secs(10.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }];
    let mut c1 = Clip::new(ClipSource::Asset { id: "v".into() }, 0, secs(5.0));
    c1.id = "c1".into();
    let mut c2 = Clip::new(ClipSource::Asset { id: "v".into() }, secs(5.0), secs(10.0));
    c2.id = "c2".into();
    c2.speed = 2.0;
    p.timeline.main = vec![c1, c2];
    let p = apply_ops(&p, &[]).unwrap();
    let l = levels(&[
        (3.0, -20.0),
        (1.0, -65.0),
        (3.0, -20.0),
        (1.5, -65.0),
        (1.5, -20.0),
    ]);
    let speech = Speech {
        levels: HashMap::from([("v".to_owned(), Arc::new(l))]),
        ..Speech::default()
    };
    // Source pauses 3.15–3.85 (c1) and 7.15–8.35 (c2 from 5 s at 2×: 5 + 1.075 … 5 + 1.675).
    assert_eq!(
        timeline_pauses(&p, &speech, secs(0.7), None),
        [r(3.15, 3.85), r(6.075, 6.675)]
    );
    let only_c2 = HashSet::from(["c2"]);
    assert_eq!(
        timeline_pauses(&p, &speech, secs(0.7), Some(&only_c2)),
        [r(6.075, 6.675)]
    );
    let none = Speech::default();
    assert!(
        timeline_pauses(&p, &none, secs(0.7), None).is_empty(),
        "no levels yet: nothing"
    );
    // Levels are in the file's sound time. With the sound 40 ms after the picture, each pause is 40 ms
    // later in the file: 40 ms on c1's timeline, 20 ms on c2's at 2×.
    let mut led = p.clone();
    led.assets[0].audio_lead_us = Some(40_000);
    assert_eq!(
        timeline_pauses(&led, &speech, secs(0.7), None),
        [r(3.19, 3.89), r(6.095, 6.695)]
    );
}
