//! The cursor's place and time for every OS's source: a point mapped into the captured
//! area as it is at that moment, and an `NSEvent` timestamp on the host clock. Pure, so every OS runs them.

use klipolo_capture::platform::common::cursor_map::{ns_seconds_to_host_ns, to_area};

#[test]
fn a_point_maps_into_a_moving_window() {
    // The same cursor, still at (500, 300) global points, while the window under it moves right by 200 points.
    let at = (500.0, 300.0);
    assert_eq!(to_area(at, (400.0, 200.0, 400.0, 400.0)), (0.25, 0.25));
    assert_eq!(
        to_area(at, (600.0, 200.0, 400.0, 400.0)),
        (-0.25, 0.25),
        "now left of the window: outside 0-1"
    );
    // A window with no size (closed, minimised) puts every point outside.
    assert_eq!(to_area(at, (600.0, 200.0, 0.0, 400.0)), (-1.0, -1.0));
    assert_eq!(to_area(at, (f64::NAN, 200.0, 400.0, 400.0)), (-1.0, -1.0));
}

#[test]
fn the_cursor_is_normalised_to_the_captured_area() {
    // On `to_area`: a 1440 × 900 point display at the global origin, top-left origin.
    let display = (0.0, 0.0, 1440.0, 900.0);
    assert_eq!(to_area((720.0, 450.0), display), (0.5, 0.5));
    assert_eq!(to_area((0.0, 0.0), display), (0.0, 0.0));
    // A second display to the right: its own top left is 0, 0 in its picture.
    assert_eq!(
        to_area((1440.0 + 960.0, 540.0), (1440.0, 0.0, 1920.0, 1080.0)),
        (0.5, 0.5)
    );
}

#[test]
fn event_seconds_become_host_ns() {
    assert_eq!(ns_seconds_to_host_ns(12.000_000_5), 12_000_000_500);
    assert_eq!(ns_seconds_to_host_ns(0.0), 0);
    assert_eq!(ns_seconds_to_host_ns(-1.0), 0);
    assert_eq!(ns_seconds_to_host_ns(f64::NAN), 0);
    assert_eq!(ns_seconds_to_host_ns(f64::INFINITY), u64::MAX);
}
