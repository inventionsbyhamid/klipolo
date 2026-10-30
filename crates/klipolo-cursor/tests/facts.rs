//! A take's facts: still runs, the idle fade, dwells and idle stretches.

mod common;

use common::{file, head, read};
use klipolo_cursor::facts::spans::{at_least, idle, intersect, subtract};
use klipolo_cursor::facts::{StillRuns, dwells, idle_opacity};
use klipolo_cursor::format::{Capture, Line, write_line};
use klipolo_cursor::timeline::TakeFacts;
use klipolo_cursor::{ClickSource, CursorTrack};

const S: i64 = 1_000_000;

fn mv(t: i64, x: f32, y: f32) -> Line {
    Line::Move { t, x, y }
}

/// Arrives at the middle at 2 s (fast), rests there with a beat every 500 ms until 8 s, then leaves fast.
fn rest_from_2_to_8() -> CursorTrack {
    let mut lines = vec![head()];
    for t in (1_800_000..=2 * S).step_by(4_000) {
        lines.push(mv(t, 0.1 + 2.0 * (t - 1_800_000) as f32 / S as f32, 0.5));
    }
    for t in (2_500_000..=8 * S).step_by(500_000) {
        lines.push(mv(t, 0.5, 0.5));
    }
    for t in (8_004_000..=8_200_000).step_by(4_000) {
        lines.push(mv(t, 0.5 + 2.0 * (t - 8 * S) as f32 / S as f32, 0.5));
    }
    read(&file(&lines))
}

#[test]
fn a_resting_cursor_hides_after_the_delay_and_returns_before_it_moves() {
    let track = rest_from_2_to_8();
    let still = StillRuns::of(&track);
    // It ends where the path leaves the still zone, on the line to the first sample after 8 s (on time).
    let (from, to) = still.at(5 * S).unwrap();
    assert!(from == 2 * S && (8 * S..8_004_000).contains(&to), "{to}");
    let at = |t: i64| idle_opacity(&track, &still, t, Some(S));
    assert_eq!(at(2_900_000), 1.0);
    assert!((0.3..0.7).contains(&at(3_125_000)), "halfway out");
    assert_eq!(at(3_300_000), 0.0);
    assert_eq!(at(7_850_000), 0.0);
    assert!(
        at(7_900_000) > 0.0 && at(7_950_000) > at(7_900_000),
        "rising from 7.85 s"
    );
    assert!(at(8 * S) > 0.999, "back as it starts to move");
    assert_eq!(at(8_004_000), 1.0);
    assert_eq!(
        idle_opacity(&track, &still, 5 * S, None),
        1.0,
        "never hidden"
    );
}

#[test]
fn a_burn_in_after_the_last_position_ends_the_last_still_run() {
    // The source went silent at 3 s and the recorder burned the cursor in at 5 s: the cursor isn't known to rest after
    // that. A burn-in before the last position changes nothing.
    let with_x = |x_at: i64| {
        let mut lines = vec![head()];
        lines.extend((0..=6).map(|i| mv(i * 500_000, 0.5, 0.5)));
        lines.push(Line::Burned { t: x_at });
        read(&file(&lines))
    };
    assert_eq!(StillRuns::of(&with_x(5 * S)).runs(), [(0, 5 * S)]);
    assert_eq!(StillRuns::of(&with_x(2 * S)).runs(), [(0, i64::MAX)]);
}

#[test]
fn an_app_hidden_cursor_fades_out_quickly() {
    // Moving the whole time (no still run), hidden by an app from 1 s to 3 s.
    let mut lines = vec![head()];
    for t in (0..=4 * S).step_by(4_000) {
        lines.push(mv(t, 0.1 + 0.2 * t as f32 / S as f32, 0.5));
    }
    lines.push(Line::Visible { t: S, on: false });
    lines.push(Line::Visible { t: 3 * S, on: true });
    let track = read(&file(&lines));
    let still = StillRuns::of(&track);
    let at = |t: i64| idle_opacity(&track, &still, t, Some(S));
    assert_eq!(at(990_000), 1.0);
    assert!((0.3..0.7).contains(&at(1_040_000)), "{}", at(1_040_000));
    assert_eq!(at(1_080_000), 0.0);
    assert_eq!(at(2_900_000), 0.0);
    assert!(
        (0.3..0.7).contains(&at(3_040_000)),
        "shown, it fades back in"
    );
    assert_eq!(at(3_080_000), 1.0);
}

#[test]
fn stillness_is_measured_against_the_diagonal() {
    // A 16:9 area (a window that may turn portrait at 50 ms); the cursor twitches by (dx, dy) every 100 ms for 2 s,
    // then leaves.
    let run_at_1s = |dx: f32, dy: f32, portrait: bool| {
        let mut lines = vec![Line::Header {
            v: 2,
            capture: Capture::Separate,
            clicks: ClickSource::Events,
            w: 1600.0,
            h: 900.0,
        }];
        for (i, t) in (0..=2 * S).step_by(100_000).enumerate() {
            let k = (i % 2) as f32;
            lines.push(mv(t, 0.5 + dx * k, 0.5 + dy * k));
        }
        lines.push(mv(2_100_000, 0.9, 0.9));
        if portrait {
            let (w, h) = (900.0, 1600.0);
            lines.push(Line::Area { t: 50_000, w, h });
        }
        // A run ends where the path leaves it, just after its last still sample: to the 10 ms.
        let run = StillRuns::of(&read(&file(&lines))).at(S);
        run.map(|(from, to)| (from, to / 10_000 * 10_000))
    };
    let still = Some((0, 2 * S));
    assert_eq!(run_at_1s(0.0015, 0.0, false), still, "0.15 % along x");
    assert_eq!(run_at_1s(0.003, 0.0, false), None, "0.3 % along x moves");
    assert_eq!(
        run_at_1s(0.0, 0.003, false),
        still,
        "0.3 % along the short side is less of the diagonal"
    );
    assert_eq!(
        run_at_1s(0.0, 0.003, true),
        None,
        "turned portrait, y is long"
    );
    assert_eq!(run_at_1s(0.003, 0.0, true), still, "and x short");
}

#[test]
fn dwells_need_a_movement_before_the_pause() {
    let mut lines = vec![head()];
    let mut rest = |from: i64, to: i64, x: f32, y: f32| {
        lines.extend((from..=to).step_by(100_000).map(|t| mv(t, x, y)));
    };
    rest(0, 2 * S, 0.5, 0.5); // where it started: no travel
    rest(2_100_000, 3_100_000, 0.51, 0.5); // under 3 % of the diagonal on
    rest(3_200_000, 4_200_000, 0.7, 0.5); // a dwell
    rest(4_300_000, 4_600_000, 0.2, 0.2); // far, but too short
    rest(4_700_000, 5_700_000, 0.205, 0.2); // just past the click below
    rest(5_800_000, 7 * S, 0.6, 0.8); // a dwell
    for (t, d) in [(4_650_000, 1), (4_680_000, 0)] {
        let (x, y) = (0.2, 0.2);
        lines.push(Line::Button { t, btn: 0, d, x, y });
    }
    let track = read(&file(&lines));
    let still = StillRuns::of(&track);
    let found = dwells(&track, &still);
    // A dwell ends where the path leaves it, just after its last still sample: to the 10 ms.
    let spans: Vec<_> = found
        .iter()
        .map(|d| (d.from, d.to / 10_000 * 10_000))
        .collect();
    assert_eq!(spans, [(3_200_000, 4_200_000), (5_800_000, 7 * S)]);
    assert!((found[0].x - 0.7).abs() < 1e-4 && (found[1].y - 0.8).abs() < 1e-4);
    // What a take caches for the timeline: its clicks, these dwells and its still runs; the activity comes later.
    let facts = TakeFacts::of_cursor(&track, &still);
    assert_eq!((facts.presses.len(), &facts.dwells), (1, &found));
    assert_eq!(facts.still, still.runs());
    assert!(facts.typing.is_none() && facts.quiet_sound.is_none());
}

#[test]
fn idle_is_where_cursor_screen_and_sound_are_all_still() {
    let still = [(0, 10 * S), (12 * S, 20 * S)];
    let screen = [(2 * S, 6 * S), (7 * S, 15 * S)];
    let sound = [(S, 4 * S), (5 * S, 9 * S), (14 * S, 30 * S)];
    assert_eq!(
        intersect(&still, &screen),
        [(2 * S, 6 * S), (7 * S, 10 * S), (12 * S, 15 * S)]
    );
    assert_eq!(
        idle(&still, &screen, &sound, &[], S),
        [
            (2 * S, 4 * S),
            (5 * S, 6 * S),
            (7 * S, 9 * S),
            (14 * S, 15 * S)
        ]
    );
    assert_eq!(
        idle(&still, &screen, &sound, &[], 1_500_000),
        [(2 * S, 4 * S), (7 * S, 9 * S)]
    );
    // Typing over 3–8 s is never idle.
    assert_eq!(
        idle(&still, &screen, &sound, &[(3 * S, 8 * S)], S),
        [(2 * S, 3 * S), (8 * S, 9 * S), (14 * S, 15 * S)]
    );
    assert_eq!(
        subtract(&[(0, 10), (20, 30)], &[(2, 4), (6, 22), (25, 26), (40, 50)]),
        [(0, 2), (4, 6), (22, 25), (26, 30)]
    );
    assert_eq!(at_least(&[(0, 5), (10, 30)], 10), [(10, 30)]);
}

/// A two-hour tutorial take: one to four eased glides (0.3–1.5 s each, at 250 Hz) between pauses of 2–12 s with a beat
/// every 500 ms, so the cursor moves about a quarter of the time (the track's typical hour); clicks as `with_presses`.
fn tutorial_take(seed: u64) -> CursorTrack {
    let mut rng = common::Rng::new(seed);
    let mut bytes = file(&[head()]);
    let (mut t, mut x, mut y) = (0, 0.5, 0.5);
    while t < 7_200 * S {
        for _ in 0..1 + (rng.unit() * 4.0) as usize {
            let (from, to) = ((x, y), (rng.range(0.05, 0.95), rng.range(0.05, 0.95)));
            let steps = (rng.range(0.3, 1.5) * 1e6) as i64 / 4_000;
            for i in 1..=steps {
                let f = i as f64 / steps as f64;
                let e = f * f * (3.0 - 2.0 * f);
                (x, y) = (from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e);
                t += 4_000;
                write_line(&mv(t, x as f32, y as f32), &mut bytes);
            }
        }
        let until = t + (rng.range(2.0, 12.0) * 1e6) as i64;
        while t + 500_000 < until {
            t += 500_000;
            write_line(&mv(t, x as f32, y as f32), &mut bytes);
        }
        t = until;
    }
    common::with_presses(read(&bytes))
}

/// The best of ten builds of `track`'s still runs and dwells (the build server is shared, and its noise only adds).
fn best_build(track: &CursorTrack) -> std::time::Duration {
    let mut took = Vec::new();
    for _ in 0..10 {
        let start = std::time::Instant::now();
        let still = StillRuns::of(track);
        let found = dwells(track, &still);
        took.push(start.elapsed());
        assert!(still.runs().len() > 500 && found.len() > 500);
    }
    eprintln!(
        "{} samples, {} presses: {took:?}",
        track.len(),
        track.presses().len()
    );
    took.into_iter().min().unwrap()
}

#[test]
#[ignore = "real scale: a two-hour take"]
fn facts_for_a_two_hour_take_build_under_5_ms_in_release() {
    let ms = |release: u128| {
        if cfg!(debug_assertions) {
            release * 100
        } else {
            release
        }
    };
    let take = best_build(&tutorial_take(11));
    assert!(
        take.as_millis() < ms(5),
        "a two-hour tutorial take: {take:?}"
    );
    // The worst case, two hours of continuous motion at 250 Hz (1.3 M samples): 4.5–9.2 ms on a 2013 Xeon, quiet to shared.
    // A guard against a slower scan (a 30 % regression trips it), not a target.
    let motion = best_build(&common::with_presses(common::random_path(11, 7_200 * S)));
    assert!(
        motion.as_millis() < ms(12),
        "two hours of motion: {motion:?}"
    );
}
