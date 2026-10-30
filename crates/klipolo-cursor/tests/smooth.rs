mod common;
use klipolo_cursor::format::Line;
use klipolo_cursor::smooth::{DRAG_RAMP_US, STEP_US, SmoothedPath};
use klipolo_model::cursor_style::Smoothing;
const PX: f32 = 1.0 / 1080.0;

fn check_range(seeds: std::ops::Range<u64>) {
    for seed in seeds {
        let track = common::random_path(seed, 20_000_000);
        for s in [Smoothing::Light, Smoothing::Medium, Smoothing::Strong] {
            let path = SmoothedPath::build(&track, s);
            let ahead = (2.0 / s.omega().unwrap() * 1e6) as i64;
            for t in (0..20_000_000).step_by(4_000) {
                let (x, y) = path.position(&track, t).unwrap();
                // A convex mix of every target since the segment began: never outside their range.
                let from = path.segment_at(t).map_or(t, |(start, _)| start);
                let (lo, hi) = common::range(&track, from, t + ahead);
                assert!(
                    x >= lo.0 - 1e-4 && x <= hi.0 + 1e-4 && y >= lo.1 - 1e-4 && y <= hi.1 + 1e-4,
                    "seed {seed} {s:?} t {t}: ({x}, {y}) outside {lo:?}–{hi:?}"
                );
            }
        }
    }
}

#[test]
fn the_drawn_path_never_leaves_the_range_of_the_real_one() {
    check_range(0..30);
}

#[test]
#[ignore = "the wide sweep"]
fn the_drawn_path_never_leaves_the_range_of_the_real_one_wide() {
    check_range(30..330);
}

#[test]
fn the_drawn_path_never_jumps_where_the_real_one_doesnt() {
    for seed in 0..30u64 {
        let track = common::random_path(seed, 20_000_000);
        let path = SmoothedPath::build(&track, Smoothing::Strong);
        for t in (4_000..20_000_000).step_by(4_000) {
            let (a, b) = (
                path.position(&track, t - 4_000).unwrap(),
                path.position(&track, t).unwrap(),
            );
            let still = common::still_between(&track, t - 400_000, t); // the raw path didn't move for 0.4 s
            if still {
                assert!(
                    (a.0 - b.0).abs() <= 0.25 * PX && (a.1 - b.1).abs() <= 0.25 * PX,
                    "seed {seed} t {t}: a snap"
                );
            }
        }
    }
}

#[test]
fn a_steady_movement_has_no_lag_once_settled() {
    let track = common::line(0.1, 0.9, 0.0, 4_000_000); // 0.2 frame widths a second
    let path = SmoothedPath::build(&track, Smoothing::Medium);
    for t in (1_000_000..3_500_000).step_by(10_000) {
        let (x, _) = path.position(&track, t).unwrap();
        assert!(
            (x - track.position(t).unwrap().0).abs() <= 0.5 * PX,
            "t {t}"
        );
    }
}

#[test]
fn presses_land_exactly_where_they_happened_and_drags_are_the_real_path() {
    let track = common::with_presses(common::random_path(7, 10_000_000));
    let path = SmoothedPath::build(&track, Smoothing::Strong);
    for p in track.presses() {
        let raw = track.position(p.down).unwrap();
        let drawn = path.position(&track, p.down).unwrap();
        assert!((raw.0 - drawn.0).abs() <= 0.5 * PX && (raw.1 - drawn.1).abs() <= 0.5 * PX);
        let mut t = p.down + DRAG_RAMP_US;
        while t < p.up.min(10_000_000) {
            assert_eq!(
                path.position(&track, t),
                track.position(t),
                "dragging at {t}"
            );
            t += STEP_US / 3; // off the 120 Hz grid on purpose
        }
    }
}

#[test]
fn off_is_the_raw_path_and_the_same_take_smooths_to_the_same_bits() {
    let track = common::random_path(3, 5_000_000);
    let off = SmoothedPath::build(&track, Smoothing::Off);
    assert_eq!(off.bytes(), 0);
    assert_eq!(off.position(&track, 1_234_567), track.position(1_234_567));
    let a = SmoothedPath::build(&track, Smoothing::Medium);
    let b = SmoothedPath::build(&track, Smoothing::Medium);
    // (Adapted to compile: an i64 range's `step_by` can't be reversed, so the times are collected once.)
    let times: Vec<i64> = (0..5_000_000).step_by(16_667).collect();
    let forward: Vec<_> = times.iter().map(|&t| a.position(&track, t)).collect();
    let backward: Vec<_> = times.iter().rev().map(|&t| b.position(&track, t)).collect();
    assert!(
        forward.iter().eq(backward.iter().rev()),
        "sampling order changes nothing"
    );
}

#[test]
#[ignore = "real scale: two hours of motion"]
fn two_hours_of_motion_smooth_fast_and_small() {
    let track = common::random_path(11, 7_200_000_000);
    let start = std::time::Instant::now();
    let path = SmoothedPath::build(&track, Smoothing::Medium);
    let took = start.elapsed();
    eprintln!("built in {took:?}, {} bytes", path.bytes());
    assert!(path.bytes() <= 2 * 1_700_000, "≤ 1.7 MB an hour of motion");
    assert!(took.as_millis() < if cfg!(debug_assertions) { 1_500 } else { 60 });
}

// The follower's exact step, and the velocity the motion blur reads.

#[test]
fn the_follower_is_exact_for_any_step_and_never_overshoots_a_held_target() {
    use klipolo_cursor::smooth::step;
    let (x, v) = step(0.2, 1.5, 0.8, 22.0, 0.008);
    let (hx, hv) = step(0.2, 1.5, 0.8, 22.0, 0.004);
    let (x2, v2) = step(hx, hv, 0.8, 22.0, 0.004);
    assert!(
        (x - x2).abs() < 1e-12 && (v - v2).abs() < 1e-12,
        "two half steps are one step"
    );
    let mut state: (f64, f64) = (0.2, 0.0);
    for dt in [0.001, 0.05, 0.5, 30.0] {
        state = step(state.0, state.1, 0.8, 12.0, dt);
        assert!(
            (0.2..=0.8).contains(&state.0),
            "from rest it only approaches: {state:?}"
        );
    }
    assert!(
        (state.0 - 0.8).abs() < 1e-9,
        "a long step lands on the target"
    );
}

#[test]
fn velocity_is_the_speed_of_the_drawn_path_and_zero_at_rest() {
    let track = common::line(0.1, 0.9, 0.0, 4_000_000);
    let path = SmoothedPath::build(&track, Smoothing::Medium);
    let (vx, vy) = path.velocity(&track, 2_000_000);
    assert!((vx - 0.2).abs() < 0.002 && vy.abs() < 1e-6, "({vx}, {vy})");
    assert_eq!(path.velocity(&track, 6_000_000), (0.0, 0.0));
}

// Every test above also passes if the drawn cursor is the raw one; this one doesn't.
#[test]
fn smoothing_glides_across_a_jump_and_stronger_smoothing_lags_more() {
    // A slow drift right, a jump from 0.25 to 0.8 at 1 s, then the drift again.
    let mut lines = vec![common::head()];
    for t in (0..2_000_000).step_by(4_000) {
        let x = if t < 1_000_000 { 0.2 } else { 0.75 } + 0.05 * t as f32 / 1e6;
        lines.push(Line::Move { t, x, y: 0.5 });
    }
    let track = common::read(&common::file(&lines));
    // How far short of the raw point the drawn one is, as a share of the jump left from where it began.
    let short_of = |s: Smoothing, t: i64| {
        let path = SmoothedPath::build(&track, s);
        assert!(path.segment_at(t).is_some(), "{s:?} smooths at {t}");
        let raw = track.position(t).unwrap().0;
        (raw - path.position(&track, t).unwrap().0) / (raw - 0.25)
    };
    for s in [Smoothing::Light, Smoothing::Medium, Smoothing::Strong] {
        let short = short_of(s, 1_000_000 + (1e6 / s.omega().unwrap()) as i64);
        assert!(
            (0.05..1.0).contains(&short),
            "{s:?} 1/ω after the jump: {short}"
        );
    }
    let t = 1_045_000;
    assert!(short_of(Smoothing::Strong, t) > short_of(Smoothing::Light, t));
}
