//! The speaker's path from tracked boxes: the main person per shot, held while they stay put, followed smoothly
//! (critically damped, under a speed limit) when they move a lot, cut with the shots and never panned across one, with
//! headroom above the head.

use klipolo_analysis::providers::{TrackedBox, TrackedFrame};
use klipolo_analysis::subject::path::{
    DEAD_ZONE, FULL, KEY_STEP_US, PathInput, SPEED, focus_path, main_subject,
};
use klipolo_model::fill_focus::{FocusPath, Keep};
use klipolo_model::layout::BoxN;
use klipolo_model::time::Us;

/// A 16:9 picture filling 9:16 shows this share of its width, all of its height.
const PORTRAIT: (f64, f64) = ((1080.0 / 1920.0) / (1920.0 / 1080.0), 1.0);
const FPS: f64 = 2.0;

fn person(id: u32, cx: f64, width: f64, top: f64) -> TrackedBox {
    TrackedBox {
        id,
        left: cx - width / 2.0,
        top,
        right: cx + width / 2.0,
        bottom: 1.0,
        confidence: Some(0.9),
    }
}

/// `seconds` of frames at 2 a second, each holding what `at` gives for its time.
fn frames(seconds: f64, at: impl Fn(f64) -> Vec<TrackedBox>) -> Vec<TrackedFrame> {
    let n = (seconds * FPS).round() as u32;
    (0..n)
        .map(|frame| TrackedFrame {
            frame,
            boxes: at(f64::from(frame) / FPS),
        })
        .collect()
}

/// Each frame with its time: frame n at n / 2 s from `start_us`.
fn timed(frames: &[TrackedFrame], start_us: Us) -> Vec<(Us, &TrackedFrame)> {
    (frames.iter())
        .map(|f| (start_us + (f64::from(f.frame) / FPS * 1e6) as Us, f))
        .collect()
}

fn input<'a>(
    frames: &'a [(Us, &'a TrackedFrame)],
    cuts: &'a [Us],
    crop: (f64, f64),
) -> PathInput<'a> {
    PathInput {
        frames,
        start_us: 0,
        end_us: (frames.len() as f64 / FPS * 1e6) as Us,
        cuts,
        crop,
        talk: &[],
        keep: &Keep::Speaker,
        area: FULL,
    }
}

fn xs(path: &FocusPath) -> Vec<f64> {
    path.keys.iter().map(|k| k.x).collect()
}

/// A small, steady wobble: what a tracker's boxes do on a still speaker.
fn wobble(t: f64) -> f64 {
    0.01 * (t * 7.3).sin()
}

#[test]
fn a_still_speaker_holds_one_place() {
    let f = frames(10.0, |t| vec![person(1, 0.3 + wobble(t), 0.2, 0.1)]);
    let path = focus_path(&input(&timed(&f, 0), &[], PORTRAIT)).expect("someone is there");
    assert!(path.is_valid());
    let x = xs(&path);
    assert!(x.iter().all(|v| (v - x[0]).abs() < 1e-9), "{x:?}");
    assert!((x[0] - 0.3).abs() < 0.02, "{x:?}");
    // A hold is two keys, its start and its end: a long still talk stays small.
    assert_eq!(path.keys.len(), 2, "{:?}", path.keys);
    assert_eq!(path.keys[0].at_us, 0);
    assert!(path.keys[1].at_us >= 9_750_000 - KEY_STEP_US);
    assert!(!path.keys.iter().any(|k| k.cut));
}

#[test]
fn a_cut_jumps_to_the_next_shots_speaker_and_never_pans_across_it() {
    let f = frames(10.0, |t| {
        vec![if t < 5.0 {
            person(1, 0.3, 0.2, 0.1)
        } else {
            person(2, 0.75, 0.2, 0.1)
        }]
    });
    let cuts = [5_000_000];
    let path = focus_path(&input(&timed(&f, 0), &cuts, PORTRAIT)).unwrap();
    let at = |us: Us| path.centre_at(us).unwrap().0;
    assert!((at(4_999_999) - 0.3).abs() < 0.02, "{}", at(4_999_999));
    assert!((at(5_000_000) - 0.75).abs() < 0.02, "{}", at(5_000_000));
    let cut: Vec<Us> = path
        .keys
        .iter()
        .filter(|k| k.cut)
        .map(|k| k.at_us)
        .collect();
    assert_eq!(cut, [5_000_000]);
    // Within each shot it holds.
    for k in &path.keys {
        let want = if k.at_us < 5_000_000 { 0.3 } else { 0.75 };
        assert!((k.x - want).abs() < 0.02, "{k:?}");
    }
}

#[test]
fn a_walking_speaker_is_followed_smoothly_under_the_speed_limit() {
    // From 0.2 to 0.8 across 12 s, with the tracker's wobble on top.
    let f = frames(12.0, |t| {
        vec![person(1, 0.2 + 0.05 * t + wobble(t), 0.15, 0.1)]
    });
    let path = focus_path(&input(&timed(&f, 0), &[], PORTRAIT)).unwrap();
    let x = xs(&path);
    // It ends near the speaker, and never turns back: no jitter.
    assert!(
        (x.last().unwrap() - 0.75).abs() < PORTRAIT.0 * DEAD_ZONE,
        "{x:?}"
    );
    assert!(x.windows(2).all(|w| w[1] >= w[0] - 1e-9), "{x:?}");
    // Never faster than the limit, in crop widths a second.
    for w in path.keys.windows(2) {
        let speed = (w[1].x - w[0].x).abs() / ((w[1].at_us - w[0].at_us) as f64 / 1e6);
        assert!(speed <= SPEED * PORTRAIT.0 + 1e-6, "{speed} at {:?}", w[1]);
    }
    // The speaker is in the crop on every frame.
    for (i, frame) in f.iter().enumerate() {
        let us = (i as f64 / FPS * 1e6) as Us;
        let c = path.centre_at(us).unwrap().0;
        let b = frame.boxes[0];
        let mid = (b.left + b.right) / 2.0;
        assert!(
            (mid - c).abs() < PORTRAIT.0 / 2.0,
            "frame {i}: {mid} vs {c}"
        );
    }
}

#[test]
fn the_main_subject_is_the_largest_most_central_person() {
    // A big person on the left and a small one in the middle: the big one.
    let f = frames(4.0, |_| {
        vec![person(1, 0.5, 0.08, 0.6), person(2, 0.25, 0.3, 0.1)]
    });
    assert_eq!(main_subject(&f, FULL), Some(2));
    // Two the same size: the one nearer the middle.
    let f = frames(4.0, |_| {
        vec![person(1, 0.15, 0.2, 0.1), person(2, 0.6, 0.2, 0.1)]
    });
    assert_eq!(main_subject(&f, FULL), Some(2));
    // Nobody.
    let f = frames(4.0, |_| vec![]);
    assert_eq!(main_subject(&f, FULL), None);
}

#[test]
fn inside_a_cropped_area_the_subject_is_picked_near_its_middle_and_the_crop_stays_in_it() {
    // The right half kept: of two the same size, the one nearer its middle (0.75), not the picture's.
    let right = BoxN {
        x: 0.5,
        y: 0.0,
        w: 0.5,
        h: 1.0,
    };
    let f = frames(4.0, |_| {
        vec![person(1, 0.55, 0.1, 0.1), person(2, 0.85, 0.1, 0.1)]
    });
    assert_eq!(main_subject(&f, FULL), Some(1));
    assert_eq!(main_subject(&f, right), Some(2));
    // Someone near its inner edge: the crop goes no further left than the area's edge.
    let f = frames(4.0, |_| vec![person(1, 0.55, 0.1, 0.1)]);
    let at = timed(&f, 0);
    let mut i = input(&at, &[], PORTRAIT);
    i.area = right;
    let path = focus_path(&i).unwrap();
    for k in &path.keys {
        assert!((k.x - (0.5 + PORTRAIT.0 / 2.0)).abs() < 1e-9, "{k:?}");
    }
}

#[test]
fn two_people_side_by_side_who_fit_in_the_crop_are_framed_together() {
    // Square crop of a 16:9 picture is 0.5625 wide: two people at 0.4 and 0.6 fit.
    let square = (0.5625, 1.0);
    let f = frames(6.0, |_| {
        vec![person(1, 0.38, 0.12, 0.1), person(2, 0.62, 0.12, 0.1)]
    });
    let path = focus_path(&input(&timed(&f, 0), &[], square)).unwrap();
    assert!((path.keys[0].x - 0.5).abs() < 0.02, "{:?}", path.keys[0]);
}

#[test]
fn a_lost_speaker_holds_then_eases_back() {
    // At 0.3 for 4 s, gone for 3 s, back at 0.7.
    let f = frames(12.0, |t| match t {
        t if t < 4.0 => vec![person(1, 0.3, 0.2, 0.1)],
        t if t < 7.0 => vec![],
        _ => vec![person(1, 0.7, 0.2, 0.1)],
    });
    let path = focus_path(&input(&timed(&f, 0), &[], PORTRAIT)).unwrap();
    let at = |s: f64| path.centre_at((s * 1e6) as Us).unwrap().0;
    assert!(
        (at(6.5) - 0.3).abs() < 0.02,
        "holds while lost: {}",
        at(6.5)
    );
    // Eases (no jump) once back, and gets there.
    assert!(at(7.25) < 0.45, "{}", at(7.25));
    assert!(
        (at(11.9) - 0.7).abs() < PORTRAIT.0 * DEAD_ZONE,
        "{}",
        at(11.9)
    );
}

#[test]
fn headroom_keeps_the_head_near_the_top_of_a_short_crop() {
    // A portrait video filling 16:9 shows this share of its height.
    let landscape = (1.0, (1080.0 / 1920.0) / (1920.0 / 1080.0));
    let f = frames(4.0, |_| vec![person(1, 0.5, 0.4, 0.4)]);
    let path = focus_path(&input(&timed(&f, 0), &[], landscape)).unwrap();
    let y = path.keys[0].y;
    let top = y - landscape.1 / 2.0;
    // The head is a little below the crop's top edge: inside it, with some room above.
    assert!(top < 0.4 && 0.4 - top < landscape.1 * 0.2, "crop top {top}");
}

#[test]
fn nobody_found_gives_no_path_and_a_shot_without_anyone_holds_the_middle() {
    let f = frames(4.0, |_| vec![]);
    assert!(focus_path(&input(&timed(&f, 0), &[], PORTRAIT)).is_none());
    // A shot with a person, then one without.
    let f = frames(8.0, |t| {
        if t < 4.0 {
            vec![person(1, 0.3, 0.2, 0.1)]
        } else {
            vec![]
        }
    });
    let cuts = [4_000_000];
    let path = focus_path(&input(&timed(&f, 0), &cuts, PORTRAIT)).unwrap();
    assert!((path.centre_at(6_000_000).unwrap().0 - 0.5).abs() < 1e-9);
}

#[test]
fn keys_are_in_the_clips_source_time() {
    let f = frames(4.0, |_| vec![person(1, 0.3, 0.2, 0.1)]);
    let at = timed(&f, 30_000_000);
    let mut i = input(&at, &[], PORTRAIT);
    i.start_us = 30_000_000;
    i.end_us = 34_000_000;
    let path = focus_path(&i).unwrap();
    assert_eq!(path.keys[0].at_us, 30_000_000);
    assert!(path.keys.last().unwrap().at_us <= 34_000_000);
}
