//! The cursor file's size: what the writer writes an hour for smooth synthetic motion and for
//! real pointer motion, and that its thinning loses nothing the reader keeps.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::Harness;
use klipolo_capture::cursor::CursorEvent as E;
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_cursor::ClickSource;

fn ns(h: &Harness, at_s: f64) -> u64 {
    h.base + (at_s * 1e9) as u64
}

#[test]
fn the_writer_stays_within_its_disk_budget() {
    use common::harness::cursor::{
        continuous, cursor_bytes_per_hour, real_mix, real_motion, typical_mix,
    };
    // Smooth synthetic paths (a circle at 250 Hz; straight runs in the typical mix) thin a lot.
    let smooth = cursor_bytes_per_hour(continuous(250.0, 60.0));
    let smooth_typical = cursor_bytes_per_hour(typical_mix(60.0));
    // Real pointer motion (accelerating flicks on whole and half points) thins only where it slows: never still for the
    // worst case, which stays under the 40 MB an hour (the format's floor, every move written, is about 41.9); moving
    // 30 % of the time for the typical one.
    let real = cursor_bytes_per_hour(real_motion(250.0, 60.0));
    let real_typical = cursor_bytes_per_hour(real_mix(60.0));
    println!(
        "cursor bytes an hour: smooth 250 Hz {smooth:.0}, smooth typical {smooth_typical:.0}, \
         real 250 Hz {real:.0}, real typical {real_typical:.0}"
    );
    assert!(smooth <= 40.0e6 && real <= 40.0e6, "{smooth} {real}");
    assert!(
        smooth_typical <= 8.0e6 && real_typical <= 8.0e6,
        "{smooth_typical} {real_typical}"
    );
}

#[test]
fn thinning_loses_nothing_the_reader_keeps() {
    use common::harness::cursor::{continuous, real_motion};
    // Every move the writer dropped is reproduced by the reader's interpolation within the reader's own rounding: half a
    // step for the thinning, half for the chord's ends and half for the move itself. Smooth motion, real pointer motion,
    // and a constant-velocity run across the reader's edges, each 250 Hz through a take.
    let (thin, whole) = lossless(&continuous(250.0, 3.0));
    assert!(thin * 3 < whole * 2, "{thin} of {whole} bytes");
    lossless(&real_motion(250.0, 3.0));
    let across = |from: f32, to: f32| -> Vec<(f64, E)> {
        (0..250)
            .map(|k| {
                let (t, x) = (f64::from(k) / 250.0, from + (to - from) * k as f32 / 250.0);
                (
                    t,
                    E::Move {
                        host_ns: 0,
                        x,
                        y: 0.5,
                    },
                )
            })
            .collect()
    };
    // Crossing between two gathers (each writes its newest point), so a chord spans the edge.
    lossless(&across(1.213, 1.813));
    lossless(&across(0.213, -0.787));
}

/// `script`'s moves through a take, against the same moves written whole: the two files' bytes, once every move's
/// read was checked within the bound.
fn lossless(script: &[(f64, E)]) -> (usize, usize) {
    use klipolo_cursor::format::{Capture, Line, write_line};
    use klipolo_cursor::{CursorTrack, Limits};
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    let place = |v: f32| ((f64::from(v) * 1e5).round() / 1e5) as f32;
    let mut whole = Vec::new();
    let header = Line::Header {
        v: 2,
        capture: Capture::Separate,
        clicks: ClickSource::Events,
        w: 1440.0,
        h: 900.0,
    };
    write_line(&header, &mut whole);
    for (t, ev) in script {
        let E::Move { x, y, .. } = *ev else {
            unreachable!()
        };
        h.cursor(
            *t,
            E::Move {
                host_ns: ns(&h, *t),
                x,
                y,
            },
        );
        let us = (*t * 1e6).round() as i64;
        write_line(
            &Line::Move {
                t: us,
                x: place(x),
                y: place(y),
            },
            &mut whole,
        );
    }
    let end = script.last().map_or(0.0, |(t, _)| *t);
    h.run_to(end + 0.5);
    let files = h.stopped_files();
    let thin = &files["cursor.jsonl"];
    let read = |b: &[u8]| CursorTrack::read(b, Limits::for_file_len(b.len() as u64));
    let (thin_read, whole_read) = (read(thin), read(&whole));
    let bound = (1.5 * klipolo_cursor::track::QUANTUM + 1e-6) as f32;
    for (t, _) in script {
        let us = (*t * 1e6).round() as i64;
        let (a, b) = (
            thin_read.position(us).unwrap(),
            whole_read.position(us).unwrap(),
        );
        assert!(
            (a.0 - b.0).abs() <= bound && (a.1 - b.1).abs() <= bound,
            "at {us}: {a:?} vs {b:?}"
        );
    }
    (thin.len(), whole.len())
}
