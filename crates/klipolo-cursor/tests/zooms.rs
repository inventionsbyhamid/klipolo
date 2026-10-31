//! Auto zoom on a take's clicks: the camera plans the take once with the spec's rules, the plan is cut into pieces
//! where the view rests on the whole frame, and each piece no zoom covers yet becomes an attached Auto zoom item.
//! Facts are built by hand on the timeline.

use klipolo_cursor::Us;
use klipolo_cursor::camera::{Plan, Rules, View};
use klipolo_cursor::timeline::{ClickAt, SpanAt, TimelineFacts};
use klipolo_cursor::zooms::{
    AUTO_SCALE, CLICKS, auto_plan, auto_spans, auto_zoom_ops, pieces, take_spans,
};
use klipolo_model::cursor_style::FocusKind;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::ops::{Op, TrackTarget};
use klipolo_model::timeline::{ItemContent, ZoomTarget};

const S: Us = 1_000_000;
const SPAN: (Us, Us) = (0, 20 * S);

/// Two clicks 0.8 s apart, close together on screen.
const NEAR: [(Us, f32, f32); 2] = [(2 * S, 0.30, 0.40), (2_800_000, 0.32, 0.42)];
/// The same two, then two more 8 s later across the screen.
const APART: [(Us, f32, f32); 4] = [
    NEAR[0],
    NEAR[1],
    (10 * S, 0.80, 0.70),
    (10_500_000, 0.82, 0.72),
];
/// The last click of a 60 s take, at 15 s.
const LATE: [(Us, f32, f32); 2] = [(14 * S, 0.30, 0.40), (15 * S, 0.32, 0.42)];

fn clicks(at: &[(Us, f32, f32)]) -> TimelineFacts {
    TimelineFacts {
        clicks: (at.iter())
            .map(|&(timeline_us, x, y)| ClickAt { timeline_us, x, y })
            .collect(),
        ..Default::default()
    }
}

fn planned(at: &[(Us, f32, f32)], span: (Us, Us)) -> Plan {
    auto_plan(&clicks(at), CLICKS, span, Rules::default())
}

fn gap() -> Us {
    Rules::default().out_gap_us
}

fn cut(p: &Plan) -> Vec<(Us, Us)> {
    pieces(p.keys(), gap())
}

/// APART, and the cursor resting at the right edge from 12.5 s to 14 s: inside the second zoom's held view.
fn apart_with_a_rest() -> TimelineFacts {
    TimelineFacts {
        dwells: vec![SpanAt {
            from_us: 12_500_000,
            to_us: 14 * S,
            x: 0.99,
            y: 0.75,
        }],
        ..clicks(&APART)
    }
}

/// What a take's auto zoom adds over `span`, with nothing there yet.
fn spans_of(f: &TimelineFacts, span: (Us, Us)) -> Vec<(Us, Us)> {
    take_spans(f, CLICKS, span, &[], Rules::default())
}

/// `span` planned on its own, as the engine plans an item: on its own clock, 0 at its start.
fn on_its_own(f: &TimelineFacts, (a, b): (Us, Us)) -> Plan {
    let local = TimelineFacts {
        clicks: (f.clicks.iter())
            .map(|c| ClickAt {
                timeline_us: c.timeline_us - a,
                ..*c
            })
            .collect(),
        dwells: (f.dwells.iter())
            .map(|d| SpanAt {
                from_us: d.from_us - a,
                to_us: d.to_us - a,
                ..*d
            })
            .collect(),
        ..Default::default()
    };
    auto_plan(&local, CLICKS, (0, b - a), Rules::default())
}

#[test]
fn rules_default_are_the_spec_values() {
    let r = Rules::default();
    assert_eq!(r.pre_roll_us, 300_000);
    assert_eq!(r.merge_us, 1_500_000);
    assert_eq!(r.max_scale, 2.0);
    assert_eq!(r.safe, 0.15);
    assert_eq!(r.hold_us, 1_200_000);
    assert_eq!(r.out_gap_us, 2_500_000);
    // The engine plans an Auto item with its scale as the most it zooms.
    assert_eq!(AUTO_SCALE, r.max_scale);
}

#[test]
fn clicks_under_1_5_s_apart_make_one_piece() {
    let pieces = cut(&planned(&NEAR, SPAN));
    assert_eq!(pieces.len(), 1, "{pieces:?}");
}

#[test]
fn clicks_8_s_apart_make_two_pieces() {
    let pieces = cut(&planned(&APART, SPAN));
    assert_eq!(pieces.len(), 2, "{pieces:?}");
    assert!(pieces[1].0 > pieces[0].1, "{pieces:?}");
    assert!(pieces[0].0 < 2 * S && pieces[0].1 > 2_800_000, "{pieces:?}");
    assert!(
        pieces[1].0 < 10 * S && pieces[1].1 > 10_500_000,
        "{pieces:?}"
    );
}

#[test]
fn a_piece_arrives_0_3_s_before_its_first_click() {
    let p = planned(&NEAR, SPAN);
    let pieces = cut(&p);
    assert!(pieces[0].0 <= 1_700_000, "{pieces:?}");
    // Settled by then: the view the click sees.
    let at_click = p.view_at(2 * S);
    assert!(at_click.scale > 1.0, "{at_click:?}");
    assert_eq!(p.view_at(1_700_000), at_click);
}

#[test]
fn a_piece_holds_at_least_1_2_s_after_its_last_click() {
    let p = planned(&NEAR, SPAN);
    let pieces = cut(&p);
    assert!(pieces[0].1 >= 4 * S, "{pieces:?}");
    assert_eq!(p.view_at(4 * S), p.view_at(2_800_000), "still holding");
}

#[test]
fn every_click_sits_inside_the_view_with_a_15_percent_margin() {
    let p = planned(&APART, SPAN);
    for &(t, x, y) in &APART {
        let v = p.view_at(t);
        assert!(v.scale > 1.0, "zoomed at {t}: {v:?}");
        let r = v.rect();
        let (mx, my) = (0.15 * r.w, 0.15 * r.h);
        let eps = 1e-6;
        assert!(
            r.x + mx - eps <= x && x <= r.x + r.w - mx + eps,
            "x of the click at {t} in {r:?}"
        );
        assert!(
            r.y + my - eps <= y && y <= r.y + r.h - my + eps,
            "y of the click at {t} in {r:?}"
        );
    }
}

#[test]
fn the_view_never_zooms_past_2x() {
    let most = |p: &Plan| (p.keys().iter()).fold(1.0_f64, |m, k| m.max(k.view.scale));
    let p = planned(&APART, SPAN);
    assert!(most(&p) > 1.0 && most(&p) <= 2.0 + 1e-9, "{}", most(&p));

    // A click every 0.5 s from 1 s to 31 s, wandering across the screen.
    let busy: Vec<(Us, f32, f32)> = (0..=60)
        .map(|i| {
            let x = 0.25 + 0.5 * i as f32 / 60.0;
            let y = if i % 2 == 0 { 0.40 } else { 0.46 };
            (S + i * S / 2, x, y)
        })
        .collect();
    let p = planned(&busy, (0, 40 * S));
    assert!(most(&p) > 1.0 && most(&p) <= 2.0 + 1e-9, "{}", most(&p));
    assert_eq!(cut(&p).len(), 1, "{:?}", cut(&p));
}

#[test]
fn a_click_right_at_the_start_still_zooms() {
    let pieces = cut(&planned(&[(100_000, 0.30, 0.40)], SPAN));
    assert_eq!(pieces.len(), 1, "{pieces:?}");
    assert!((0..=100_000).contains(&pieces[0].0), "{pieces:?}");
}

#[test]
fn a_click_at_the_very_start_still_zooms() {
    // No time to arrive before it: the whole frame shows it as it happens, and the zoom arrives as soon as it can.
    let p = planned(&[(0, 0.30, 0.40)], SPAN);
    let pieces = cut(&p);
    assert_eq!(pieces.len(), 1, "{pieces:?}");
    assert_eq!(pieces[0].0, 0);
    assert_eq!(p.view_at(0), View::FULL);
    assert!(
        p.view_at(1_200_000).scale > 1.0,
        "{:?}",
        p.view_at(1_200_000)
    );
}

#[test]
fn a_take_shorter_than_one_move_gets_no_zoom() {
    let f = clicks(&[(100_000, 0.30, 0.40)]);
    let spans = spans_of(&f, (0, 300_000));
    assert_eq!(spans, vec![]);
    let mut mint = || -> String { unreachable!("no item, no id") };
    assert_eq!(auto_zoom_ops(&spans, AUTO_SCALE, CLICKS, &mut mint), vec![]);
}

#[test]
fn a_click_outside_the_frame_zooms_at_its_edge() {
    // Off the top right corner (another display, or outside the window in one app's recording): framed against the
    // corner it is nearest, at the full 2x.
    let p = planned(&[(2 * S, 1.2, -0.1)], SPAN);
    assert_eq!(cut(&p).len(), 1, "{:?}", cut(&p));
    let v = p.view_at(2 * S);
    assert_eq!(v.scale, 2.0, "{v:?}");
    let r = v.rect();
    assert!((r.x + r.w - 1.0).abs() < 1e-6 && r.y.abs() < 1e-6, "{r:?}");
}

#[test]
fn a_click_on_the_last_frame_adds_nothing_past_the_end() {
    // The camera holds a shot through its click, then needs a whole move (at least 0.35 s) to reach the whole frame
    // by the end: a click 10 ms before the end can't have both, so it is dropped, never zoomed past the end.
    let p = planned(&[(4_990_000, 0.30, 0.40)], (0, 5 * S));
    assert_eq!(cut(&p), vec![]);
    assert_eq!(p.ladder().dropped, 1);
}

#[test]
fn a_click_0_4_s_before_the_end_still_zooms() {
    let p = planned(&[(4_600_000, 0.30, 0.40)], (0, 5 * S));
    let pieces = cut(&p);
    assert_eq!(pieces.len(), 1, "{pieces:?}");
    assert!(pieces[0].1 <= 5 * S, "{pieces:?}");
    assert!(
        p.view_at(4_600_000).scale > 1.0,
        "{:?}",
        p.view_at(4_600_000)
    );
}

#[test]
fn no_clicks_no_pieces_no_ops() {
    let f = TimelineFacts {
        dwells: vec![SpanAt {
            from_us: 5 * S,
            to_us: 8 * S,
            x: 0.30,
            y: 0.40,
        }],
        ..Default::default()
    };
    let p = auto_plan(&f, CLICKS, SPAN, Rules::default());
    let spans = auto_spans(&p, &[], gap());
    assert_eq!(spans, vec![]);
    let mut mint = || -> String { unreachable!("no item, no id") };
    assert_eq!(auto_zoom_ops(&spans, AUTO_SCALE, CLICKS, &mut mint), vec![]);
}

#[test]
fn a_taken_span_is_left_alone() {
    let p = planned(&APART, SPAN);
    let all = auto_spans(&p, &[], gap());
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all, cut(&p));
    assert_eq!(auto_spans(&p, &[(9 * S, 12 * S)], gap()), vec![all[0]]);
    // In any order: one overlapping the first piece's end takes it.
    let late_first = [(20 * S, 21 * S), (5 * S, 6 * S)];
    assert_eq!(auto_spans(&p, &late_first, gap()), vec![all[1]]);
    // Spans that only touch a piece take nothing.
    let touching = [(all[1].1, all[1].1 + S), (all[0].1, all[1].0)];
    assert_eq!(auto_spans(&p, &touching, gap()), all);

    // A take's spans are checked as the items they become: the last, ended after its hold, misses a zoom at 15 s.
    let take =
        |taken: &[(Us, Us)]| take_spans(&clicks(&APART), CLICKS, SPAN, taken, Rules::default());
    let shorter = take(&[]);
    assert!(shorter[1].1 < 15 * S, "{shorter:?}");
    assert_eq!(take(&[(15 * S, 18 * S)]), shorter);
    assert_eq!(take(&[(12 * S, 14 * S)]), vec![shorter[0]]);
}

#[test]
fn ops_are_attached_auto_zoom_items_at_2x() {
    let spans = [(S, 4 * S), (9 * S, 12 * S)];
    let mut n = 0;
    let mut mint = || {
        n += 1;
        format!("zoom-{n}")
    };
    let ops = auto_zoom_ops(&spans, AUTO_SCALE, CLICKS, &mut mint);
    assert_eq!(ops.len(), 2);
    let whole = Layout::new(
        Anchor::Center,
        BoxN {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
    );
    for (i, (op, &(from, to))) in ops.iter().zip(&spans).enumerate() {
        let Op::AddItem {
            track,
            item,
            attach,
        } = op
        else {
            panic!("{op:?}")
        };
        assert_eq!(*track, TrackTarget::Auto);
        assert!(*attach);
        assert_eq!(item.id, format!("zoom-{}", i + 1));
        assert_eq!((item.start_us, item.end_us()), (from, to));
        assert_eq!(item.attach, None);
        assert_eq!(item.layout, whole);
        assert_eq!(
            item.content,
            ItemContent::Zoom {
                target: ZoomTarget::Auto {
                    using: vec![FocusKind::Clicks]
                },
                scale: 2.0,
            }
        );
        assert!(item.effects.is_empty());
    }

    // The kinds as the item keeps them: sorted, each once, and none for all three (every kind).
    let using_of = |using: &[FocusKind]| {
        let ops = auto_zoom_ops(&spans[..1], AUTO_SCALE, using, &mut || "z".into());
        match &ops[0] {
            Op::AddItem { item, .. } => match &item.content {
                ItemContent::Zoom {
                    target: ZoomTarget::Auto { using },
                    ..
                } => using.clone(),
                c => panic!("{c:?}"),
            },
            op => panic!("{op:?}"),
        }
    };
    use FocusKind::{Clicks, Pauses, Typing};
    assert_eq!(using_of(&[Typing, Clicks, Typing]), vec![Clicks, Typing]);
    assert_eq!(using_of(&[Typing, Pauses, Clicks, Pauses]), vec![]);
}

#[test]
fn spans_of_no_length_make_no_items() {
    let mut n = 0;
    let mut mint = || {
        n += 1;
        format!("zoom-{n}")
    };
    let spans = [(S, S), (3 * S, 2 * S), (4 * S, 5 * S)];
    let ops = auto_zoom_ops(&spans, AUTO_SCALE, CLICKS, &mut mint);
    assert_eq!(ops.len(), 1, "{ops:?}");
    let Op::AddItem { item, .. } = &ops[0] else {
        panic!("{ops:?}")
    };
    assert_eq!(
        (item.id.as_str(), item.start_us, item.duration_us),
        ("zoom-1", 4 * S, S)
    );
}

#[test]
fn the_last_zoom_ends_after_its_hold_not_at_the_take_end() {
    let p = planned(&LATE, (0, 60 * S));
    // The camera holds its last shot until the take's end...
    let held = cut(&p);
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].1, 60 * S);
    // ...but the item ends one move (1.5 s at most) after the hold that follows the last click (1.5 s), so the
    // rest of the take shows the whole frame.
    let spans = spans_of(&clicks(&LATE), (0, 60 * S));
    assert_eq!(spans.len(), 1, "{spans:?}");
    assert_eq!(spans[0].0, held[0].0);
    assert!((16_200_000..=18 * S).contains(&spans[0].1), "{spans:?}");
    // The same for the second of two zooms 8 s apart, over 20 s: its hold ends at 12 s.
    let spans = spans_of(&clicks(&APART), SPAN);
    assert!((11_700_000..=13_500_000).contains(&spans[1].1), "{spans:?}");
}

#[test]
fn a_take_ending_soon_after_its_last_click_zooms_out_at_its_end() {
    // The hold ends at 4.3 s and the move out takes about 1.1 s: by 6 s, less than the 2.5 s gap is left.
    let p = planned(&NEAR, (0, 6 * S));
    assert_eq!(
        spans_of(&clicks(&NEAR), (0, 6 * S)),
        vec![(cut(&p)[0].0, 6 * S)]
    );
    // By 8 s, more than the gap is left: it zooms out after its hold.
    let spans = spans_of(&clicks(&NEAR), (0, 8 * S));
    assert!(spans[0].1 < 6 * S, "{spans:?}");
}

#[test]
fn the_engine_draws_each_item_as_the_whole_plan_at_every_click() {
    let takes = [
        (clicks(&APART), SPAN),
        (clicks(&LATE), (0, 60 * S)),
        (apart_with_a_rest(), SPAN),
    ];
    for (f, span) in &takes {
        let whole = auto_plan(f, CLICKS, *span, Rules::default());
        let spans = spans_of(f, *span);
        for t in f.clicks.iter().map(|c| c.timeline_us) {
            let Some(&(a, b)) = spans.iter().find(|s| (s.0..s.1).contains(&t)) else {
                panic!("the click at {t} is in no item: {spans:?}");
            };
            let item = on_its_own(f, (a, b));
            assert_eq!(item.view_at(t - a), whole.view_at(t), "the click at {t}");
            assert!(whole.view_at(t).scale > 1.0, "zoomed at {t}");
        }
        for &(a, b) in &spans {
            let item = on_its_own(f, (a, b));
            // The same path until its last hold ends, and the whole frame by its end.
            let hold = (whole.shots().iter())
                .filter(|s| (a..b).contains(&s.from))
                .map(|s| s.to)
                .max()
                .unwrap();
            for t in (a..hold).step_by(50_000) {
                assert_eq!(item.view_at(t - a), whole.view_at(t), "at {t} in {a}..{b}");
            }
            assert_eq!(item.view_at(b - a), View::FULL);
        }
    }
}

#[test]
fn a_rest_in_the_shorter_move_out_keeps_the_zoom_to_the_end() {
    // Ended after its hold, the second item would zoom out past the rest at the right edge from 12.5 s; its own
    // camera would then drop the shot and draw no zoom at all. So it keeps the take's end, as the whole plan holds it.
    let f = apart_with_a_rest();
    let spans = spans_of(&f, SPAN);
    assert_eq!(spans.len(), 2, "{spans:?}");
    let item = on_its_own(&f, spans[1]);
    for t in [10 * S, 10_500_000] {
        let v = item.view_at(t - spans[1].0);
        assert!(v.scale > 1.0, "zoomed at {t}: {v:?}");
    }
    assert_eq!(spans[1].1, SPAN.1);
}
