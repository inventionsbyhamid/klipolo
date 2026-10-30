//! The follow-crop's harder inputs: a
//! hidden or burned-in cursor held, inputs out of range that never panic, event times past the take left out, and a file
//! with no area measured on the take's picture. The typed part is `follow_typed.rs`'s.

mod common;

use common::activity::activity_of;
use common::follow::{
    S, STEP, alone, at, check_speed, cursor_of, glide, once, rest, span, vertical,
};
use common::{file, read};
use klipolo_cursor::Us;
use klipolo_cursor::activity::Tick;
use klipolo_cursor::facts::Dwell;
use klipolo_cursor::follow::{Crop, FollowInput, FollowPath, FollowRules};
use klipolo_cursor::format::Line;

#[test]
fn a_hidden_or_burned_in_cursor_holds_the_crop() {
    let crop = vertical(1.0);
    // An app hides the cursor from 0.5 s to 4 s, while it moves right.
    let mut lines = Vec::new();
    rest(&mut lines, (0, S), (0.5, 0.5));
    glide(&mut lines, (S, 3 * S), (0.5, 0.5), (0.9, 0.5));
    rest(&mut lines, (3_500_000, 7 * S), (0.9, 0.5));
    lines.push(Line::Visible {
        t: 500_000,
        on: false,
    });
    lines.push(Line::Visible { t: 4 * S, on: true });
    let track = cursor_of(lines);
    let p = FollowPath::build(&alone(&track, crop));
    assert_eq!(at(&p, 4 * S - STEP), at(&p, 0), "held while hidden");
    assert!(
        at(&p, 6 * S).0 > 0.7,
        "follows once shown: {:?}",
        at(&p, 6 * S)
    );
    // The cursor jumps right at 1.95 s and the recorder burns it in at 2 s: the file has no positions after. The screen
    // file runs to 7 s, and typing on the left from 4 s keeps the path going.
    let burned = |x_line: bool| {
        let mut lines = Vec::new();
        rest(&mut lines, (0, 1_900_000), (0.5, 0.5));
        lines.push(Line::Move {
            t: 1_900_000,
            x: 0.5,
            y: 0.5,
        });
        lines.push(Line::Move {
            t: 1_950_000,
            x: 0.95,
            y: 0.5,
        });
        if x_line {
            lines.push(Line::Burned { t: 2 * S });
        }
        let track = cursor_of(lines);
        let typing = [span(4 * S, 6 * S, (0.3, 0.5, 0.02, 0.02))];
        let screen = activity_of(&[Tick::default(); 70]);
        let runs = [(0, &screen)];
        let p = FollowPath::build(&FollowInput {
            typing: &typing,
            activity: &runs,
            ..alone(&track, crop)
        });
        at(&p, 3_900_000).0
    };
    assert!(
        burned(true) < 0.6,
        "held after the burn-in: {}",
        burned(true)
    );
    assert!(
        burned(false) > 0.7,
        "without it, the last position is held: {}",
        burned(false)
    );
}

#[test]
fn inputs_out_of_range_never_panic_and_the_crop_stays_inside() {
    // A format-1 file (no header, so no area) and no picture aspect either: speeds are measured on a square.
    let wander = |k: Us| {
        let s = k as f32 * 0.37;
        Line::Move {
            t: k * 20_000,
            x: 0.5 + 0.7 * s.sin(),
            y: 0.5 + 0.6 * (s * 0.7).cos(),
        }
    };
    let track = read(&file(&(0..500).map(wander).collect::<Vec<_>>()));
    assert!(track.header().is_none());
    let nan = f32::NAN;
    let dwells = [
        Dwell {
            from: 0,
            to: 5 * S,
            x: nan,
            y: 0.5,
        },
        Dwell {
            from: 6 * S,
            to: 2 * S,
            x: 0.2,
            y: 0.2,
        },
    ];
    let typing = [
        span(0, S, (0.2, 0.2, f32::INFINITY, 0.1)),
        span(3 * S, 4 * S, (nan, 0.1, 0.1, 0.1)),
        span(2 * S, S, (0.9, 0.9, -1.0, 0.5)),
        span(5 * S, 7 * S, (0.1, 0.1, 0.9, 0.9)),
    ];
    let media = [span(0, 9 * S, (nan, nan, nan, nan))];
    let a = activity_of(&[Tick::default(); 10]);
    let runs = [(-5 * S, &a), (Us::MIN, &a)];
    let rules = FollowRules {
        dead_zone: f64::NAN,
        safe: 0.7,
        omega: -1.0,
        speed: f64::INFINITY,
        pre_roll_us: -5,
    };
    let crops = [
        Crop { w: 0.3, h: 0.5 },
        Crop {
            w: f64::NAN,
            h: 3.0,
        },
        Crop { w: -1.0, h: 0.0 },
    ];
    for crop in crops {
        let input = FollowInput {
            track: &track,
            dwells: &dwells,
            typing: &typing,
            media: &media,
            activity: &runs,
            aspect: f64::NAN,
            crop,
            rules,
        };
        let p = FollowPath::build(&input);
        for t in (0..12 * S).step_by(10_007) {
            let (x, y) = at(&p, t);
            assert!(
                (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y),
                "{crop:?} at {t}: ({x}, {y})"
            );
        }
    }
    // Out-of-range rules fall back to the defaults, and the crop's limits hold.
    let input = FollowInput {
        track: &track,
        dwells: &dwells,
        typing: &typing,
        media: &media,
        activity: &runs,
        aspect: f64::NAN,
        crop: crops[0],
        rules,
    };
    // No area and no aspect: measured on a square.
    let p = FollowPath::build(&input);
    check_speed(&p, &input, FollowRules::default(), 1.0, (0, 12 * S)).unwrap();
    // Event times past the take (its last sample, and its screen files' ends) are left out: a span that never ends and
    // a click 17 years on make a path no longer than the take's 10 s.
    let mut far = (0..500).map(wander).collect::<Vec<_>>();
    for (t, d) in [(1 << 49, 1), ((1 << 49) + 80_000, 0)] {
        far.push(Line::Button {
            t,
            btn: 0,
            d,
            x: 0.5,
            y: 0.5,
        });
    }
    let far = read(&file(&far));
    assert_eq!(far.clicks().count(), 1);
    let endless = [span(S, Us::MAX, (0.4, 0.4, 0.01, 0.01))];
    let p = FollowPath::build(&FollowInput {
        track: &far,
        typing: &endless,
        ..input
    });
    assert!(p.bytes() <= 2_000, "{} bytes", p.bytes());
}

#[test]
fn a_file_with_no_area_measures_speed_on_the_takes_picture() {
    // A format-1 file (no area) of a take whose picture is 1.6 wide. The cursor jumps 0.1 down, inside a crop 1.5×
    // closer that can move both ways; rules that chase it hard (no dead zone, ω 20) make the limit bind. Measured on the
    // picture, a move down covers up to 1.6 times the share of the height a square would allow.
    let still = |t0: Us, y: f32| {
        (0..=20).map(move |k| Line::Move {
            t: t0 + k * 100_000,
            x: 0.5,
            y,
        })
    };
    let track = read(&file(
        &still(0, 0.5)
            .chain(still(2_004_000, 0.6))
            .collect::<Vec<_>>(),
    ));
    assert!(track.header().is_none());
    let crop = vertical(1.5);
    let rules = FollowRules {
        dead_zone: 0.0,
        omega: 20.0,
        ..FollowRules::default()
    };
    let fastest_down = |aspect: f64| {
        let p = FollowPath::build(&FollowInput {
            aspect,
            rules,
            ..alone(&track, crop)
        });
        let y: Vec<f64> = (0..4 * S)
            .step_by(STEP as usize)
            .map(|t| at(&p, t).1)
            .collect();
        y.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max)
    };
    let square = once(crop.w);
    let on_the_picture = fastest_down(1.6);
    assert!(
        on_the_picture > 1.2 * square && on_the_picture <= 1.6 * square + 1e-9,
        "{} of a square's limit",
        on_the_picture / square
    );
    assert!(
        fastest_down(f64::NAN) <= square + 1e-9,
        "no aspect: a square"
    );
}
