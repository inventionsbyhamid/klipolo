//! The camera's focus from timeline facts: the one function the engine's Auto zooms and chat's
//! zoom tools both plan from.

use klipolo_cursor::Rect01;
use klipolo_cursor::camera::{Focus, Weight, focus_from};
use klipolo_cursor::timeline::{ClickAt, RegionAt, SpanAt, TimelineFacts};
use klipolo_model::cursor_style::FocusKind;

fn facts() -> TimelineFacts {
    let rect = Rect01 {
        x: 0.2,
        y: 0.2,
        w: 0.4,
        h: 0.3,
    };
    TimelineFacts {
        clicks: vec![ClickAt {
            timeline_us: 2_000_000,
            x: 0.5,
            y: 0.5,
        }],
        dwells: vec![SpanAt {
            from_us: 4_000_000,
            to_us: 5_000_000,
            x: 0.1,
            y: 0.9,
        }],
        typing: vec![RegionAt {
            from_us: 6_000_000,
            to_us: 9_000_000,
            rect: Rect01 {
                w: 0.1,
                h: 0.05,
                ..rect
            },
        }],
        media: vec![RegionAt {
            from_us: 10_000_000,
            to_us: 30_000_000,
            rect,
        }],
        ..TimelineFacts::default()
    }
}

#[test]
fn using_pauses_only_gives_dwells() {
    let f = facts();
    let all = focus_from(&f, &[], 0, 60_000_000);
    let weights: Vec<Weight> = all.iter().map(|f| f.weight).collect();
    assert_eq!(weights, [Weight::Click, Weight::Dwell, Weight::Typing]);
    let pauses = focus_from(&f, &[FocusKind::Pauses], 0, 60_000_000);
    assert_eq!(pauses.len(), 1);
    assert_eq!((pauses[0].at, pauses[0].until), (4_000_000, 5_000_000));
    assert_eq!((pauses[0].rect.x, pauses[0].rect.y), (0.1, 0.9));
    assert_eq!(
        focus_from(&f, &[], 3_000_000, 4_500_000).len(),
        1,
        "only what starts in the range"
    );
}

#[test]
fn a_media_region_is_no_focus_unless_clicked() {
    let mut f = facts();
    assert!(
        focus_from(&f, &[], 10_000_000, 60_000_000).is_empty(),
        "playing media alone"
    );
    f.clicks.push(ClickAt {
        timeline_us: 10_500_000,
        x: 0.4,
        y: 0.3,
    });
    let clicked = focus_from(&f, &[], 10_000_000, 60_000_000);
    assert_eq!(
        clicked,
        [Focus {
            at: 10_500_000,
            until: 10_500_000,
            rect: Rect01 {
                x: 0.4,
                y: 0.3,
                w: 0.0,
                h: 0.0
            },
            weight: Weight::Click,
        }],
        "the click is the focus, not the media"
    );
}

#[test]
fn the_kinds_a_narrowed_using_leaves_out_stay_on_screen_through_the_glide() {
    // A smart zoom on clicks only. Typing beside the click, inside the shot's view, while the cursor walks off to the
    // right edge: the glide follows the cursor, but never so far that the typing leaves the view.
    use klipolo_cursor::camera::{Input, Rules, check_plan, keep_from, plan, plan_keeping};
    let f = TimelineFacts {
        clicks: vec![ClickAt {
            timeline_us: 10_000_000,
            x: 0.5,
            y: 0.5,
        }],
        typing: vec![RegionAt {
            from_us: 10_500_000,
            to_us: 11_400_000,
            rect: Rect01 {
                x: 0.3,
                y: 0.5,
                w: 0.01,
                h: 0.01,
            },
        }],
        ..TimelineFacts::default()
    };
    let using = [FocusKind::Clicks];
    let (focus, keep) = (
        focus_from(&f, &using, 0, 30_000_000),
        keep_from(&f, &using, 0, 30_000_000),
    );
    assert_eq!((focus.len(), keep.len()), (1, 1));
    assert!(
        keep_from(&f, &[], 0, 30_000_000).is_empty(),
        "nothing left out"
    );
    let input = Input {
        span: (0, 30_000_000),
        focus: &focus,
        fixed: &[],
        rules: Rules::default(),
    };
    let walk = |t: i64| {
        let u = ((t - 10_000_000) as f64 / 1e6).clamp(0.0, 1.0);
        Some(((0.5 + 0.45 * u) as f32, 0.5))
    };
    let shows = |p: &klipolo_cursor::camera::Plan, t: i64| {
        let r = p.view_at(t).rect();
        (r.x..=r.x + r.w).contains(&0.305)
    };
    let typed = (10_500_000..=11_400_000).step_by(100_000);
    // Without it, the glide drifts off the typing.
    let drifts = plan(&input).with_glide(&walk);
    assert!(typed.clone().any(|t| !shows(&drifts, t)), "the case to fix");
    let kept = plan_keeping(&input, &keep).with_glide(&walk);
    assert_eq!(kept.shots().len(), 1, "the click still zooms");
    for t in typed {
        assert!(
            shows(&kept, t),
            "typing off screen at {t}: {:?}",
            kept.view_at(t)
        );
    }
    assert_eq!(
        kept.ladder().dropped,
        0,
        "left-out kinds aren't dropped focus"
    );
    check_plan(&kept, Rules::default(), 30_000_000).unwrap();
}
