//! The camera's inputs over one Auto zoom's span: the facts of only the clips touching it, which give exactly
//! the focus the whole timeline's give there; the raw cursor with each clip's take looked up once; and a take's facts
//! with its screen files' activity added on the recording clock.

mod common;

use std::sync::Arc;

use common::activity::{Scene, track_of};
use klipolo_cursor::activity::{media, typing};
use klipolo_cursor::camera::{Focus, Input, Rules, focus_from, plan};
use klipolo_cursor::facts::{Dwell, StillRuns};
use klipolo_cursor::format::Line;
use klipolo_cursor::timeline::{RawCursor, TakeFacts, cursor_at, on_timeline, on_timeline_between};
use klipolo_cursor::{Press, Rect01, RegionSpan};
use klipolo_model::cursor_style::{ClickSource, CursorCapture, RecordingCursor};
use klipolo_model::project::{
    Project, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole, SourceRun,
};
use klipolo_model::query::clip_starts;
use klipolo_model::timeline::{Clip, ClipSource};

const S: i64 = 1_000_000;

fn take(id: &str, nudge_us: i64) -> Recording {
    let screen = RecordingSource {
        id: "screen".into(),
        role: SourceRole::Screen,
        label: "capture.source.screen".into(),
        runs: vec![SourceRun {
            asset_id: format!("{id}-screen"),
            offset_us: 0,
            duration_us: 200 * S,
            drift_ppm: 0.0,
            file_in_us: 0,
        }],
        nudge_us,
        mix: SourceMix::default(),
        synced: None,
    };
    Recording {
        id: id.into(),
        kind: RecordingKind::Screen,
        sources: vec![screen],
        primary: "screen".into(),
        cursor: Some(RecordingCursor {
            path: format!("/takes/{id}/cursor.jsonl"),
            capture: CursorCapture::Separate,
            clicks: ClickSource::Events,
        }),
        markers: Vec::new(),
        game: None,
    }
}

fn clip(id: &str, from_s: i64, to_s: i64, speed: f64) -> Clip {
    let mut c = Clip::new(
        ClipSource::Recording { id: id.into() },
        from_s * S,
        to_s * S,
    );
    c.speed = speed;
    c
}

/// r plays 0–10 s, then on at 2× to 30 s (a split: timeline 10–20 s), b-roll (20–25 s), 40–60 s (a cut: 25–45 s), then
/// on to 70 s (45–55 s). Its pauses and typing run across the split and the cut, where pieces join.
fn project() -> (Project, Arc<TakeFacts>) {
    let mut p = Project::new("Spans");
    p.recordings = vec![take("r", 100_000)];
    let broll = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, 5 * S);
    p.timeline.main = vec![
        clip("r", 0, 10, 1.0),
        clip("r", 10, 30, 2.0),
        broll,
        clip("r", 40, 60, 1.0),
        clip("r", 60, 70, 1.0),
    ];
    let press = |s: i64| Press {
        down: s * S / 2,
        up: s * S / 2 + 50_000,
        btn: 0,
        x: 0.25,
        y: 0.75,
    };
    let dwell = |from: i64, to: i64| Dwell {
        from: from * S,
        to: to * S,
        x: 0.3,
        y: 0.6,
    };
    let typed = |from: i64, to: i64| RegionSpan {
        from: from * S,
        to: to * S,
        rect: Rect01 {
            x: 0.1,
            y: 0.7,
            w: 0.05,
            h: 0.02,
        },
    };
    let facts = TakeFacts {
        presses: (1..140).map(press).collect(),
        dwells: vec![dwell(8, 12), dwell(28, 45), dwell(55, 65)],
        still: vec![(0, 200 * S)],
        typing: Some(vec![typed(3, 11), typed(29, 42), typed(58, 63)]),
        media: Some(Vec::new()),
        quiet_screen: Some(Vec::new()),
        ..TakeFacts::default()
    };
    (p, Arc::new(facts))
}

#[test]
fn a_span_gets_the_focus_the_whole_timeline_gives_it() {
    let (p, facts) = project();
    let starts = clip_starts(&p.timeline.main);
    let of = |_: &Recording| Some(facts.clone());
    let whole = on_timeline(&p, &of, S);
    // Every span on a 0.5 s grid, ends on clip edges included (10, 20, 25, 45 s).
    for a in (0..56).map(|k| k * S / 2 + if k % 3 == 0 { 0 } else { 1 }) {
        for b in (a + S..58 * S).step_by(1_500_000) {
            let part = on_timeline_between(&p, &starts, &of, a, b);
            let (mine, all) = (focus_from(&part, &[], a, b), focus_from(&whole, &[], a, b));
            // To the bit, but where a fact runs on past the span: then both end there or later.
            assert_eq!(mine.len(), all.len(), "over [{a}, {b})");
            for (m, w) in mine.iter().zip(&all) {
                let ends = m.until == w.until || (m.until >= b && w.until >= b);
                assert!(
                    ends && (m.at, m.rect, m.weight) == (w.at, w.rect, w.weight),
                    "over [{a}, {b}): {m:?} {w:?}"
                );
            }
            // So the camera plans the same.
            let planned = |focus: &[Focus]| {
                let p = plan(&Input {
                    span: (a, b),
                    focus,
                    fixed: &[],
                    rules: Rules::default(),
                });
                (
                    p.keys().to_vec(),
                    p.shots().to_vec(),
                    p.events().to_vec(),
                    p.ladder(),
                )
            };
            assert_eq!(planned(&mine), planned(&all), "plans over [{a}, {b})");
        }
    }
    // Over the b-roll (20–25 s): the clips touching it, the one ending at 20 s and the one starting at 25 s, and no other.
    let edges = on_timeline_between(&p, &starts, &of, 20 * S, 25 * S);
    let (first, last) = (
        edges.clicks[0].timeline_us,
        edges.clicks.last().unwrap().timeline_us,
    );
    assert!(
        (10 * S..11 * S).contains(&first) && (44 * S + 1..45 * S).contains(&last),
        "{first} {last}"
    );
}

#[test]
fn the_raw_cursor_reads_as_cursor_at_does() {
    let (p, _) = project();
    let starts = clip_starts(&p.timeline.main);
    let mut lines = vec![common::head()];
    for t in (0..=80 * S).step_by(50_000) {
        let v = (t as f32 / (80 * S) as f32).min(1.0);
        lines.push(Line::Move {
            t,
            x: v,
            y: 1.0 - v,
        });
    }
    let track = Arc::new(common::read(&common::file(&lines)));
    let of = |_: &Recording| Some(track.clone());
    let raw = RawCursor::new(&p, &starts, &of, 5 * S, 50 * S);
    for t in (5 * S..50 * S).step_by(16_667) {
        assert_eq!(raw.at(t), cursor_at(&p, &starts, &of, t), "at {t}");
    }
    assert_eq!(raw.at(22 * S), None, "b-roll");
    assert_eq!(raw.at(60 * S), None, "outside the span it was made for");
}

#[test]
fn a_takes_facts_gain_each_screen_files_activity_on_the_recording_clock() {
    // Two screen files: the first 0–8 s, the second (a resume's) from 60 s, each with a line typed 5 s in.
    let scene = Scene {
        typed: 50,
        ..Scene::default()
    };
    let (a, b) = (track_of(&scene, 80), track_of(&scene, 80));
    // The cursor rests throughout (a heartbeat every 500 ms for 80 s).
    let mut lines = vec![common::head()];
    lines.extend((0..=160).map(|k| Line::Move {
        t: k * 500_000,
        x: 0.5,
        y: 0.5,
    }));
    let still = StillRuns::of(&common::read(&common::file(&lines)));
    let facts = TakeFacts::default().with_screens(&still, &[(0, &a), (60 * S, &b)], S);
    let typed = facts.typing.expect("known");
    let first = typing(&a, &still, 0);
    assert!(!first.is_empty());
    assert_eq!(typed.len(), 2 * first.len());
    assert_eq!(typed[first.len()].from, first[0].from + 60 * S);
    assert_eq!(facts.media.expect("known").len(), 2 * media(&a).len());
    let quiet = facts.quiet_screen.expect("known");
    assert!(
        quiet.windows(2).all(|w| w[0].1 <= w[1].0),
        "sorted, disjoint"
    );
    assert!(quiet.iter().any(|q| q.0 >= 60 * S));
    assert_eq!(facts.quiet_sound, None, "not the screen's to say");
    let none = TakeFacts::default().with_screens(&still, &[], S);
    assert_eq!(none.typing, None, "no screen file read: not known");
}
