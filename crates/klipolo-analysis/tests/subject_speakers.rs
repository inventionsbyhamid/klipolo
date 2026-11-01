//! Who the crop keeps in frame where two or more people don't all fit it: whoever is speaking, by the switching rules
//! (a new speaker after 1.5 s of talk, at least 2 s between switches, a far switch cut at the start of the speech, a
//! near one eased), or who the person or chat chose.

use klipolo_analysis::providers::{TrackedBox, TrackedFrame};
use klipolo_analysis::subject::path::{FULL, PathInput, Talk, focus_path, people};
use klipolo_model::fill_focus::{FocusPath, Keep, KeepRange, Who};
use klipolo_model::time::Us;

/// A 16:9 picture filling 9:16 shows this share of its width, all of its height.
const PORTRAIT: (f64, f64) = ((1080.0 / 1920.0) / (1920.0 / 1080.0), 1.0);
const SQUARE: (f64, f64) = (0.5625, 1.0);
const FPS: f64 = 2.0;

fn s(seconds: f64) -> Us {
    (seconds * 1e6).round() as Us
}

fn person(id: u32, cx: f64, width: f64) -> TrackedBox {
    TrackedBox {
        id,
        left: cx - width / 2.0,
        top: 0.1,
        right: cx + width / 2.0,
        bottom: 1.0,
        confidence: Some(0.9),
    }
}

/// `seconds` of frames at 2 a second, each holding `boxes`.
fn frames(seconds: f64, boxes: &[TrackedBox]) -> Vec<TrackedFrame> {
    let n = (seconds * FPS).round() as u32;
    (0..n)
        .map(|frame| TrackedFrame {
            frame,
            boxes: boxes.to_vec(),
        })
        .collect()
}

fn timed(frames: &[TrackedFrame]) -> Vec<(Us, &TrackedFrame)> {
    (frames.iter())
        .map(|f| ((f64::from(f.frame) / FPS * 1e6) as Us, f))
        .collect()
}

fn talk(from: f64, to: f64, person: u8) -> Talk {
    Talk {
        start_us: s(from),
        end_us: s(to),
        person,
    }
}

/// Two people far apart: P1 (tracked as 7) on the left, P2 (tracked as 3) on the right.
fn two_apart(seconds: f64) -> Vec<TrackedFrame> {
    frames(seconds, &[person(3, 0.75, 0.2), person(7, 0.25, 0.2)])
}

fn path_of(f: &[TrackedFrame], crop: (f64, f64), talk: &[Talk], keep: &Keep) -> FocusPath {
    let timed = timed(f);
    let input = PathInput {
        frames: &timed,
        start_us: 0,
        end_us: (f.len() as f64 / FPS * 1e6) as Us,
        cuts: &[],
        crop,
        talk,
        keep,
        area: FULL,
    };
    let path = focus_path(&input).expect("someone is there");
    assert!(path.is_valid(), "{path:?}");
    path
}

fn x_at(path: &FocusPath, seconds: f64) -> f64 {
    path.centre_at(s(seconds)).unwrap().0
}

fn cuts(path: &FocusPath) -> Vec<Us> {
    path.keys
        .iter()
        .filter(|k| k.cut)
        .map(|k| k.at_us)
        .collect()
}

/// Whether the crop is on the person at `x`, as far as the picture's edge lets it.
fn on(path: &FocusPath, seconds: f64, x: f64) -> bool {
    let half = PORTRAIT.0 / 2.0;
    (x_at(path, seconds) - x.clamp(half, 1.0 - half)).abs() < 0.03
}

#[test]
fn the_shots_people_are_named_left_to_right() {
    assert_eq!(people(&two_apart(4.0)), [7, 3]);
    // Someone small at the back isn't one of them.
    let f = frames(4.0, &[person(1, 0.7, 0.25), person(2, 0.2, 0.05)]);
    assert_eq!(people(&f), [1]);
}

#[test]
fn the_crop_cuts_to_whoever_speaks_at_the_start_of_their_speech() {
    let f = two_apart(24.0);
    let said = [talk(0.0, 8.0, 1), talk(8.0, 16.0, 2), talk(16.0, 24.0, 1)];
    let path = path_of(&f, PORTRAIT, &said, &Keep::Speaker);
    assert!(on(&path, 4.0, 0.25) && on(&path, 7.9, 0.25), "{path:?}");
    assert!(on(&path, 8.0, 0.75) && on(&path, 15.9, 0.75), "{path:?}");
    assert!(on(&path, 16.0, 0.25) && on(&path, 23.0, 0.25), "{path:?}");
    // A far switch is a cut, never a long pan.
    assert_eq!(cuts(&path), [s(8.0), s(16.0)]);
}

#[test]
fn the_first_speaker_of_a_shot_opens_it() {
    let f = two_apart(12.0);
    let path = path_of(&f, PORTRAIT, &[talk(1.0, 9.0, 2)], &Keep::Speaker);
    assert!(on(&path, 0.0, 0.75) && on(&path, 11.0, 0.75), "{path:?}");
    assert!(cuts(&path).is_empty());
}

#[test]
fn an_interjection_never_takes_the_frame() {
    let f = two_apart(20.0);
    // "Mm-hm" alone in a pause, and one over the speaker.
    let said = [
        talk(0.0, 10.0, 1),
        talk(10.0, 11.0, 2),
        talk(11.0, 20.0, 1),
        talk(14.0, 14.6, 2),
    ];
    let path = path_of(&f, PORTRAIT, &said, &Keep::Speaker);
    assert!(cuts(&path).is_empty(), "{path:?}");
    for t in [2.0, 10.5, 14.3, 19.0] {
        assert!(on(&path, t, 0.25), "at {t}: {path:?}");
    }
}

#[test]
fn the_crop_stays_at_least_two_seconds_before_it_switches_again() {
    let f = two_apart(16.0);
    // P2 answers for 1.6 s; P1 picks up at once.
    let said = [talk(0.0, 5.0, 1), talk(5.0, 6.6, 2), talk(6.6, 16.0, 1)];
    let path = path_of(&f, PORTRAIT, &said, &Keep::Speaker);
    assert_eq!(cuts(&path), [s(5.0), s(7.0)], "{path:?}");
    assert!(on(&path, 6.8, 0.75) && on(&path, 7.0, 0.25), "{path:?}");
}

#[test]
fn rapid_back_and_forth_stays_with_whoever_speaks_more() {
    let f = two_apart(20.0);
    let said = [
        talk(0.0, 6.0, 1),
        // Short turns, P2 saying more of them.
        talk(6.0, 7.0, 2),
        talk(7.0, 7.6, 1),
        talk(7.6, 8.8, 2),
        talk(8.8, 9.2, 1),
        talk(9.2, 10.4, 2),
        talk(10.4, 11.0, 1),
        talk(11.0, 20.0, 1),
    ];
    let path = path_of(&f, PORTRAIT, &said, &Keep::Speaker);
    // P1's last short turn runs on into their long one, which takes the frame from its start.
    assert_eq!(cuts(&path), [s(6.0), s(10.4)], "{path:?}");
    assert!(on(&path, 7.2, 0.75) && on(&path, 10.3, 0.75), "{path:?}");
    assert!(on(&path, 15.0, 0.25), "{path:?}");
}

#[test]
fn talking_over_each_other_keeps_the_one_who_said_as_much() {
    let f = two_apart(14.0);
    // P2 talks over P1 for 4 s: as much as P1 in that stretch, so the crop stays.
    let said = [talk(0.0, 10.0, 1), talk(4.0, 8.0, 2), talk(10.0, 14.0, 1)];
    let path = path_of(&f, PORTRAIT, &said, &Keep::Speaker);
    assert!(cuts(&path).is_empty(), "{path:?}");
    assert!(on(&path, 6.0, 0.25), "{path:?}");
}

#[test]
fn silence_holds_the_last_speaker() {
    let f = two_apart(20.0);
    // Nobody visible speaks from 5 to 12 s (silence, or a voice off camera).
    let said = [talk(0.0, 2.0, 2), talk(2.0, 5.0, 1), talk(12.0, 20.0, 2)];
    let path = path_of(&f, PORTRAIT, &said, &Keep::Speaker);
    assert!(on(&path, 8.0, 0.25) && on(&path, 11.9, 0.25), "{path:?}");
    assert!(on(&path, 12.0, 0.75), "{path:?}");
}

#[test]
fn a_near_switch_eases_instead_of_cutting() {
    // Too far apart to frame together in 9:16, close enough to move between.
    let f = frames(16.0, &[person(1, 0.45, 0.16), person(2, 0.58, 0.16)]);
    let said = [talk(0.0, 6.0, 1), talk(6.0, 16.0, 2)];
    let path = path_of(&f, PORTRAIT, &said, &Keep::Speaker);
    assert!(cuts(&path).is_empty(), "{path:?}");
    assert!((x_at(&path, 3.0) - 0.45).abs() < 0.03, "{path:?}");
    assert!((x_at(&path, 15.0) - 0.58).abs() < 0.03, "{path:?}");
    // On the way, somewhere between the two.
    let mid = (60..=100)
        .map(|t| x_at(&path, f64::from(t) / 10.0))
        .any(|x| x > 0.47 && x < 0.56);
    assert!(mid, "{path:?}");
}

#[test]
fn people_who_fit_together_or_one_person_are_framed_as_before() {
    // Square: two people at 0.38 and 0.62 fit, so they are framed together whoever speaks.
    let f = frames(12.0, &[person(1, 0.38, 0.12), person(2, 0.62, 0.12)]);
    let said = [talk(0.0, 6.0, 1), talk(6.0, 12.0, 2)];
    let path = path_of(&f, SQUARE, &said, &Keep::Speaker);
    assert!((x_at(&path, 2.0) - 0.5).abs() < 0.02 && (x_at(&path, 9.0) - 0.5).abs() < 0.02);
    // One person, whatever is said.
    let f = frames(12.0, &[person(1, 0.3, 0.2)]);
    let with = path_of(&f, PORTRAIT, &said, &Keep::Speaker);
    let without = path_of(&f, PORTRAIT, &[], &Keep::Speaker);
    assert_eq!(with, without);
}

#[test]
fn without_anyone_heard_the_main_subject_holds_as_before() {
    let f = frames(10.0, &[person(1, 0.25, 0.2), person(2, 0.75, 0.3)]);
    let path = path_of(&f, PORTRAIT, &[], &Keep::Speaker);
    // The bigger one, on the right.
    assert!(on(&path, 5.0, 0.75), "{path:?}");
}

#[test]
fn one_person_chosen_stays_in_frame_whoever_speaks() {
    let f = two_apart(16.0);
    let said = [talk(0.0, 8.0, 1), talk(8.0, 16.0, 2)];
    let path = path_of(&f, PORTRAIT, &said, &Keep::Person { person: 2 });
    assert!(on(&path, 2.0, 0.75) && on(&path, 12.0, 0.75), "{path:?}");
    assert!(cuts(&path).is_empty());
    // Someone not there is no one: whoever speaks.
    let path = path_of(&f, PORTRAIT, &said, &Keep::Person { person: 3 });
    assert!(on(&path, 2.0, 0.25) && on(&path, 12.0, 0.75), "{path:?}");
}

#[test]
fn both_frames_everyone_where_they_fit_and_else_whoever_speaks_more() {
    let said = [talk(0.0, 4.0, 1), talk(4.0, 12.0, 2)];
    let f = frames(12.0, &[person(1, 0.38, 0.12), person(2, 0.62, 0.12)]);
    let path = path_of(&f, SQUARE, &said, &Keep::Both);
    assert!((x_at(&path, 6.0) - 0.5).abs() < 0.02, "{path:?}");
    let f = two_apart(12.0);
    let path = path_of(&f, PORTRAIT, &said, &Keep::Both);
    assert!(on(&path, 1.0, 0.75) && on(&path, 11.0, 0.75), "{path:?}");
}

#[test]
fn chats_ranges_keep_who_it_chose_there_and_the_speaker_elsewhere() {
    let f = two_apart(20.0);
    let said = [talk(0.0, 20.0, 1)];
    let keep = Keep::Ranges {
        ranges: vec![KeepRange {
            start_us: s(5.0),
            end_us: s(10.0),
            who: Who::Person { person: 2 },
        }],
    };
    let path = path_of(&f, PORTRAIT, &said, &keep);
    assert!(on(&path, 2.0, 0.25) && on(&path, 15.0, 0.25), "{path:?}");
    assert!(on(&path, 5.0, 0.75) && on(&path, 9.9, 0.75), "{path:?}");
    assert_eq!(cuts(&path), [s(5.0), s(10.0)]);
}
