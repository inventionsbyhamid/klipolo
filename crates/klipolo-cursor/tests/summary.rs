//! The planner's cursor line and `describeCursor`'s answer: plain words, region words, no coordinates.

use klipolo_cursor::Rect01;
use klipolo_cursor::summary::{describe, region_word, summary_line};
use klipolo_cursor::timeline::{ClickAt, RegionAt, SpanAt, TimelineFacts};

const S: i64 = 1_000_000;

fn click(s: i64, x: f32, y: f32) -> ClickAt {
    ClickAt {
        timeline_us: s * S,
        x,
        y,
    }
}

fn facts() -> TimelineFacts {
    TimelineFacts {
        clicks: vec![
            click(20, 0.5, 0.5),
            click(64, 0.8, 0.2),
            click(65, 0.75, 0.1),
        ],
        dwells: vec![SpanAt {
            from_us: 72 * S,
            to_us: 75 * S,
            x: 0.1,
            y: 0.5,
        }],
        idle: vec![(113 * S, 140 * S)],
        activity_known: true,
        sound_known: true,
        ..TimelineFacts::default()
    }
}

#[test]
fn the_summary_reads_plainly() {
    let golden = "Cursor: 3 clicks, 1 pause, 1 idle stretch. Near 1:00–3:00: clicks 1:04 and 1:05 (top right); \
                  paused 1:12–1:15 (middle left); idle 1:53–2:20.";
    // The playhead at 1:30. The click at 0:20 is the farthest from it, and doesn't fit.
    let f = facts();
    assert_eq!(
        summary_line(&f, 90 * S, golden.len() + 10).as_deref(),
        Some(golden)
    );
    let all = summary_line(&f, 90 * S, 400).unwrap();
    assert!(
        all.contains("Near 0:00–3:00: clicks 0:20 (centre), 1:04 and 1:05 (top right);"),
        "{all}"
    );
    assert_eq!(summary_line(&TimelineFacts::default(), 0, 400), None);
    assert_eq!(region_word(0.5, 0.5), "centre");
    assert_eq!(region_word(0.1, 0.9), "bottom left");
    assert_eq!(region_word(0.5, 0.1), "top centre");
}

/// Every m:ss or h:mm:ss in `s`, in µs.
fn clock_times(s: &str) -> Vec<i64> {
    s.split(|c: char| !(c.is_ascii_digit() || c == ':'))
        .map(|w| w.trim_matches(':'))
        .filter(|w| w.contains(':'))
        .map(|w| {
            w.split(':')
                .fold(0, |acc, p| acc * 60 + p.parse::<i64>().unwrap())
                * S
        })
        .collect()
}

#[test]
fn the_summary_stays_under_400_bytes_and_keeps_what_is_near_the_playhead() {
    // 10,000 clicks over two hours, scattered over the frame; a pause far away; the playhead at 1:30:00.
    let mut seed = 0x5EED_u64;
    let mut unit = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    let f = TimelineFacts {
        clicks: (0..10_000)
            .map(|i| ClickAt {
                timeline_us: i * 720_000,
                x: unit(),
                y: unit(),
            })
            .collect(),
        dwells: vec![SpanAt {
            from_us: 600 * S,
            to_us: 605 * S,
            x: 0.5,
            y: 0.5,
        }],
        activity_known: true,
        sound_known: true,
        ..TimelineFacts::default()
    };
    let playhead = 5_400 * S;
    let line = summary_line(&f, playhead, 400).unwrap();
    assert!(line.len() <= 400, "{} bytes: {line}", line.len());
    assert!(
        line.starts_with("Cursor: 10000 clicks, 1 pause, 0 idle stretches. Near "),
        "{line}"
    );
    let times = clock_times(line.split_once("Near ").unwrap().1);
    assert!(times.len() >= 10, "{line}");
    for t in times {
        assert!((t - playhead).abs() <= 180 * S, "{line}");
    }
}

#[test]
fn describe_answers_for_a_range() {
    let mut f = facts();
    f.typing = vec![RegionAt {
        from_us: 100 * S,
        to_us: 112 * S,
        rect: Rect01 {
            x: 0.0,
            y: 0.7,
            w: 0.3,
            h: 0.2,
        },
    }];
    assert_eq!(
        describe(&f, 60 * S, 120 * S),
        "Between 1:00 and 2:00: 2 clicks: 1:04 and 1:05 (top right); 1 pause: 1:12–1:15 (middle left); \
         1 typing stretch: 1:40–1:52 (bottom left); 1 idle stretch: 1:53–2:20."
    );
    assert_eq!(
        describe(&f, 150 * S, 170 * S),
        "Between 2:30 and 2:50: no clicks, pauses, typing or idle stretches."
    );
    f.activity_known = false;
    assert_eq!(
        describe(&f, 0, 30 * S),
        "Between 0:00 and 0:30: 1 click: 0:20 (centre). Typing and idle stretches aren't counted yet: the screen is \
         still being analysed."
    );
}

#[test]
fn the_summary_says_idle_isnt_measured_until_the_screen_and_the_sound_are() {
    let mut f = facts();
    f.idle.clear();
    let counts = |f: &TimelineFacts| {
        let line = summary_line(f, 90 * S, 400).unwrap();
        line.split_once(" Near ").unwrap().0.to_owned()
    };
    assert_eq!(counts(&f), "Cursor: 3 clicks, 1 pause, 0 idle stretches.");
    // Before the screen analysis, or the sound measurement, has run, no idle stretch is known yet: never "0".
    f.sound_known = false;
    assert_eq!(
        counts(&f),
        "Cursor: 3 clicks, 1 pause, idle not measured yet."
    );
    // Before the screen analysis, typing isn't counted either, and the line says so.
    f.sound_known = true;
    f.activity_known = false;
    assert_eq!(
        counts(&f),
        "Cursor: 3 clicks, 1 pause, typing and idle not measured yet."
    );
}

#[test]
fn describe_lists_twelve_of_a_kind_then_says_how_many_more() {
    let f = TimelineFacts {
        clicks: (0..15).map(|k| click(60 + 2 * k, 0.5, 0.5)).collect(),
        activity_known: true,
        sound_known: true,
        ..TimelineFacts::default()
    };
    assert_eq!(
        describe(&f, 60 * S, 120 * S),
        "Between 1:00 and 2:00: 15 clicks: 1:00, 1:02, 1:04, 1:06, 1:08, 1:10, 1:12, 1:14, 1:16, 1:18, 1:20 and 1:22 \
         (centre), and 3 more."
    );
}
