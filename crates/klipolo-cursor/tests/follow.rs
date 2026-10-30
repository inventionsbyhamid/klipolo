//! The vertical follow-crop: the crop's centre over a take, from the raw cursor, clicks ahead,
//! typing and dwells; held in its dead zone, never faster than its limit, never past the picture, and the same bits
//! every time. Harder inputs are `follow_inputs.rs`'s.

mod common;

use common::activity::best;
use common::follow::{
    ASPECT, S, STEP, Tutorial, alone, at, check_speed, click, cursor_of, glide, rest, span,
    vertical,
};
use klipolo_cursor::Us;
use klipolo_cursor::facts::{StillRuns, dwells};
use klipolo_cursor::follow::{FollowInput, FollowPath, FollowRules};
use klipolo_cursor::format::Line;

#[test]
fn the_crop_holds_while_the_cursor_stays_in_its_middle() {
    // A crop 1.5× closer (0.23 × 0.67 of the picture) can move both ways. For 10 s the cursor wanders ±4 % across and
    // ±10 % down around the middle: inside the crop's central 40 % the whole time.
    let crop = vertical(1.5);
    let wander = |k: Us| {
        let s = (k * 4_000) as f32 / 1e6;
        let (x, y) = (0.5 + 0.04 * (3.0 * s).sin(), 0.5 + 0.1 * (2.0 * s).sin());
        Line::Move { t: k * 4_000, x, y }
    };
    let track = cursor_of((0..=2_500).map(wander).collect());
    let p = FollowPath::build(&alone(&track, crop));
    let start = at(&p, 0);
    assert!(
        (start.0 - 0.5).abs() < 1e-4 && (start.1 - 0.5).abs() < 1e-4,
        "{start:?}"
    );
    for t in (0..12 * S).step_by(10_000) {
        assert_eq!(at(&p, t), start, "moved at {t}");
    }
    // Before the take's first sample the first centre holds, so a cursor file starting late never makes the crop jump
    // there; an empty path has none.
    assert_eq!(p.centre_at(-1), Some(start), "held before the first sample");
    assert_eq!(FollowPath::default().centre_at(0), None);
}

#[test]
fn the_crop_follows_when_the_cursor_nears_an_edge_and_never_shows_past_the_frame() {
    // A vertical crop, 0.35 of the picture wide: it holds while the cursor is within 0.07 of its centre (its central
    // 40 %).
    let crop = vertical(1.0);
    let (half, dead) = (crop.w / 2.0, 0.2 * crop.w);
    let mut lines = Vec::new();
    rest(&mut lines, (0, S), (0.5, 0.5));
    glide(&mut lines, (S, 4 * S), (0.5, 0.5), (0.97, 0.5));
    rest(&mut lines, (4_500_000, 6 * S), (0.97, 0.5));
    glide(&mut lines, (6 * S, 7 * S), (0.97, 0.5), (1.3, 0.5));
    rest(&mut lines, (7_500_000, 9 * S), (1.3, 0.5));
    let track = cursor_of(lines);
    let input = alone(&track, crop);
    let p = FollowPath::build(&input);
    let start = at(&p, 0);
    // Still while the cursor stays in the dead zone (until about 1.45 s), then it follows.
    let x = |t: Us| 0.5 + 0.47 * (t - S) as f64 / (3 * S) as f64;
    assert!(x(1_400_000) < start.0 + dead && x(1_500_000) > start.0 + dead);
    assert_eq!(at(&p, 1_400_000), start);
    assert!(at(&p, 1_600_000).0 > start.0, "moving by 1.6 s");
    assert!(at(&p, 2_500_000).0 > start.0 + 0.01, "follows it right");
    // Against the picture's edge by 6 s, never past it, and held once the cursor leaves the picture.
    assert!(at(&p, 6 * S).0 > 1.0 - half - 0.005, "{:?}", at(&p, 6 * S));
    check_speed(&p, &input, FollowRules::default(), ASPECT, (0, 10 * S)).unwrap();
    assert!((at(&p, 9 * S).0 - at(&p, 7 * S).0).abs() < 1e-4, "held");
    assert_eq!(
        p.centre_at(20 * S),
        p.centre_at(9 * S),
        "held after the end"
    );
}

#[test]
fn it_moves_toward_a_click_before_the_click_happens() {
    // The cursor rests on the left and darts right only in the last 100 ms before a click there at 3 s.
    let crop = vertical(1.0);
    let mut lines = Vec::new();
    rest(&mut lines, (0, 2_900_000), (0.2, 0.5));
    lines.push(Line::Move {
        t: 2_900_000,
        x: 0.2,
        y: 0.5,
    });
    glide(&mut lines, (2_900_000, 3 * S), (0.2, 0.5), (0.8, 0.5));
    click(&mut lines, 3 * S, (0.8, 0.5));
    rest(&mut lines, (3_500_000, 6 * S), (0.8, 0.5));
    let track = cursor_of(lines);
    let p = FollowPath::build(&alone(&track, crop));
    let resting = at(&p, 0);
    let first_move = (0..3 * S)
        .step_by(STEP as usize)
        .find(|&t| at(&p, t) != resting)
        .expect("it moves before the click");
    // From the first step 0.3 s or less before the click, while the cursor still rests.
    assert!(
        (2_700_000..2_700_000 + STEP).contains(&first_move),
        "first moved at {first_move}"
    );
    assert!(
        at(&p, 2_850_000).0 > resting.0 + 0.005,
        "{:?}",
        at(&p, 2_850_000)
    );
    // The resting cursor stays inside the crop until it moves.
    for t in (0..2_900_000).step_by(STEP as usize) {
        let (cx, _) = at(&p, t);
        assert!((0.2 - cx).abs() <= crop.w / 2.0, "left behind at {t}: {cx}");
    }
}

/// The speed and picture checks over `seeds` tutorials of a minute, vertical crops at 1× and 2×.
fn never_faster(seeds: std::ops::Range<u64>) {
    for seed in seeds {
        let take = Tutorial::new(seed, 60 * S);
        let crop = vertical(if seed % 2 == 0 { 1.0 } else { 2.0 });
        let runs = take.runs();
        let input = take.input(&runs, crop);
        let p = FollowPath::build(&input);
        let (from, to) = take.span();
        let travelled = check_speed(&p, &input, FollowRules::default(), ASPECT, (from, to + S))
            .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        assert!(
            travelled > 0.1,
            "seed {seed}: the crop barely moved ({travelled})"
        );
    }
}

#[test]
fn it_never_moves_faster_than_the_limit() {
    never_faster(0..30);
}

#[test]
#[ignore = "300 seeded takes"]
fn it_never_moves_faster_than_the_limit_over_300_takes() {
    never_faster(0..300);
}

#[test]
fn typing_pulls_the_crop_to_the_text_and_playing_media_does_not() {
    // The cursor is moved out of the way (a dwell from 0.8 s) and left there. A video plays upper right from 2 s to
    // 13 s, with a typing span inside it (its own changes, as a merged list can leave); a line is typed lower right
    // from 13 s.
    let crop = vertical(1.0);
    let dead = 0.2 * crop.w;
    let mut lines = vec![Line::Move {
        t: 0,
        x: 0.5,
        y: 0.5,
    }];
    glide(&mut lines, (0, 800_000), (0.5, 0.5), (0.25, 0.5));
    rest(&mut lines, (1_300_000, 20 * S), (0.25, 0.5));
    let track = cursor_of(lines);
    let dwells = dwells(&track, &StillRuns::of(&track));
    assert_eq!(dwells.len(), 1, "{dwells:?}");
    let media = [span(2 * S, 13 * S, (0.65, 0.15, 0.2, 0.2))];
    let typing = [
        span(4 * S, 8 * S, (0.7, 0.2, 0.05, 0.03)),
        span(13 * S, 19 * S, (0.72, 0.62, 0.06, 0.03)),
    ];
    let input = FollowInput {
        dwells: &dwells,
        typing: &typing,
        media: &media,
        ..alone(&track, crop)
    };
    let p = FollowPath::build(&input);
    // Resting 0.07 right of the dwell (it brought the cursor back into its central 40 %) all through the video: the
    // span inside it would pull it past 0.6.
    for t in (3 * S..12_900_000).step_by(100_000) {
        let (cx, _) = at(&p, t);
        assert!((cx - (0.25 + dead)).abs() < 2e-3, "pulled at {t}: {cx}");
    }
    // By 19 s the typed text is inside the crop's central 40 %, though the cursor dwells far to the left.
    let (cx, _) = at(&p, 19 * S);
    assert!(cx - dead <= 0.72 + 1e-3 && 0.78 <= cx + dead + 1e-3, "{cx}");
}

#[test]
fn the_same_take_follows_the_same_bits() {
    let (a, b) = (Tutorial::new(3, 60 * S), Tutorial::new(3, 60 * S));
    let crop = vertical(1.5);
    let (ra, rb) = (a.runs(), b.runs());
    let (pa, pb) = (
        FollowPath::build(&a.input(&ra, crop)),
        FollowPath::build(&b.input(&rb, crop)),
    );
    let bits = |c: Option<(f64, f64)>| c.map(|(x, y)| (x.to_bits(), y.to_bits()));
    let (from, to) = a.span();
    for t in (from - S..to + S).step_by(7_919) {
        assert_eq!(bits(pa.centre_at(t)), bits(pb.centre_at(t)), "at {t}");
    }
    assert_eq!(pa.bytes(), pb.bytes());
}

#[test]
#[ignore = "real scale: a two-hour take, in release"]
fn two_hours_build_in_100_ms_and_half_a_megabyte_an_hour() {
    let take = Tutorial::new(9, 7_200 * S);
    let runs = take.runs();
    let input = take.input(&runs, vertical(1.0));
    let mut p = FollowPath::default();
    let took = best(&mut || p = FollowPath::build(&input));
    eprintln!(
        "2 h: {took:?}, {} bytes; {} presses, {} dwells, {} typing, {} media spans",
        p.bytes(),
        take.track.presses().len(),
        take.dwells.len(),
        take.typing.len(),
        take.media.len()
    );
    assert!(
        took.as_millis() <= if cfg!(debug_assertions) { 1_000 } else { 100 },
        "{took:?}"
    );
    assert!(p.bytes() <= 1_000_000, "{} bytes", p.bytes());
}
