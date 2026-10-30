mod common;
use common::*;
use klipolo_model::project::*;
use klipolo_model::time::{Us, MAX_TIME_US};
use klipolo_model::timeline::*;
use klipolo_model::validate::validate;
use klipolo_model::ModelError;

fn with_track(items: Vec<Item>) -> Project {
    let mut p = fixture();
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.items = items;
    p.timeline.tracks.push(track);
    p
}

fn assert_invalid(p: &Project, needle: &str) {
    match validate(p) {
        Err(e @ ModelError::Invalid(_)) => {
            let msg = e.to_string();
            assert!(msg.contains(needle), "expected {needle:?} in {msg:?}")
        }
        other => panic!("expected Invalid containing {needle:?}, got {other:?}"),
    }
}

#[test]
fn fixture_is_valid() {
    validate(&fixture()).unwrap();
    validate(&with_track(vec![
        text_item("t", 1.0, 2.0),
        video_item("v", 4.0, 2.0, 1.0),
    ]))
    .unwrap();
}

type Mutation = Box<dyn Fn(&mut Project)>;

#[test]
fn rejects_bad_projects() {
    let cases: Vec<(Mutation, &str)> = vec![
        (Box::new(|p| p.name = "  ".into()), "name"),
        (Box::new(|p| p.settings.fps = 29), "fps"),
        (
            Box::new(|p| p.settings.primary_format = FormatKind::Square),
            "primary format",
        ),
        (
            Box::new(|p| p.settings.formats.clear()),
            "between one and three",
        ),
        (
            Box::new(|p| p.settings.formats = vec![FormatKind::Landscape, FormatKind::Landscape]),
            "only be added once",
        ),
        (
            Box::new(|p| p.assets[0].id = "camera".into()),
            "duplicate id (camera)",
        ),
        (
            Box::new(|p| p.timeline.main[0].source_out_us = secs(61.0)),
            "outside its source",
        ),
        (
            Box::new(|p| p.timeline.main[0].source_in_us = secs(20.0)),
            "outside its source",
        ),
        (Box::new(|p| p.timeline.main[1].speed = 8.0), "speed"),
        (Box::new(|p| p.timeline.main[1].volume = 3.0), "volume"),
        (Box::new(|p| p.timeline.main[1].speed = f64::NAN), "speed"),
        (
            Box::new(|p| p.timeline.main[1].speed = f64::INFINITY),
            "speed",
        ),
        (Box::new(|p| p.timeline.main[1].volume = f64::NAN), "volume"),
        (
            Box::new(|p| p.timeline.main[1].volume = f64::INFINITY),
            "volume",
        ),
        (
            Box::new(|p| p.timeline.main[1].fade_in_us = secs(11.0)),
            "fade",
        ),
        (
            Box::new(|p| {
                p.timeline.main[1].fade_in_us = secs(6.0);
                p.timeline.main[1].fade_out_us = secs(6.0)
            }),
            "fade",
        ),
        (
            Box::new(|p| p.timeline.main[1].camera = Some(CameraLayout::default())),
            "camera",
        ),
        (
            Box::new(|p| p.timeline.main[0].look.lut_id = Some("missing".into())),
            "LUT",
        ),
        (
            Box::new(|p| {
                p.luts.push(ColorLut {
                    id: "lut".into(),
                    name: "Teal".into(),
                    path: " ".into(),
                })
            }),
            "LUT Teal has no file",
        ),
        (Box::new(|p| p.background.color = "red".into()), "colour"),
        (
            Box::new(|p| p.recordings[0].sources[1].runs[0].asset_id = "mic".into()),
            "recording",
        ),
    ];
    for (mutate, needle) in cases {
        let mut p = fixture();
        mutate(&mut p);
        assert_invalid(&p, needle);
    }
}

#[test]
fn rejects_bad_items() {
    assert_invalid(
        &with_track(vec![text_item("t", 1.0, 2.0), text_item("u", 2.0, 2.0)]),
        "overlap",
    );
    assert_invalid(
        &with_track(vec![text_item("t", 3.0, 2.0), text_item("u", 0.0, 1.0)]),
        "overlap",
    );
    let mut empty = text_item("t", 1.0, 2.0);
    if let ItemContent::Text { text, .. } = &mut empty.content {
        *text = " ".into();
    }
    assert_invalid(&with_track(vec![empty]), "text");
    let mut too_short = text_item("t", 1.0, 0.0);
    too_short.duration_us = 5;
    assert_invalid(&with_track(vec![too_short]), "too short");
    assert_invalid(
        &with_track(vec![video_item("v", 0.0, 5.0, 8.0)]),
        "outside its source",
    );
    // Not "a": that id belongs to the fixture's first clip.
    let mut audio_on_overlay = still_item("aud", 0.0, 1.0);
    audio_on_overlay.content = ItemContent::Audio {
        asset_id: "music".into(),
        source_in_us: 0,
        volume: 1.0,
        muted: false,
        fade_in_us: 0,
        fade_out_us: 0,
        effect: None,
        drift_ppm: 0.0,
        centre_from: None,
        tie: None,
        duck: None,
    };
    assert_invalid(&with_track(vec![audio_on_overlay]), "belong");
    let mut faded = text_item("t", 1.0, 2.0);
    faded.layout.opacity = 1.5;
    assert_invalid(&with_track(vec![faded]), "opacity");
}

/// Fixture plus an overlay track (an attached video layer, then a text layer) and an audio track.
/// Every stored time is in use, so each one can be pushed out of range.
fn busy() -> Project {
    let mut video = video_item("v", 1.0, 2.0, 1.0);
    video.attach = Some(Attach {
        clip_id: "a".into(),
        source: ClipSource::Recording { id: "rec".into() },
        source_us: secs(1.0),
    });
    let mut p = with_track(vec![video, text_item("t", 4.0, 2.0)]);
    let mut music = still_item("m", 0.0, 30.0);
    music.content = ItemContent::Audio {
        asset_id: "music".into(),
        source_in_us: 0,
        volume: 1.0,
        muted: false,
        fade_in_us: secs(1.0),
        fade_out_us: secs(1.0),
        effect: None,
        drift_ppm: 0.0,
        centre_from: None,
        tie: None,
        duck: None,
    };
    let mut audio = Track::new(TrackKind::Audio, "Audio 1");
    audio.items.push(music);
    p.timeline.tracks.push(audio);
    p
}

type TimeField = fn(&mut Project) -> &mut Us;

/// Every stored time in `busy()`. The text layer's start and duration are checked on the last
/// layer of its track, so nothing but the time bound can reject them.
const TIME_FIELDS: &[(&str, TimeField)] = &[
    ("asset duration", |p| &mut p.assets[0].duration_us),
    ("camera offset", |p| {
        &mut p.recordings[0].sources[1].runs[0].offset_us
    }),
    ("mic offset", |p| {
        &mut p.recordings[0].sources[2].runs[0].offset_us
    }),
    ("mic length", |p| {
        &mut p.recordings[0].sources[2].runs[0].duration_us
    }),
    ("mic nudge", |p| &mut p.recordings[0].sources[2].nudge_us),
    ("clip source in", |p| &mut p.timeline.main[0].source_in_us),
    ("clip source out", |p| &mut p.timeline.main[0].source_out_us),
    ("clip fade in", |p| &mut p.timeline.main[0].fade_in_us),
    ("clip fade out", |p| &mut p.timeline.main[0].fade_out_us),
    ("item start", |p| {
        &mut p.timeline.tracks[0].items[1].start_us
    }),
    ("item duration", |p| {
        &mut p.timeline.tracks[0].items[1].duration_us
    }),
    ("attach source", |p| {
        &mut p.timeline.tracks[0].items[0]
            .attach
            .as_mut()
            .unwrap()
            .source_us
    }),
    ("video source in", |p| {
        match &mut p.timeline.tracks[0].items[0].content {
            ItemContent::Video { source_in_us, .. } => source_in_us,
            _ => unreachable!(),
        }
    }),
    ("audio source in", |p| {
        match &mut p.timeline.tracks[1].items[0].content {
            ItemContent::Audio { source_in_us, .. } => source_in_us,
            _ => unreachable!(),
        }
    }),
    ("audio fade in", |p| {
        match &mut p.timeline.tracks[1].items[0].content {
            ItemContent::Audio { fade_in_us, .. } => fade_in_us,
            _ => unreachable!(),
        }
    }),
    ("audio fade out", |p| {
        match &mut p.timeline.tracks[1].items[0].content {
            ItemContent::Audio { fade_out_us, .. } => fade_out_us,
            _ => unreachable!(),
        }
    }),
];

#[test]
fn rejects_out_of_range_times_without_panicking() {
    validate(&busy()).unwrap();
    for (what, field) in TIME_FIELDS {
        for v in [i64::MAX, i64::MIN, MAX_TIME_US + 1, -MAX_TIME_US - 1] {
            let mut p = busy();
            *field(&mut p) = v;
            assert!(
                matches!(validate(&p), Err(ModelError::Invalid(_))),
                "{what} = {v} was accepted"
            );
        }
    }
}

#[test]
fn accepts_times_at_the_bound() {
    let mut p = busy();
    p.assets[0].duration_us = MAX_TIME_US;
    p.recordings[0].sources[2].runs[0].offset_us = MAX_TIME_US;
    p.recordings[0].sources[1].nudge_us = -5_000_000;
    p.timeline.tracks[0].items[1].start_us = MAX_TIME_US;
    p.timeline.tracks[0].items[1].duration_us = MAX_TIME_US;
    validate(&p).unwrap();
}

#[test]
fn clip_and_item_helpers_saturate() {
    let mut clip = Clip::new(ClipSource::Asset { id: "broll".into() }, i64::MIN, i64::MAX);
    assert_eq!(clip.duration_us(), i64::MAX);
    clip.source_in_us = i64::MAX;
    clip.source_out_us = i64::MIN;
    assert_eq!(clip.duration_us(), i64::MIN);
    let mut item = text_item("t", 0.0, 1.0);
    item.start_us = i64::MAX;
    assert_eq!(item.end_us(), i64::MAX);
    item.start_us = i64::MIN;
    item.duration_us = i64::MIN;
    assert_eq!(item.end_us(), i64::MIN);
}

#[test]
fn tolerates_sub_frame_rounding_overlaps() {
    let mut touching = text_item("u", 3.0, 1.0);
    touching.start_us = secs(3.0) - 4;
    validate(&with_track(vec![
        text_item("t", 1.0, 2.0),
        touching.clone(),
    ]))
    .unwrap();
    touching.start_us = secs(3.0) - 5;
    assert_invalid(
        &with_track(vec![text_item("t", 1.0, 2.0), touching]),
        "overlap",
    );
}
