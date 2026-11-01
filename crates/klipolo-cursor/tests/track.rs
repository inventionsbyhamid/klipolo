mod common;

use common::{file, head, read};
use klipolo_cursor::format::{Capture, Line, write_line};
use klipolo_cursor::{ClickSource, CursorTrack, Limits};

#[test]
fn presses_pair_downs_with_ups_and_one_still_held_at_the_end_stays_open() {
    let t = read(&file(&[
        head(),
        Line::Button {
            t: 1_000,
            btn: 0,
            d: 1,
            x: 0.5,
            y: 0.5,
        },
        Line::Button {
            t: 1_800,
            btn: 0,
            d: 0,
            x: 0.5,
            y: 0.5,
        },
        Line::Button {
            t: 5_000,
            btn: 1,
            d: 1,
            x: 0.2,
            y: 0.2,
        },
    ]));
    assert_eq!(t.presses().len(), 2);
    assert_eq!((t.presses()[0].down, t.presses()[0].up), (1_000, 1_800));
    assert_eq!(t.presses()[1].up, i64::MAX);
    assert_eq!(
        t.clicks().count(),
        1,
        "only the primary button counts as a click"
    );
    assert!(t.held_at(1_500) && !t.held_at(2_000) && t.any_held_at(6_000) && !t.held_at(6_000));
}

#[test]
fn a_button_that_arrives_after_a_later_beat_is_kept() {
    // The monitor's button (stamped with its hardware time) is written after the sampler's later heartbeat.
    let t = read(&file(&[
        head(),
        Line::Move {
            t: 100_000,
            x: 0.5,
            y: 0.5,
        },
        Line::Button {
            t: 85_000,
            btn: 0,
            d: 1,
            x: 0.5,
            y: 0.5,
        },
        Line::Button {
            t: 95_000,
            btn: 0,
            d: 0,
            x: 0.5,
            y: 0.5,
        },
    ]));
    assert_eq!(t.clicks().count(), 1);
    assert_eq!(t.skipped(), 0);
}

#[test]
fn a_line_older_than_its_own_kind_is_skipped_and_counted_and_equal_times_are_kept() {
    let other_head = Line::Header {
        v: 2,
        capture: Capture::Burned,
        clicks: ClickSource::Sampled,
        w: 800.0,
        h: 600.0,
    };
    let t = read(&file(&[
        head(),
        Line::Move {
            t: 10_000,
            x: 0.1,
            y: 0.1,
        },
        Line::Move {
            t: 20_000,
            x: 0.2,
            y: 0.2,
        },
        Line::Move {
            t: 15_000,
            x: 0.9,
            y: 0.9,
        },
        Line::Button {
            t: 30_000,
            btn: 0,
            d: 1,
            x: 0.2,
            y: 0.2,
        },
        Line::Button {
            t: 30_000,
            btn: 0,
            d: 0,
            x: 0.2,
            y: 0.2,
        },
        head(),
        other_head,
    ]));
    assert_eq!(t.skipped(), 1, "only the move older than the last move");
    assert_eq!(t.len(), 2);
    assert_eq!(
        (t.presses()[0].down, t.presses()[0].up),
        (30_000, 30_000),
        "equal times are kept"
    );
    // A repeated header is the same take; a different one is noted, and the first stays.
    assert_eq!(t.header().map(|h| h.capture), Some(Capture::Separate));
    assert_eq!(t.notes().other_headers, 1);
}

#[test]
fn positions_interpolate_between_samples_and_hold_after_the_last() {
    let t = read(&file(&[
        head(),
        Line::Move {
            t: 0,
            x: 0.0,
            y: 0.0,
        },
        Line::Move {
            t: 10_000,
            x: 1.0,
            y: 0.5,
        },
    ]));
    let (x, y) = t.position(5_000).unwrap();
    assert!((x - 0.5).abs() < 1e-4 && (y - 0.25).abs() < 1e-4);
    assert_eq!(t.position(99_000), t.position(10_000));
    assert!(t.position(-1).is_none());
}

#[test]
fn shapes_hidden_spans_clicks_mode_and_the_burned_in_point_answer_by_time() {
    let t = read(&file(&[
        head(),
        Line::ShapeDef {
            id: 0,
            key: "00000000000000ff".into(),
            file: "cursors/0.png".into(),
            pw: 256,
            ph: 256,
            hx: 40.0,
            hy: 30.0,
            pts: 32.0,
            sys: None,
        },
        Line::Shape { t: 0, id: 0 },
        Line::Visible {
            t: 2_000,
            on: false,
        },
        Line::Visible { t: 3_000, on: true },
        Line::Clicks {
            t: 4_000,
            clicks: ClickSource::Sampled,
        },
        Line::Burned { t: 9_000 },
    ]));
    assert_eq!(t.shape_at(1).unwrap().id, 0);
    assert_eq!(t.shape_at(1).unwrap().key, 0xff);
    assert!(!t.visible_at(2_500) && t.visible_at(3_000));
    assert_eq!(
        (t.clicks_at(3_999), t.clicks_at(4_000), t.clicks_weakest()),
        (
            ClickSource::Events,
            ClickSource::Sampled,
            ClickSource::Sampled
        )
    );
    assert!(t.separate_at(8_999) && !t.separate_at(9_000));
}

#[test]
fn a_shape_file_outside_cursors_or_too_large_is_refused() {
    let bad = file(&[
        head(),
        Line::ShapeDef {
            id: 0,
            key: "0000000000000001".into(),
            file: "../../secret".into(),
            pw: 8,
            ph: 8,
            hx: 0.0,
            hy: 0.0,
            pts: 8.0,
            sys: None,
        },
        Line::ShapeDef {
            id: 1,
            key: "0000000000000002".into(),
            file: "cursors/1.png".into(),
            pw: 4096,
            ph: 8,
            hx: 0.0,
            hy: 0.0,
            pts: 8.0,
            sys: None,
        },
    ]);
    assert!(read(&bad).shapes().is_empty());
}

#[test]
fn a_version_one_file_keeps_its_positions_and_presses() {
    // The first recorder builds wrote one line per frame, the buttons held as bits: takes recorded then keep their
    // cursor. These are that writer's exact lines.
    let v1 = concat!(
        r#"{"t":0,"x":0.1,"y":0.2}"#,
        "\n",
        r#"{"t":16667,"x":0.5,"y":0.25,"b":1}"#,
        "\n",
        r#"{"t":33333,"x":0.5,"y":0.25,"b":1}"#,
        "\n",
        r#"{"t":50000,"x":0.6,"y":0.25}"#,
        "\n",
        r#"{"t":66667,"x":0.6"#,
    );
    let t = read(v1.as_bytes());
    assert_eq!(t.len(), 4);
    let (x, y) = t.position(50_000).unwrap();
    assert!((x - 0.6).abs() < 1e-4 && (y - 0.25).abs() < 1e-4);
    assert_eq!(
        t.clicks().map(|p| (p.down, p.up)).collect::<Vec<_>>(),
        [(16_667, 50_000)]
    );
    assert!(
        t.header().is_none() && !t.separate_at(0),
        "format-1 takes show the cursor in their frames"
    );
    assert_eq!(t.clicks_weakest(), ClickSource::Sampled);
}

#[test]
fn positions_cost_eight_bytes_each() {
    let mut bytes = Vec::new();
    write_line(&head(), &mut bytes);
    for i in 0..10_000i64 {
        write_line(
            &Line::Move {
                t: i * 4_000,
                x: 0.5,
                y: (i % 7) as f32 / 7.0,
            },
            &mut bytes,
        );
    }
    let t = read(&bytes);
    assert_eq!(t.len(), 10_000);
    assert_eq!(t.position_bytes(), 8 * t.len());
}

#[test]
fn a_long_file_stops_at_its_limit_and_keeps_every_press() {
    // A minute of motion with presses, read with a 64 KB limit: positions stop, presses don't.
    let bytes = common::bytes_of(&common::with_presses(common::random_path(5, 60_000_000)));
    let whole = read(&bytes);
    let limits = Limits {
        max_bytes: 64 * 1024,
        ..Limits::for_file_len(bytes.len() as u64)
    };
    let t = CursorTrack::read(&bytes[..], limits);
    assert!(t.bytes() <= 64 * 1024, "{}", t.bytes());
    assert!(
        t.notes().limited && t.len() < whole.len(),
        "{} of {}",
        t.len(),
        whole.len()
    );
    assert!(whole.presses().len() > 20, "{}", whole.presses().len());
    assert_eq!(t.presses(), whole.presses());
}

#[test]
#[ignore = "real scale: three hours at 250 Hz (about 110 MB of lines)"]
fn a_three_hour_file_at_250_hz_stays_under_16_mb_and_keeps_every_press() {
    let mut bytes = Vec::new();
    write_line(&head(), &mut bytes);
    let n = 3 * 3_600 * 250i64;
    for i in 0..n {
        let t = i * 4_000;
        write_line(
            &Line::Move {
                t,
                x: (i % 997) as f32 / 997.0,
                y: 0.5,
            },
            &mut bytes,
        );
        if i % 500 == 0 {
            write_line(
                &Line::Button {
                    t,
                    btn: 0,
                    d: 1,
                    x: 0.5,
                    y: 0.5,
                },
                &mut bytes,
            );
        }
        if i % 500 == 12 {
            // the up in time order
            write_line(
                &Line::Button {
                    t,
                    btn: 0,
                    d: 0,
                    x: 0.5,
                    y: 0.5,
                },
                &mut bytes,
            );
        }
    }
    let started = std::time::Instant::now();
    let t = read(&bytes);
    let took = started.elapsed();
    println!(
        "three hours at 250 Hz: {} MB of lines read in {took:?}: {} positions, {} bytes",
        bytes.len() / 1_000_000,
        t.len(),
        t.bytes()
    );
    assert!(t.bytes() <= CursorTrack::MAX_BYTES, "{}", t.bytes());
    assert_eq!(t.presses().len() as i64, n / 500);
}
