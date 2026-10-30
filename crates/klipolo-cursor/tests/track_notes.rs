//! What a read keeps and tells beyond the lines themselves: one sample a time,
//! impossible values and times refused and counted, the header count and each kind's last time.

mod common;

use common::{file, head, read};
use klipolo_cursor::format::{Capture, Line, TIME_LIMIT_US, parse_line, write_line};
use klipolo_cursor::smooth::SmoothedPath;
use klipolo_cursor::{ClickSource, LastTimes};
use klipolo_model::cursor_style::Smoothing;

fn mv(t: i64, x: f32) -> Line {
    Line::Move { t, x, y: 0.5 }
}

fn btn(t: i64, btn: u8, d: u8) -> Line {
    Line::Button {
        t,
        btn,
        d,
        x: 0.5,
        y: 0.5,
    }
}

fn shape(id: u16, hotspot: (f32, f32), pts: f32) -> Line {
    Line::ShapeDef {
        id,
        key: format!("{id:016x}"),
        file: format!("cursors/{id}.png"),
        pw: 32,
        ph: 32,
        hx: hotspot.0,
        hy: hotspot.1,
        pts,
        sys: None,
    }
}

#[test]
fn samples_never_share_a_time() {
    // Two moves at one time, and a rest point 50 µs after a kept move: each lands in the tick before it.
    let t = read(&file(&[
        head(),
        mv(0, 0.1),
        mv(50, 0.2),
        mv(600_000, 0.5),
        mv(600_000, 0.6),
        mv(900_000, 0.7),
    ]));
    let times: Vec<i64> = t.samples(i64::MIN, i64::MAX).map(|s| s.0).collect();
    assert!(times.windows(2).all(|w| w[0] < w[1]), "{times:?}");
    assert_eq!(times, [0, 600_000, 900_000]);
    let (x, _) = t.position(0).unwrap();
    assert!(
        (x - 0.2).abs() < 1e-4,
        "the later position in a tick is kept: {x}"
    );
    let (x, _) = t.position(600_000).unwrap();
    assert!((x - 0.6).abs() < 1e-4, "{x}");
}

#[test]
fn impossible_sizes_and_hotspots_are_refused_and_counted() {
    let zero_area = Line::Header {
        v: 2,
        capture: Capture::Separate,
        clicks: ClickSource::Events,
        w: 0.0,
        h: 900.0,
    };
    let bad_area = Line::Area {
        t: 10,
        w: -1.0,
        h: 600.0,
    };
    let lines = [
        zero_area,
        bad_area,
        shape(0, (33.0, 4.0), 16.0),
        shape(1, (4.0, -0.5), 16.0),
        shape(2, (4.0, 4.0), 0.0),
        shape(3, (32.0, 32.0), 16.0),
    ];
    let t = read(&file(&lines));
    let head = t
        .header()
        .expect("the header is kept: it says the cursor is separate");
    assert_eq!(
        (head.capture, head.area_pts),
        (Capture::Separate, (0.0, 0.0))
    );
    assert_eq!(t.area_at(20), (0.0, 0.0), "unknown");
    assert_eq!(
        t.shapes().iter().map(|s| s.id).collect::<Vec<_>>(),
        [3],
        "a hotspot on the image's edge is inside it"
    );
    assert_eq!(t.notes().refused, 5);
    assert!(t.notes().lost_any());
    for line in &lines[1..5] {
        let mut out = Vec::new();
        klipolo_cursor::format::write_line(line, &mut out);
        assert_eq!(parse_line(&out[..out.len() - 1]), None, "{line:?}");
    }
}

#[test]
fn a_header_written_before_the_area_is_known_is_unknown_and_not_refused() {
    // The recorder writes 0 × 0 when its picture's size isn't known yet, then an `a` line.
    let unknown = Line::Header {
        v: 2,
        capture: Capture::Separate,
        clicks: ClickSource::Events,
        w: 0.0,
        h: 0.0,
    };
    let area = Line::Area {
        t: 1_000,
        w: 1440.0,
        h: 900.0,
    };
    let t = read(&file(&[unknown, area, mv(2_000, 0.5)]));
    assert_eq!(t.header().map(|h| h.area_pts), Some((0.0, 0.0)));
    assert_eq!(t.area_at(1_500), (1440.0, 900.0));
    assert!(!t.notes().lost_any(), "{:?}", t.notes());
}

#[test]
fn each_kinds_last_time_and_the_header_count_are_read() {
    let t = read(&file(&[
        head(),
        mv(1_000, 0.1),
        mv(9_000, 0.2),
        mv(5_000, 0.3), // out of order: skipped, and not the last
        btn(2_000, 0, 1),
        btn(7_000, 3, 1),
        btn(3_000, 0, 0),
        Line::Shape { t: 4_000, id: 0 },
        Line::Visible {
            t: 6_000,
            on: false,
        },
        Line::Area {
            t: 6_500,
            w: 800.0,
            h: 600.0,
        },
        Line::Clicks {
            t: 8_000,
            clicks: ClickSource::Sampled,
        },
        head(),
    ]));
    assert_eq!(
        t.last_times(),
        LastTimes {
            moves: Some(9_000),
            buttons: Some(7_000),
            shapes: Some(4_000),
            visible: Some(6_000),
            areas: Some(6_500),
            clicks: Some(8_000),
            burned: None,
        }
    );
    assert_eq!((t.notes().headers, t.notes().skipped), (2, 1));
    assert_eq!(read(&file(&[mv(0, 0.5)])).last_times().buttons, None);
}

#[test]
fn times_outside_a_sane_window_are_refused_so_no_time_sum_overflows() {
    // Positions just before i64::MAX would overflow the smoothing's time sums (a panic in debug).
    let far = i64::MAX - 100_000;
    let mut lines = vec![head()];
    lines.extend((0..10).map(|i| mv(far + i * 4_000, 0.1 + i as f32 * 0.05)));
    lines.extend([btn(far, 0, 1), Line::Burned { t: i64::MIN }]);
    let mut bytes = file(&lines);
    bytes.extend_from_slice(format!(r#"{{"t":{far},"x":0.5,"y":0.5,"b":1}}"#).as_bytes()); // format 1
    let t = read(&bytes);
    assert_eq!((t.notes().refused, t.len(), t.presses().len()), (13, 0, 0));
    for s in [Smoothing::Light, Smoothing::Strong] {
        assert_eq!(SmoothedPath::build(&t, s).bytes(), 0);
    }
    let parsed = |t| {
        let mut out = Vec::new();
        write_line(&mv(t, 0.5), &mut out);
        parse_line(&out[..out.len() - 1])
    };
    assert!(parsed(TIME_LIMIT_US).is_some() && parsed(-TIME_LIMIT_US).is_some());
    assert!(parsed(TIME_LIMIT_US + 1).is_none());
}
