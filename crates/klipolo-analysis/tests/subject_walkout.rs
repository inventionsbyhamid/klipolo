//! Whoever speaks, on a real tracker answer: SAM's boxes for NASA's Crew-1 walkout (16 s at 10 frames a second, four
//! astronauts abreast), kept to the two at either end, with a made-up talk between them. The windows asked about, what
//! they cost at Muse Spark's price, and the vertical crop's cuts to whoever speaks. Nothing here reaches Meta.

use std::collections::BTreeMap;

use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::providers::{Heard, TrackedBox, TrackedFrame, VoiceMatcher};
use klipolo_analysis::subject::path::{PathInput, focus_path, people};
use klipolo_analysis::subject::voices::{self, VoiceTurn};
use klipolo_meta::{MetaClient, SecretString, pricing};
use klipolo_model::fill_focus::{Keep, Voice};
use klipolo_model::time::{RangeUs, Us};

const BOXES: &str =
    include_str!("../../klipolo-sam/tests/fixtures/person_concept_one_bit.boxes.json");
const PORTRAIT: (f64, f64) = ((1080.0 / 1920.0) / (1920.0 / 1080.0), 1.0);

fn s(seconds: f64) -> Us {
    (seconds * 1e6).round() as Us
}

/// The walkout's frames with only the astronauts on the far left (SAM's 3) and far right (its 0), at 10 a second.
fn two_astronauts() -> Vec<(Us, TrackedFrame)> {
    let rows: Vec<(u32, u32, f64, f64, f64, f64, f32)> = serde_json::from_str(BOXES).unwrap();
    let mut frames: BTreeMap<u32, TrackedFrame> = BTreeMap::new();
    for (frame, id, l, t, r, b, c) in rows.into_iter().filter(|r| r.1 == 0 || r.1 == 3) {
        let f = frames.entry(frame).or_insert_with(|| TrackedFrame {
            frame,
            boxes: Vec::new(),
        });
        f.boxes.push(TrackedBox {
            id,
            left: l / 854.0,
            top: t / 480.0,
            right: r / 854.0,
            bottom: b / 480.0,
            confidence: Some(c),
        });
    }
    (frames.into_values())
        .map(|f| ((f64::from(f.frame) / 10.0 * 1e6) as Us, f))
        .collect()
}

fn turn(from: f64, to: f64, label: &str) -> VoiceTurn {
    VoiceTurn {
        start_us: s(from),
        end_us: s(to),
        voice: Voice {
            part: 0,
            label: label.into(),
        },
    }
}

#[test]
fn a_real_two_person_walk_keeps_whoever_speaks() {
    let frames = two_astronauts();
    let timed: Vec<(Us, &TrackedFrame)> = frames.iter().map(|(t, f)| (*t, f)).collect();
    let plain: Vec<TrackedFrame> = frames.iter().map(|(_, f)| f.clone()).collect();
    assert_eq!(people(&plain), [3, 0], "left to right");

    // A on the left talks, B on the right says "mm-hm" at 3 s, answers from 6 s, and A picks up at 12 s.
    let turns = [
        turn(0.0, 6.0, "A"),
        turn(3.0, 3.6, "B"),
        turn(6.0, 12.0, "B"),
        turn(12.0, 16.0, "A"),
    ];
    let clip = RangeUs {
        start: 0,
        end: s(16.0),
    };
    let windows = voices::windows(&timed, &[], clip, &turns, PORTRAIT.0);
    let meta = Meta::new(
        MetaClient::direct(SecretString::from("test-meta-key-not-real")).with_ledger(None),
    );
    let most = voices::most_usd(&meta, &turns, clip);
    let asked: f64 = (windows.iter())
        .map(|w| meta.estimate_usd((w.range.end - w.range.start) as f64 / 1e6))
        .sum();
    let tracking = pricing::sam_frames_usd(32);
    println!(
        "two-person walkout: {} windows {:?}, voices at most ${most:.4} (these ${asked:.4}), tracking ${tracking:.4}",
        windows.len(),
        (windows.iter())
            .map(|w| format!(
                "{} {:.1}-{:.1}",
                w.voice.label,
                w.range.start as f64 / 1e6,
                w.range.end as f64 / 1e6
            ))
            .collect::<Vec<_>>()
    );
    // A's three stretches alone and B's answer; B's "mm-hm" over A is never asked about.
    assert_eq!(windows.len(), 4, "{windows:?}");
    assert!(windows.iter().all(|w| w.people.len() == 2));
    assert!(most < 0.02 && asked <= most + 1e-12, "{most} {asked}");

    // What the vote would say: A is the one on the left, B the one on the right.
    let heard = BTreeMap::from([
        (turns[0].voice.clone(), Heard::Person { person: 1 }),
        (turns[2].voice.clone(), Heard::Person { person: 2 }),
    ]);
    let talk = voices::talk(&turns, &heard, 0);
    let input = PathInput {
        frames: &timed,
        start_us: 0,
        end_us: s(16.0),
        cuts: &[],
        crop: PORTRAIT,
        talk: &talk,
        keep: &Keep::Speaker,
    };
    let path = focus_path(&input).unwrap();
    assert!(path.is_valid());
    let cuts: Vec<f64> = (path.keys.iter())
        .filter(|k| k.cut)
        .map(|k| k.at_us as f64 / 1e6)
        .collect();
    let x = |at: f64| path.centre_at(s(at)).unwrap().0;
    println!(
        "crop: cuts at {cuts:?} s; centre at 2 s {:.2}, 4 s {:.2}, 9 s {:.2}, 14 s {:.2}; {} keys",
        x(2.0),
        x(4.0),
        x(9.0),
        x(14.0),
        path.keys.len()
    );
    assert_eq!(cuts, [6.0, 12.0]);
    assert!(x(2.0) < 0.4 && x(4.0) < 0.4 && x(14.0) < 0.4, "{path:?}");
    assert!(x(9.0) > 0.6, "{path:?}");
}
