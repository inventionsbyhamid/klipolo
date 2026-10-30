//! `check_plan`'s own tests: it fails each rule it is named for, on a plan broken that way.

use super::super::{Focus, Input, Key, Rules, View, Weight, plan};
use super::check_plan;
use crate::Rect01;

fn one_click() -> super::Plan {
    let rect = Rect01 {
        x: 0.5,
        y: 0.5,
        w: 0.0,
        h: 0.0,
    };
    let focus = [Focus {
        at: 10_000_000,
        until: 10_000_000,
        rect,
        weight: Weight::Click,
    }];
    plan(&Input {
        span: (0, 30_000_000),
        focus: &focus,
        fixed: &[],
        rules: Rules::default(),
    })
}

/// The check fails each rule it is named for, on a plan broken that way.
#[test]
fn the_check_catches_a_broken_plan() {
    let (r, span) = (Rules::default(), 30_000_000);
    assert_eq!(check_plan(&one_click(), r, span), Ok(()));
    let broken = |f: &dyn Fn(&mut super::Plan)| {
        let mut p = one_click();
        f(&mut p);
        check_plan(&p, r, span).expect_err("a broken plan passes")
    };
    // The move in (1.08 s) starts 0.5 s earlier: now longer than a move may take.
    assert!(broken(&|p| p.keys[0].at -= 500_000).contains("a move of"));
    // The move in takes half its time: faster than the limits.
    assert!(broken(&|p| p.keys[0].at += (p.keys[1].at - p.keys[0].at) / 2).contains("pan"));
    // The shot claims to hold past the start of its move out.
    assert!(broken(&|p| p.shots[0].to = 29_500_000).contains("not held"));
    // A dropped action in a corner the zoom hides, which the planner's own list forgot.
    let forgot = Focus {
        at: 10_500_000,
        until: 10_500_000,
        rect: Rect01 {
            x: 0.02,
            y: 0.98,
            w: 0.0,
            h: 0.0,
        },
        weight: Weight::Click,
    };
    assert!(broken(&|p| p.all.push(forgot)).contains("off screen"));
    // A pan-only move, out and back, inside the shot's hold: the scale never changes.
    let v = one_click().keys[1].view;
    let pan = |p: &mut super::Plan| {
        let (a, b) = (
            Key {
                at: 11_000_000,
                view: v,
                moving: true,
            },
            Key {
                at: 11_400_000,
                view: View {
                    cx: v.cx + 0.02,
                    ..v
                },
                moving: true,
            },
        );
        p.keys.splice(
            2..2,
            [
                a,
                b,
                Key {
                    at: 11_800_000,
                    view: v,
                    moving: false,
                },
            ],
        );
    };
    assert!(broken(&pan).contains("not held"));
}

/// The squeezed move: two of the person's zooms 0.3 s apart at very different views pass (the move
/// between them is as they placed it), but an Auto move beside them made faster than its limits still fails.
#[test]
fn only_a_move_filling_the_gap_between_their_zooms_is_exempt() {
    use super::super::Fixed;
    let (r, span) = (Rules::default(), 30_000_000);
    let theirs = |from, to, cx, cy| Fixed {
        from,
        to,
        view: View { cx, cy, scale: 3.0 },
    };
    let fixed = [
        theirs(5_000_000, 8_000_000, 0.2, 0.2),
        theirs(8_300_000, 11_000_000, 0.8, 0.8),
    ];
    let rect = Rect01 {
        x: 0.5,
        y: 0.5,
        w: 0.0,
        h: 0.0,
    };
    let focus = [Focus {
        at: 20_000_000,
        until: 20_000_000,
        rect,
        weight: Weight::Click,
    }];
    let planned = || {
        plan(&Input {
            span: (0, span),
            focus: &focus,
            fixed: &fixed,
            rules: r,
        })
    };
    assert_eq!(check_plan(&planned(), r, span), Ok(()));
    // The click's zoom in, from the whole frame, takes half its time.
    let mut p = planned();
    let k = p
        .keys
        .iter()
        .position(|k| k.moving && k.at > 12_000_000 && k.view == View::FULL)
        .expect("the zoom in");
    p.keys[k].at += (p.keys[k + 1].at - p.keys[k].at) / 2;
    let e = check_plan(&p, r, span).expect_err("a sped-up Auto move passes");
    assert!(e.contains("pan"), "{e}");
}
