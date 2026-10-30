//! How the follow-crop moves:
//! it holds while the cursor is inside its central dead zone and brings it back inside once it
//! leaves. It moves up to twice as fast only while the cursor is in its margin, ramped across the margin's depth, along
//! a side it can move, and never for a click still to come, slowing to its limit before one rather than in one step.

mod common;

use common::follow::{
    ASPECT, S, STEP, alone, at, check_speed, click, cursor_of, glide, once, rest, step_lengths,
    vertical,
};
use klipolo_cursor::follow::{FollowInput, FollowPath, FollowRules};
use klipolo_cursor::format::Line;

#[test]
fn the_crop_brings_a_resting_cursor_back_into_its_dead_zone() {
    // The cursor moves out of the crop's central 40 % but stays inside its safe room, and rests there.
    let crop = vertical(1.0);
    let dead = 0.2 * crop.w;
    let mut lines = Vec::new();
    rest(&mut lines, (0, S), (0.5, 0.5));
    glide(&mut lines, (S, 2 * S), (0.5, 0.5), (0.61, 0.5));
    rest(&mut lines, (2_500_000, 12 * S), (0.61, 0.5));
    let track = cursor_of(lines);
    let input = alone(&track, crop);
    let p = FollowPath::build(&input);
    // Never faster than its limit (the cursor never reaches the margin)…
    check_speed(&p, &input, FollowRules::default(), ASPECT, (0, 12 * S)).unwrap();
    // …brought back inside the dead zone, then held bit for bit.
    let (cx, _) = at(&p, 8 * S);
    assert!(cx > 0.52 && 0.61 - cx <= dead + 1e-4, "{cx}");
    for t in (8 * S..12 * S).step_by(10_000) {
        assert_eq!(at(&p, t), at(&p, 8 * S), "moved at {t}");
    }
}

#[test]
fn it_moves_up_to_twice_as_fast_only_while_the_cursor_is_in_the_margin() {
    // A quick flick to the picture's right edge: the cursor runs through the crop's margin and past it.
    let crop = vertical(1.0);
    let mut lines = Vec::new();
    rest(&mut lines, (0, S), (0.5, 0.5));
    glide(&mut lines, (S, 1_300_000), (0.5, 0.5), (0.95, 0.5));
    rest(&mut lines, (1_800_000, 6 * S), (0.95, 0.5));
    let track = cursor_of(lines);
    let input = alone(&track, crop);
    let p = FollowPath::build(&input);
    // Twice at most, and once at most whenever the cursor is inside the safe room.
    check_speed(&p, &input, FollowRules::default(), ASPECT, (0, 6 * S)).unwrap();
    let fastest = step_lengths(&p, (0, 6 * S)).into_iter().fold(0.0, f64::max);
    assert!(
        fastest > 1.3 * once(crop.w),
        "{}× its limit",
        fastest / once(crop.w)
    );
}

#[test]
fn a_cursor_near_the_top_of_a_full_height_crop_never_speeds_it_across() {
    // A 9:16 crop is as tall as the picture: it can't move up or down, so the cursor in its top 10 % is no reason to
    // move faster across. Rules that chase the cursor hard (no dead zone, ω 20) make the limit bind.
    let crop = vertical(1.0);
    let rules = FollowRules {
        dead_zone: 0.0,
        omega: 20.0,
        ..FollowRules::default()
    };
    let mut lines = Vec::new();
    rest(&mut lines, (0, S), (0.5, 0.05));
    // A jump of 0.1 across, inside the safe room.
    lines.push(Line::Move {
        t: S + 4_000,
        x: 0.6,
        y: 0.05,
    });
    rest(&mut lines, (1_500_000, 4 * S), (0.6, 0.05));
    let track = cursor_of(lines);
    let input = FollowInput {
        rules,
        ..alone(&track, crop)
    };
    let p = FollowPath::build(&input);
    let fastest = step_lengths(&p, (0, 4 * S)).into_iter().fold(0.0, f64::max);
    let limit = once(crop.w);
    assert!(
        fastest > 0.9 * limit && fastest <= limit + 1e-12,
        "{}× its limit",
        fastest / limit
    );
}

#[test]
fn a_click_ahead_never_speeds_the_crop_up_or_leaves_a_resting_cursor_behind() {
    // The cursor rests at the left edge of the crop's dead zone; a far click comes at 3 s, the cursor darting there
    // only in its last 30 ms.
    let crop = vertical(1.0);
    let dead = 0.2 * crop.w;
    let mut lines = Vec::new();
    rest(&mut lines, (0, S), (0.5, 0.5));
    glide(&mut lines, (S, 1_500_000), (0.5, 0.5), (0.42, 0.5));
    rest(&mut lines, (2 * S, 2_970_000), (0.42, 0.5));
    lines.push(Line::Move {
        t: 2_970_000,
        x: 0.42,
        y: 0.5,
    });
    glide(&mut lines, (2_970_000, 3 * S), (0.42, 0.5), (0.95, 0.5));
    click(&mut lines, 3 * S, (0.95, 0.5));
    rest(&mut lines, (3_500_000, 5 * S), (0.95, 0.5));
    let track = cursor_of(lines);
    let p = FollowPath::build(&alone(&track, crop));
    let (cx, _) = at(&p, 2_600_000);
    assert!((cx - (0.42 + dead)).abs() < 2e-3, "{cx}");
    // Toward the click at its limit, no faster, from 0.3 s before it.
    let lead = step_lengths(&p, (2_600_000, 2_970_000));
    let fastest = lead.iter().copied().fold(0.0, f64::max);
    assert!(
        fastest > 0.0 && fastest <= once(crop.w) + 1e-12,
        "{}× its limit",
        fastest / once(crop.w)
    );
    for t in (0..2_970_000).step_by(STEP as usize) {
        let (cx, _) = at(&p, t);
        let (x, _) = track.position(t).unwrap();
        assert!(
            (f64::from(x) - cx).abs() <= crop.w / 2.0,
            "left behind at {t}: {cx}"
        );
    }
}

#[test]
fn a_click_after_a_chase_never_drops_the_crops_speed_in_one_step() {
    // A deliberate move to a button outside the crop (1 s, 0.45 of the picture), a short rest, then the click at
    // 1.4 s. The crop chases faster than its limit, and slows to it over the 0.3 s before the
    // click comes due, never in one step.
    chase_then_click(FollowRules::default());
}

#[test]
fn a_short_pre_roll_still_slows_the_crop_over_300_ms() {
    // The same with a 50 ms pre-roll: the slowing still takes 0.3 s, not one pre-roll.
    chase_then_click(FollowRules {
        pre_roll_us: 50_000,
        ..FollowRules::default()
    });
}

/// The chase and click above under `rules`: faster than the limit, and never slowing by more than 0.2 of it a step.
fn chase_then_click(rules: FollowRules) {
    let crop = vertical(1.0);
    let mut lines = vec![Line::Move {
        t: 0,
        x: 0.25,
        y: 0.5,
    }];
    glide(&mut lines, (0, S), (0.25, 0.5), (0.7, 0.5));
    rest(&mut lines, (S, 3 * S), (0.7, 0.5));
    click(&mut lines, 1_400_000, (0.7, 0.5));
    let track = cursor_of(lines);
    let input = FollowInput {
        rules,
        ..alone(&track, crop)
    };
    let p = FollowPath::build(&input);
    check_speed(&p, &input, rules, ASPECT, (0, 3 * S)).unwrap();
    let steps = step_lengths(&p, (0, 3 * S));
    let fastest = steps.iter().copied().fold(0.0, f64::max);
    assert!(
        fastest > 1.3 * once(crop.w),
        "{}× its limit",
        fastest / once(crop.w)
    );
    let drop = steps.windows(2).map(|w| w[0] - w[1]).fold(0.0, f64::max);
    assert!(
        drop <= 0.2 * once(crop.w),
        "slowed by {}× its limit in one step",
        drop / once(crop.w)
    );
}

#[test]
fn the_limit_ramps_across_the_margin_and_never_switches() {
    // The cursor jumps half way into the crop's margin and rests there; rules that chase it hard (no dead zone, ω 20)
    // make the limit bind. Each step is at most 1 + (the cursor's depth in the margin) times
    // the limit, and over 1× while it is deep: a switch to 2× would break the first, no catch-up the second.
    let crop = vertical(1.0);
    let rules = FollowRules {
        dead_zone: 0.0,
        omega: 20.0,
        ..FollowRules::default()
    };
    let (half, room) = (crop.w / 2.0, 0.35 * crop.w);
    let x = 0.5 + room + 0.5 * (half - room);
    let mut lines = Vec::new();
    rest(&mut lines, (0, S), (0.5, 0.5));
    lines.push(Line::Move {
        t: S + 4_000,
        x: x as f32,
        y: 0.5,
    });
    rest(&mut lines, (1_500_000, 3 * S), (x as f32, 0.5));
    let track = cursor_of(lines);
    let p = FollowPath::build(&FollowInput {
        rules,
        ..alone(&track, crop)
    });
    let (limit, mut deep) = (once(crop.w), 0);
    for k in 30..90 {
        let (t0, t1) = (k * STEP, (k + 1) * STEP);
        let (c0, c1) = (at(&p, t0).0, at(&p, t1).0);
        let cursor = f64::from(track.position(t1).unwrap().0);
        let depth = (((cursor - c0).abs() - room) / (half - room)).clamp(0.0, 1.0);
        let step = (c1 - c0).abs();
        let most = (1.0 + depth) * limit * (1.0 + 1e-3) + 1e-12;
        assert!(
            step <= most,
            "{}× its limit at {t1}, {depth} deep",
            step / limit
        );
        deep += usize::from(depth > 0.2 && step > 1.1 * limit);
    }
    assert!(deep > 0, "it never caught up faster in the margin");
}
