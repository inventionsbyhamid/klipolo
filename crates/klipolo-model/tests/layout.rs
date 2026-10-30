use klipolo_model::layout::*;
use klipolo_model::project::FormatKind::*;

fn close(a: BoxN, b: BoxN) {
    for (x, y) in [(a.x, b.x), (a.y, b.y), (a.w, b.w), (a.h, b.h)] {
        assert!((x - y).abs() < 1e-4, "{a:?} != {b:?}");
    }
}

#[test]
fn same_format_returns_rect() {
    let l = Layout::new(
        Anchor::TopLeft,
        BoxN {
            x: 0.05,
            y: 0.05,
            w: 0.4,
            h: 0.1,
        },
    );
    close(resolve_rect(&l, Landscape, Landscape), l.rect);
}

#[test]
fn top_left_title_keeps_margins_in_portrait() {
    let l = Layout::new(
        Anchor::TopLeft,
        BoxN {
            x: 0.05,
            y: 0.05,
            w: 0.4,
            h: 0.1,
        },
    );
    close(
        resolve_rect(&l, Landscape, Portrait),
        BoxN {
            x: 0.088889,
            y: 0.028125,
            w: 0.711111,
            h: 0.05625,
        },
    );
}

#[test]
fn bottom_center_caption_clamps_width_in_portrait() {
    let l = Layout::new(
        Anchor::BottomCenter,
        BoxN {
            x: 0.1,
            y: 0.78,
            w: 0.8,
            h: 0.12,
        },
    );
    close(
        resolve_rect(&l, Landscape, Portrait),
        BoxN {
            x: 0.0,
            y: 0.87625,
            w: 1.0,
            h: 0.0675,
        },
    );
}

#[test]
fn override_wins() {
    let mut l = Layout::new(
        Anchor::Center,
        BoxN {
            x: 0.4,
            y: 0.4,
            w: 0.2,
            h: 0.2,
        },
    );
    let manual = BoxN {
        x: 0.1,
        y: 0.2,
        w: 0.3,
        h: 0.1,
    };
    l.overrides.insert(Portrait, manual);
    close(resolve_rect(&l, Landscape, Portrait), manual);
}

#[test]
fn anchors_follow_rect_centre() {
    assert_eq!(
        anchor_for_rect(&BoxN {
            x: 0.0,
            y: 0.0,
            w: 0.2,
            h: 0.1
        }),
        Anchor::TopLeft
    );
    assert_eq!(
        anchor_for_rect(&BoxN {
            x: 0.4,
            y: 0.45,
            w: 0.2,
            h: 0.1
        }),
        Anchor::Center
    );
    assert_eq!(
        anchor_for_rect(&BoxN {
            x: 0.75,
            y: 0.8,
            w: 0.2,
            h: 0.1
        }),
        Anchor::BottomRight
    );
    assert_eq!(
        anchor_for_rect(&BoxN {
            x: 0.3,
            y: 0.85,
            w: 0.4,
            h: 0.1
        }),
        Anchor::BottomCenter
    );
}
