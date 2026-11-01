//! A new recording clip's camera bubble is round in every format, whatever the project's main format: its box is a
//! square of pixels there, 4 % in from the bottom-right corner, and every other format places it from that.

use klipolo_model::layout::resolve_rect;
use klipolo_model::project::FormatKind::{self, Landscape, Portrait, Square};
use klipolo_model::timeline::CameraLayout;

const FORMATS: [FormatKind; 3] = [Landscape, Portrait, Square];

#[test]
fn the_default_bubble_is_round_in_every_format_for_any_main_format() {
    for primary in FORMATS {
        let camera = CameraLayout::for_primary(primary);
        for format in FORMATS {
            let rect = resolve_rect(&camera.bubble, primary, format);
            let (w, h) = format.size();
            let (w, h) = (rect.w * f64::from(w), rect.h * f64::from(h));
            assert!(
                (w - h).abs() < 0.5,
                "{primary:?} project, {format:?}: {w} x {h} px"
            );
            assert!((w - 384.0).abs() < 0.5, "{primary:?}, {format:?}: {w} px");
        }
        // In the main format, 4 % in from the right and bottom edges.
        let r = camera.bubble.rect;
        assert!((1.0 - r.x - r.w - 0.04).abs() < 1e-3, "{primary:?}: {r:?}");
        assert!((1.0 - r.y - r.h - 0.04).abs() < 1e-3, "{primary:?}: {r:?}");
    }
}

#[test]
fn the_default_is_the_bubble_a_16_9_project_starts_with() {
    assert_eq!(
        CameraLayout::for_primary(Landscape),
        CameraLayout::default()
    );
}
