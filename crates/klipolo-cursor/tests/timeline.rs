//! A take's facts on the timeline: through the screen's sync nudge, clip speed and cuts,
//! the one mapping the engine's camera and chat share.

mod common;

use std::sync::Arc;

use klipolo_cursor::facts::Dwell;
use klipolo_cursor::format::Line;
use klipolo_cursor::timeline::{
    ClickAt, MAX_FACTS, RegionAt, SpanAt, TakeFacts, TimelineFacts, cursor_at, on_timeline,
};
use klipolo_cursor::{CursorTrack, Press, Rect01, RegionSpan};
use klipolo_model::cursor_style::{ClickSource, CursorCapture, RecordingCursor};
use klipolo_model::project::{
    Project, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole, SourceRun,
};
use klipolo_model::query::clip_starts;
use klipolo_model::timeline::{Clip, ClipSource};

const S: i64 = 1_000_000;

fn screen_take(id: &str, nudge_us: i64) -> Recording {
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

fn clip(take: &str, from_s: i64, to_s: i64, speed: f64) -> Clip {
    let source = ClipSource::Recording { id: take.into() };
    let mut c = Clip::new(source, from_s * S, to_s * S);
    c.speed = speed;
    c
}

/// r1 (its screen nudged 100 ms later) plays 10–40 s at 1× (timeline 0–30 s) and 60–100 s at 2× (30–50 s), so 40–60 s
/// is cut out; 5 s of b-roll (50–55 s); r2 plays 0–30 s (55–85 s).
fn project() -> Project {
    let mut p = Project::new("Facts");
    p.recordings = vec![screen_take("r1", 100_000), screen_take("r2", 0)];
    let broll = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, 5 * S);
    p.timeline.main = vec![
        clip("r1", 10, 40, 1.0),
        clip("r1", 60, 100, 2.0),
        broll,
        clip("r2", 0, 30, 1.0),
    ];
    p
}

fn press(down: i64) -> Press {
    let (x, y) = (0.25, 0.75);
    Press {
        down,
        up: down + 50_000,
        btn: 0,
        x,
        y,
    }
}

fn dwell(from: i64, to: i64) -> Dwell {
    let (x, y) = (0.25, 0.75);
    Dwell { from, to, x, y }
}

const TYPED: Rect01 = Rect01 {
    x: 0.1,
    y: 0.7,
    w: 0.3,
    h: 0.1,
};

/// r1 has no screen activity or sound measured yet; r2 has both.
fn take_facts() -> (TakeFacts, TakeFacts) {
    let r1 = TakeFacts {
        // 5.1 s plays before the clip, 45.1 s is cut out, and 100.05 s is past the end once nudged.
        presses: [5, 20, 45, 80]
            .map(|s| press(s * S))
            .into_iter()
            .chain([press(99_950_000)])
            .collect(),
        // The second runs into the cut: only its part before 40 s plays.
        dwells: vec![dwell(30 * S, 32 * S), dwell(39 * S, 41 * S)],
        still: vec![(0, 50 * S), (70 * S, i64::MAX)],
        ..TakeFacts::default()
    };
    let r2 = TakeFacts {
        presses: vec![press(5 * S)],
        still: vec![(S, 9 * S), (14 * S, 30 * S)],
        typing: Some(vec![RegionSpan {
            from: 10 * S,
            to: 12 * S,
            rect: TYPED,
        }]),
        media: Some(Vec::new()),
        quiet_screen: Some(vec![(0, 20 * S)]),
        quiet_sound: Some(vec![(2 * S, 8 * S), (15 * S, 25 * S)]),
        ..TakeFacts::default()
    };
    (r1, r2)
}

#[test]
fn facts_land_where_their_clips_play_them() {
    let p = project();
    let (r1, r2) = take_facts();
    let (r1, r2) = (Arc::new(r1), Arc::new(r2));
    let take = |r: &Recording| match r.id.as_str() {
        "r1" => Some(r1.clone()),
        "r2" => Some(r2.clone()),
        _ => None,
    };
    let click = |timeline_us| ClickAt {
        timeline_us,
        x: 0.25,
        y: 0.75,
    };
    let span = |from_us, to_us| SpanAt {
        from_us,
        to_us,
        x: 0.25,
        y: 0.75,
    };
    let expected = TimelineFacts {
        clicks: vec![click(10_100_000), click(40_050_000), click(60 * S)],
        dwells: vec![span(20_100_000, 22_100_000), span(29_100_000, 30 * S)],
        typing: vec![RegionAt {
            from_us: 65 * S,
            to_us: 67 * S,
            rect: TYPED,
        }],
        media: Vec::new(),
        // Only r2 knows its screen and sound: 2–8 s and 15–20 s of it.
        idle: vec![(57 * S, 63 * S), (70 * S, 75 * S)],
        activity_known: false,
        sound_known: false,
    };
    assert_eq!(on_timeline(&p, &take, S), expected);

    // Once r1's activity and sound are measured (all busy), every take knows them.
    let mut known = take_facts().0;
    known.typing = Some(Vec::new());
    known.media = Some(Vec::new());
    known.quiet_screen = Some(Vec::new());
    known.quiet_sound = Some(Vec::new());
    let known = Arc::new(known);
    let take = |r: &Recording| {
        Some(if r.id == "r1" {
            known.clone()
        } else {
            r2.clone()
        })
    };
    let facts = on_timeline(&p, &take, S);
    assert!(facts.activity_known && facts.sound_known);
    assert_eq!(facts.idle, expected.idle);
    assert_eq!(on_timeline(&p, &|_| None, S), TimelineFacts::default());
}

/// A track moving along x (or y) at `per_s` of the frame a second for 120 s, a sample every 100 ms.
fn moving(per_s: f32, along_y: bool) -> Arc<CursorTrack> {
    let mut lines = vec![common::head()];
    for t in (0..=120 * S).step_by(100_000) {
        let v = per_s * t as f32 / S as f32;
        let (x, y) = if along_y { (0.5, v) } else { (v, 0.5) };
        lines.push(Line::Move { t, x, y });
    }
    Arc::new(common::read(&common::file(&lines)))
}

#[test]
fn the_raw_cursor_follows_the_clip_under_the_time() {
    let p = project();
    let starts = clip_starts(&p.timeline.main);
    let (r1, r2) = (moving(0.005, false), moving(0.01, true));
    let track = |r: &Recording| Some(if r.id == "r1" { r1.clone() } else { r2.clone() });
    let at = |t: i64| cursor_at(&p, &starts, &track, t);
    let near = |a: Option<(f32, f32)>, b: (f32, f32)| {
        a.is_some_and(|a| (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4)
    };
    // 10 s plays 20 s of r1, whose cursor is 100 ms earlier (19.9 s); 40 s plays 80 s of it at 2×.
    assert!(near(at(10 * S), (0.0995, 0.5)), "{:?}", at(10 * S));
    assert!(near(at(40 * S), (0.3995, 0.5)), "{:?}", at(40 * S));
    assert_eq!(at(52 * S), None, "b-roll has no cursor");
    assert!(near(at(60 * S), (0.5, 0.05)), "{:?}", at(60 * S));
    assert_eq!(at(90 * S), None, "past the end");
}

#[test]
fn each_kind_stops_at_the_cap() {
    let p = project();
    let many = 120_000;
    let r2 = Arc::new(TakeFacts {
        presses: (0..many).map(|i| press(i * 200)).collect(),
        dwells: (0..many).map(|i| dwell(i * 200, i * 200 + 100)).collect(),
        ..TakeFacts::default()
    });
    let take = |r: &Recording| (r.id == "r2").then(|| r2.clone());
    let facts = on_timeline(&p, &take, S);
    assert_eq!(
        (facts.clicks.len(), facts.dwells.len()),
        (MAX_FACTS, MAX_FACTS)
    );
    let last = (MAX_FACTS as i64 - 1) * 200;
    assert_eq!(
        facts.clicks.last().map(|c| c.timeline_us),
        Some(55 * S + last)
    );
}

#[test]
fn a_pause_and_an_idle_stretch_across_a_split_stay_one() {
    // r2 split at 10 s into two clips that play on, the second at 2×: timeline 0–10 s and 10–20 s.
    let mut p = Project::new("Split");
    p.recordings = vec![screen_take("r2", 0)];
    p.timeline.main = vec![clip("r2", 0, 10, 1.0), clip("r2", 10, 30, 2.0)];
    let r2 = Arc::new(TakeFacts {
        dwells: vec![dwell(8 * S, 12 * S)],
        still: vec![(6 * S, 14 * S)],
        typing: Some(Vec::new()),
        media: Some(Vec::new()),
        quiet_screen: Some(vec![(0, 30 * S)]),
        quiet_sound: Some(vec![(0, 30 * S)]),
        ..TakeFacts::default()
    });
    let take = |_: &Recording| Some(r2.clone());
    // Still 6–10 s, then 10–12 s on the timeline: 4 s and 2 s, so only the joined 6 s lasts the 5 s asked for.
    let facts = on_timeline(&p, &take, 5 * S);
    let joined = SpanAt {
        from_us: 8 * S,
        to_us: 11 * S,
        x: 0.25,
        y: 0.75,
    };
    assert_eq!(facts.dwells, vec![joined]);
    assert_eq!(facts.idle, vec![(6 * S, 12 * S)]);
}

#[test]
fn typing_is_never_an_idle_stretch() {
    // r2 plays 0–30 s; its cursor rests and its screen and sound are quiet throughout, but someone types over 10–20 s.
    let mut p = Project::new("Typing");
    p.recordings = vec![screen_take("r2", 0)];
    p.timeline.main = vec![clip("r2", 0, 30, 1.0)];
    let typed = RegionSpan {
        from: 10 * S,
        to: 20 * S,
        rect: TYPED,
    };
    let r2 = Arc::new(TakeFacts {
        still: vec![(0, i64::MAX)],
        typing: Some(vec![typed]),
        media: Some(Vec::new()),
        quiet_screen: Some(vec![(0, 30 * S)]),
        quiet_sound: Some(vec![(0, 30 * S)]),
        ..TakeFacts::default()
    });
    let take = |_: &Recording| Some(r2.clone());
    let facts = on_timeline(&p, &take, S);
    assert_eq!(facts.idle, vec![(0, 10 * S), (20 * S, 30 * S)]);
}
