//! What a cut does to the layers over the main track, free and attached alike: a layer inside a removed range goes with
//! it, one after it moves back with the footage, and one across a cut keeps what stays. Music stays free and is fitted
//! to the new end (`media::music`).
mod common;
use common::*;
use klipolo_model::media::music::{add_music_ops, is_music, with_music_refit, MusicOpts};
use klipolo_model::ops::Edge;
use klipolo_model::query::{anchor_at, main_track_duration, video_duration};
use klipolo_model::timeline::{Item, ItemContent, Track, TrackKind};
use klipolo_model::{apply_ops, Op, Project, RangeUs};

/// A track: its kind, its id and its layers, those marked `true` attached to the clip under their start.
type TrackOf<'a> = (TrackKind, &'a str, Vec<(Item, bool)>);

/// The fixture (clip a 0–20 s, clip b 20–30 s) with a 60 s whoosh file, and `tracks`.
fn project(tracks: Vec<TrackOf<'_>>) -> Project {
    let mut p = fixture();
    let mut whoosh = p.asset("music").unwrap().clone();
    whoosh.id = "whoosh".into();
    whoosh.path = "/media/whoosh".into();
    whoosh.duration_us = secs(60.0);
    p.assets.push(whoosh);
    for (kind, id, layers) in tracks {
        let mut track = Track::new(kind, id);
        track.id = id.into();
        for (mut item, attached) in layers {
            if attached {
                item.attach = anchor_at(&p.timeline.main, item.start_us);
                assert!(item.attach.is_some(), "{} is over a clip", item.id);
            }
            track.items.push(item);
        }
        p.timeline.tracks.push(track);
    }
    p
}

/// A sound effect of the whoosh file (no ducking), `start` to `start + len` seconds, fading in and out over `fade` s.
fn sound(id: &str, start: f64, len: f64, fade: f64) -> Item {
    Item {
        content: ItemContent::Audio {
            asset_id: "whoosh".into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: secs(fade),
            fade_out_us: secs(fade),
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        ..still_item(id, start, len)
    }
}

fn cut(from: f64, to: f64) -> Op {
    Op::DeleteRanges {
        ranges: vec![RangeUs {
            start: secs(from),
            end: secs(to),
        }],
    }
}

fn after(p: &Project, ops: &[Op]) -> Project {
    apply_ops(p, ops).unwrap_or_else(|e| panic!("refused: {e}"))
}

fn has(p: &Project, id: &str) -> bool {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .any(|i| i.id == id)
}

/// Start and end in seconds.
fn span(item: &Item) -> (f64, f64) {
    (to_secs(item.start_us), to_secs(item.end_us()))
}

fn to_secs(us: i64) -> f64 {
    us as f64 / 1e6
}

/// The layers of track `id`, in order, as spans.
fn spans_on(p: &Project, id: &str) -> Vec<(f64, f64)> {
    let track = p.timeline.tracks.iter().find(|t| t.id == id).unwrap();
    track.items.iter().map(span).collect()
}

/// A sound layer's place in its file and its fades, in seconds.
fn sound_of(item: &Item) -> (f64, f64, f64) {
    match &item.content {
        ItemContent::Audio {
            source_in_us,
            fade_in_us,
            fade_out_us,
            ..
        } => (
            to_secs(*source_in_us),
            to_secs(*fade_in_us),
            to_secs(*fade_out_us),
        ),
        other => panic!("not a sound layer: {other:?}"),
    }
}

/// Nothing runs past the video's end, and the video ends with the main track.
fn assert_nothing_past_the_end(p: &Project) {
    let end = main_track_duration(&p.timeline.main);
    assert_eq!(video_duration(&p.timeline), end);
    for item in p.timeline.tracks.iter().flat_map(|t| &t.items) {
        assert!(
            item.end_us() <= end,
            "{} ends past the video: {:?}",
            item.id,
            span(item)
        );
    }
}

#[test]
fn a_cut_takes_free_sounds_inside_it_moves_the_ones_after_and_trims_the_ones_across() {
    let p = project(vec![
        (
            TrackKind::Audio,
            "s1",
            vec![
                (sound("across", 8.0, 4.0, 0.5), false),
                (sound("after", 20.0, 2.0, 0.0), false),
            ],
        ),
        (
            TrackKind::Audio,
            "s2",
            vec![(sound("inside", 11.0, 3.0, 0.0), false)],
        ),
        (
            TrackKind::Audio,
            "s3",
            vec![(sound("around", 9.0, 8.0, 0.3), false)],
        ),
    ]);
    let p = after(&p, &[cut(10.0, 15.0)]);

    assert!(!has(&p, "inside"), "a sound inside the cut goes with it");
    let moved = item_by_id(&p, "after");
    assert_eq!(
        span(moved),
        (15.0, 17.0),
        "a sound after the cut moves back by its 5 s"
    );
    assert_eq!(moved.attach, None, "and stays free");
    // Across the cut's start: the part before it stays, and its own end went, so no fade-out.
    let across = item_by_id(&p, "across");
    assert_eq!(span(across), (8.0, 10.0));
    assert_eq!(sound_of(across), (0.0, 0.5, 0.0));
    // Around the cut: two pieces, the second playing on from where the cut ends in the file.
    assert_eq!(spans_on(&p, "s3"), [(9.0, 10.0), (10.0, 12.0)]);
    let pieces = &p
        .timeline
        .tracks
        .iter()
        .find(|t| t.id == "s3")
        .unwrap()
        .items;
    assert_eq!(pieces[0].id, "around");
    assert_eq!(sound_of(&pieces[0]), (0.0, 0.3, 0.0));
    assert_eq!(sound_of(&pieces[1]), (6.0, 0.0, 0.3));
    assert!(pieces.iter().all(|i| i.attach.is_none()));
}

#[test]
fn a_cut_ripples_free_overlays_as_it_does_attached_ones() {
    let p = project(vec![
        (
            TrackKind::Overlay,
            "free",
            vec![
                (still_item("f-across", 8.0, 4.0), false),
                (still_item("f-inside", 12.0, 1.0), false),
                (still_item("f-after", 20.0, 1.0), false),
                // A card after the main track's end (only layers play there): it stays right after the video.
                (still_item("f-end", 30.0, 3.0), false),
            ],
        ),
        (
            TrackKind::Overlay,
            "kept",
            vec![
                (still_item("a-across", 8.0, 4.0), true),
                (still_item("a-inside", 12.0, 1.0), true),
                (still_item("a-after", 20.0, 1.0), true),
            ],
        ),
    ]);
    let p = after(&p, &[cut(10.0, 15.0)]);

    for kind in ["f", "a"] {
        assert!(!has(&p, &format!("{kind}-inside")), "{kind}: inside goes");
        assert_eq!(
            span(item_by_id(&p, &format!("{kind}-across"))),
            (8.0, 10.0),
            "{kind}"
        );
        assert_eq!(
            span(item_by_id(&p, &format!("{kind}-after"))),
            (15.0, 16.0),
            "{kind}"
        );
    }
    assert_eq!(span(item_by_id(&p, "f-end")), (25.0, 28.0));
    assert_eq!(video_duration(&p.timeline), secs(28.0));
    for id in ["f-across", "f-after", "f-end"] {
        assert_eq!(item_by_id(&p, id).attach, None, "{id} stays free");
    }
}

#[test]
fn music_stays_free_and_is_fitted_to_the_new_end() {
    let p = project(vec![]);
    let song = p.asset("music").unwrap().clone();
    let p = after(&p, &add_music_ops(&p, &song, MusicOpts::default()).unwrap());
    let bed = |p: &Project| -> Vec<(f64, f64)> {
        let mut m: Vec<&Item> = p
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.items)
            .filter(|i| is_music(i))
            .collect();
        m.sort_by_key(|i| i.start_us);
        assert!(m.iter().all(|i| i.attach.is_none()), "music stays free");
        m.into_iter().map(span).collect()
    };
    assert_eq!(bed(&p), [(0.0, 30.0)]);

    // A cut in the middle doesn't chop it: it plays on, fitted to the new end.
    let middle = after(&p, &with_music_refit(&p, vec![cut(10.0, 12.0)]).unwrap());
    assert_eq!(bed(&middle), [(0.0, 28.0)]);
    // Keeping only the start ends it with the video.
    let start = after(&p, &with_music_refit(&p, vec![cut(5.0, 30.0)]).unwrap());
    assert_eq!(bed(&start), [(0.0, 5.0)]);
    assert_nothing_past_the_end(&start);
}

#[test]
fn removing_or_trimming_a_clip_ripples_free_layers_too() {
    let p = project(vec![
        (
            TrackKind::Audio,
            "s1",
            vec![(sound("s", 22.0, 2.0, 0.0), false)],
        ),
        (
            TrackKind::Overlay,
            "ov",
            vec![(still_item("t", 18.5, 1.0), false)],
        ),
    ]);

    // Clip a (0–20 s) goes: the title over it goes too, and the sound after it moves back 20 s.
    let removed = after(&p, &[Op::RemoveClip { id: "a".into() }]);
    assert!(!has(&removed, "t"));
    assert_eq!(span(item_by_id(&removed, "s")), (2.0, 4.0));

    // Clip a's end trimmed by 2 s (18–20 s goes): the same.
    let trimmed = after(
        &p,
        &[Op::TrimClip {
            id: "a".into(),
            edge: Edge::End,
            source_us: secs(18.0),
        }],
    );
    assert!(!has(&trimmed, "t"));
    assert_eq!(span(item_by_id(&trimmed, "s")), (20.0, 22.0));
}

#[test]
fn free_layers_on_a_locked_track_ripple_too() {
    // A lock blocks direct edits; the ripple still applies, to keep sync.
    let mut p = project(vec![(
        TrackKind::Audio,
        "s1",
        vec![(sound("s", 20.0, 2.0, 0.0), false)],
    )]);
    p.timeline.tracks[0].locked = true;
    let p = after(&p, &[cut(10.0, 15.0)]);
    assert_eq!(span(item_by_id(&p, "s")), (15.0, 17.0));
}
