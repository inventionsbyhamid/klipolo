//! What a cut does to the layers over the main track. Attached layers follow their footage: one inside a removed range
//! goes with it, one after it moves back, one across a cut keeps what stays. Free layers keep their time, and those the
//! cut leaves past the video's new end are trimmed to it or go. A sound effect keeps nothing past the video's end, where
//! it was never heard. Music stays free and is fitted to the new end (`media::music`).
mod common;
use common::*;
use klipolo_model::history::Document;
use klipolo_model::media::music::{add_music_ops, is_music, with_music_refit, MusicOpts};
use klipolo_model::ops::Edge;
use klipolo_model::query::{anchor_at, main_track_duration, video_duration};
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, Track, TrackKind};
use klipolo_model::{apply_ops, Op, Project, RangeUs, Transaction};

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
            volume_keys: Vec::new(),
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

fn tx(ops: Vec<Op>) -> Transaction {
    Transaction {
        label: "edit".into(),
        ops,
        origin: None,
        ..Default::default()
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
fn the_demo_cut_leaves_no_free_sound_past_the_new_end() {
    // A 3:37.5 video, a title over its start, and a 155 s sound dropped free at 2:02.97, its last minute past the
    // end. Keeping only 0–25.5 s cuts the rest.
    let mut p = project(vec![
        (
            TrackKind::Audio,
            "Audio 1",
            vec![(sound("whoosh-layer", 0.0, 1.0, 0.0), false)],
        ),
        (
            TrackKind::Overlay,
            "Overlay 1",
            vec![(still_item("title", 1.0, 3.0), true)],
        ),
    ]);
    let mut take = p.asset("screen").unwrap().clone();
    take.id = "take".into();
    take.duration_us = secs(600.0);
    p.assets.push(take);
    let mut clip = Clip::new(ClipSource::Asset { id: "take".into() }, 0, secs(217.5));
    clip.id = "clipA".into();
    p.timeline.main = vec![clip];
    p.timeline.tracks[1].items[0].attach = anchor_at(&p.timeline.main, secs(1.0));
    p.assets
        .iter_mut()
        .find(|a| a.id == "whoosh")
        .unwrap()
        .duration_us = 155_016_438;
    let layer = &mut p.timeline.tracks[0].items[0];
    (layer.start_us, layer.duration_us) = (122_966_667, 155_016_438);
    let ops = vec![cut(25.5, 217.5)];
    let next = after(&p, &ops);

    assert!(
        !has(&next, "whoosh-layer"),
        "the sound left past the end goes"
    );
    assert_eq!(span(item_by_id(&next, "title")), (1.0, 4.0));
    assert_eq!(main_track_duration(&next.timeline.main), secs(25.5));
    assert_nothing_past_the_end(&next);
    let review = tx(ops).review(&p).unwrap();
    assert_eq!(review.removals.items, ["whoosh-layer"]);
}

#[test]
fn free_layers_keep_their_time_through_a_cut() {
    let p = project(vec![
        (
            TrackKind::Overlay,
            "free",
            vec![(text_item("intro", 0.0, 3.0), false)],
        ),
        (
            TrackKind::Overlay,
            "kept",
            vec![(still_item("caption", 20.0, 1.0), true)],
        ),
        (
            TrackKind::Audio,
            "s1",
            vec![(sound("ding", 20.0, 2.0, 0.0), false)],
        ),
    ]);
    let next = after(&p, &[cut(5.0, 10.0)]);
    assert_eq!(item_by_id(&next, "intro"), item_by_id(&p, "intro"));
    assert_eq!(item_by_id(&next, "ding"), item_by_id(&p, "ding"));
    // The attached caption follows its footage back by the cut's 5 s.
    assert_eq!(span(item_by_id(&next, "caption")), (15.0, 16.0));
}

#[test]
fn a_free_title_at_the_start_survives_an_insert_then_a_delete() {
    let p = project(vec![(
        TrackKind::Overlay,
        "free",
        vec![(text_item("intro", 0.0, 3.0), false)],
    )]);
    let mut clip = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(10.0));
    clip.id = "x".into();
    let inserted = after(
        &p,
        &[Op::InsertClip {
            clip,
            index: Some(0),
        }],
    );
    assert_eq!(item_by_id(&inserted, "intro"), item_by_id(&p, "intro"));
    let back = after(&inserted, &[Op::RemoveClip { id: "x".into() }]);
    assert_eq!(item_by_id(&back, "intro"), item_by_id(&p, "intro"));
    let clips: Vec<&str> = back.timeline.main.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(clips, ["a", "b"]);
}

#[test]
fn a_free_overlay_across_the_new_end_is_trimmed_to_it() {
    // Cutting 10–20 s ends the video at 20 s: a free title from 15 s is trimmed there, one from 22 s goes, and a card
    // placed after the end (from 30 s) moves with it, to 20 s.
    let p = project(vec![
        (
            TrackKind::Overlay,
            "free",
            vec![
                (text_item("across", 15.0, 10.0), false),
                (still_item("card", 30.0, 3.0), false),
            ],
        ),
        (
            TrackKind::Overlay,
            "more",
            vec![(still_item("late", 22.0, 2.0), false)],
        ),
    ]);
    let ops = vec![cut(10.0, 20.0)];
    let next = after(&p, &ops);
    let across = item_by_id(&next, "across");
    assert_eq!(span(across), (15.0, 20.0));
    assert!(
        across
            .layout
            .motion
            .as_ref()
            .is_some_and(|m| m.exit.is_some()),
        "it keeps its fade-out, at its new end"
    );
    assert!(!has(&next, "late"));
    assert_eq!(span(item_by_id(&next, "card")), (20.0, 23.0));
    assert_eq!(video_duration(&next.timeline), secs(23.0));

    // The proposal names what goes, what is shortened and what moves.
    let review = tx(ops).review(&p).unwrap();
    assert_eq!(review.removals.items, ["late"]);
    assert_eq!(review.removals.shortened, ["across"]);
    assert_eq!(review.removals.moved, ["card"]);
}

#[test]
fn keeping_only_the_start_leaves_no_sound_past_the_new_end() {
    // A whoosh from 20 s for 20 s (its last 10 s past the 30 s video, never heard), one attached at 25 s that runs on
    // 5 s past the end, and a title in the part kept. Keeping only 0–5 s cuts 5–30 s.
    let p = project(vec![
        (
            TrackKind::Audio,
            "s1",
            vec![(sound("long", 20.0, 20.0, 0.0), false)],
        ),
        (
            TrackKind::Audio,
            "s2",
            vec![(sound("late", 25.0, 10.0, 0.0), true)],
        ),
        (
            TrackKind::Overlay,
            "ov",
            vec![(still_item("title", 1.0, 3.0), true)],
        ),
    ]);
    let ops = vec![cut(5.0, 30.0)];
    let next = after(&p, &ops);

    assert!(!has(&next, "long"), "the free whoosh is left past the end");
    assert!(
        !has(&next, "late"),
        "the attached one went with its footage"
    );
    assert_eq!(span(item_by_id(&next, "title")), (1.0, 4.0));
    assert_eq!(main_track_duration(&next.timeline.main), secs(5.0));
    assert_nothing_past_the_end(&next);

    // The proposal names both as going.
    let review = tx(ops).review(&p).unwrap();
    assert_eq!(review.removals.items, ["long", "late"]);
}

#[test]
fn a_sound_running_past_the_end_ends_with_the_video_after_a_cut() {
    // Two sounds run 5 s and 4 s past the 30 s end, never heard there; one is free, one attached. A sound placed after
    // the end on purpose keeps its distance from the end.
    let p = project(vec![
        (
            TrackKind::Audio,
            "s1",
            vec![
                (sound("tail", 25.0, 10.0, 0.5), false),
                (sound("parked", 36.0, 2.0, 0.0), false),
            ],
        ),
        (
            TrackKind::Audio,
            "s2",
            vec![(sound("tied", 24.0, 10.0, 0.0), true)],
        ),
    ]);
    let p = after(&p, &[cut(2.0, 6.0)]);

    // Free: it keeps its time and is trimmed where the video now ends.
    let tail = item_by_id(&p, "tail");
    assert_eq!(span(tail), (25.0, 26.0));
    assert_eq!(tail.attach, None);
    // Attached: it moves back 4 s with its footage and is cut at the new end.
    assert_eq!(span(item_by_id(&p, "tied")), (20.0, 26.0));
    // Placed after the end: 6 s after it, as before.
    assert_eq!(span(item_by_id(&p, "parked")), (32.0, 34.0));
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
fn removing_or_trimming_a_clip_keeps_free_layers_and_ends_those_past_the_end() {
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

    // Clip a's end trimmed by 2 s (18–20 s goes): the video ends at 28 s, so both keep their time.
    let trimmed = after(
        &p,
        &[Op::TrimClip {
            id: "a".into(),
            edge: Edge::End,
            source_us: secs(18.0),
        }],
    );
    assert_eq!(item_by_id(&trimmed, "t"), item_by_id(&p, "t"));
    assert_eq!(item_by_id(&trimmed, "s"), item_by_id(&p, "s"));

    // Clip b (20–30 s) goes: the video ends at 20 s, so the sound at 22 s goes and the title stays.
    let removed = after(&p, &[Op::RemoveClip { id: "b".into() }]);
    assert!(!has(&removed, "s"));
    assert_eq!(item_by_id(&removed, "t"), item_by_id(&p, "t"));
}

#[test]
fn free_layers_on_a_locked_track_stay_as_they_are() {
    // A lock keeps a track's layers from edits: a cut leaves its free ones where they are, even past the new end, and
    // one placed after the end stays put.
    let mut p = project(vec![(
        TrackKind::Audio,
        "s1",
        vec![
            (sound("s", 20.0, 2.0, 0.0), false),
            (sound("closing", 31.0, 2.0, 0.0), false),
        ],
    )]);
    p.timeline.tracks[0].locked = true;
    let next = after(&p, &[cut(5.0, 30.0)]);
    assert_eq!(item_by_id(&next, "s"), item_by_id(&p, "s"));
    assert_eq!(item_by_id(&next, "closing"), item_by_id(&p, "closing"));
}

#[test]
fn a_cut_and_what_it_ends_undo_as_one_step() {
    let p = project(vec![
        (
            TrackKind::Overlay,
            "free",
            vec![
                (text_item("across", 15.0, 10.0), false),
                (still_item("card", 30.0, 3.0), false),
            ],
        ),
        (
            TrackKind::Audio,
            "s1",
            vec![(sound("late", 22.0, 2.0, 0.0), false)],
        ),
    ]);
    let mut doc = Document::new(p).unwrap();
    let before = doc.project().clone();
    let cleaned = |p: &Project| {
        assert_eq!(span(item_by_id(p, "across")), (15.0, 20.0));
        assert_eq!(span(item_by_id(p, "card")), (20.0, 23.0));
        assert!(!has(p, "late"));
    };
    doc.apply(&tx(vec![cut(10.0, 20.0)])).unwrap();
    cleaned(doc.project());
    doc.undo().unwrap();
    assert_eq!(*doc.project(), before);
    doc.redo().unwrap();
    cleaned(doc.project());
}

#[test]
fn a_recording_clips_separated_sound_follows_its_clip_through_a_cut() {
    // Clip a's mic, separated: sound layers of the recording's own, attached to the clip and tied to the recording.
    let p = after(
        &project(vec![]),
        &[Op::DetachSource {
            clip_id: "a".into(),
            source_id: "mic".into(),
        }],
    );
    let mic = |p: &Project| -> Vec<(f64, f64)> {
        let mut layers: Vec<&Item> = (p.timeline.tracks.iter().flat_map(|t| &t.items))
            .filter(
                |i| matches!(&i.content, ItemContent::Audio { asset_id, .. } if asset_id == "mic"),
            )
            .collect();
        layers.sort_by_key(|i| i.start_us);
        layers.into_iter().map(span).collect()
    };
    let tied = |p: &Project| {
        (p.timeline.tracks.iter().flat_map(|t| &t.items)).all(|i| {
            i.attach.is_some() && matches!(&i.content, ItemContent::Audio { tie: Some(_), .. })
        })
    };
    assert_eq!(mic(&p), [(0.0, 20.0)]);
    assert!(tied(&p));

    // A cut through the clip cuts its sound with it; keeping only the start keeps the sound under it, none past the end.
    let middle = after(&p, &[cut(5.0, 10.0)]);
    assert_eq!(mic(&middle), [(0.0, 5.0), (5.0, 15.0)]);
    assert!(tied(&middle));
    let start = after(&p, &[cut(5.0, 30.0)]);
    assert_eq!(mic(&start), [(0.0, 5.0)]);
    assert!(tied(&start));
    assert_nothing_past_the_end(&start);
    // Removing the clip takes its sound.
    assert!(mic(&after(&p, &[Op::RemoveClip { id: "a".into() }])).is_empty());

    // Freed by the person, it keeps its time like any free layer, and is trimmed to a new end before its own.
    let id = p.timeline.tracks[0].items[0].id.clone();
    let freed = after(
        &p,
        &[Op::SetAttach {
            id: id.clone(),
            attached: false,
        }],
    );
    assert_eq!(mic(&after(&freed, &[cut(5.0, 10.0)])), [(0.0, 20.0)]);
    let short = after(&freed, &[cut(5.0, 30.0)]);
    assert_eq!(mic(&short), [(0.0, 5.0)]);
    assert_eq!(item_by_id(&short, &id).attach, None);
}

#[test]
fn an_end_card_after_the_video_moves_with_its_end_and_the_music_follows() {
    // A 3:37.5 video, a free end card after it (3:37.5 for 4 s), a free whoosh at 1:40 and music to the video's end
    // (3:41.5, the card's). Keeping only 0–25.5 s puts the card right after the footage: no black before it.
    let mut p = project(vec![
        (
            TrackKind::Overlay,
            "Overlay 1",
            vec![(still_item("card", 217.5, 4.0), false)],
        ),
        (
            TrackKind::Audio,
            "Audio 1",
            vec![(sound("whoosh-layer", 100.0, 5.0, 0.0), false)],
        ),
    ]);
    let mut take = p.asset("screen").unwrap().clone();
    take.id = "take".into();
    take.duration_us = secs(600.0);
    p.assets.push(take);
    let mut clip = Clip::new(ClipSource::Asset { id: "take".into() }, 0, secs(217.5));
    clip.id = "clipA".into();
    p.timeline.main = vec![clip];
    let song = p.asset("music").unwrap().clone();
    let p = after(&p, &add_music_ops(&p, &song, MusicOpts::default()).unwrap());
    let music = |p: &Project| -> Vec<(f64, f64)> {
        let mut m: Vec<&Item> = (p.timeline.tracks.iter().flat_map(|t| &t.items))
            .filter(|i| is_music(i))
            .collect();
        m.sort_by_key(|i| i.start_us);
        m.into_iter().map(span).collect()
    };
    assert_eq!(video_duration(&p.timeline), secs(221.5));
    assert_eq!(music(&p).last().map(|s| s.1), Some(221.5));

    let ops = with_music_refit(&p, vec![cut(25.5, 217.5)]).unwrap();
    let next = after(&p, &ops);
    assert_eq!(span(item_by_id(&next, "card")), (25.5, 29.5));
    assert_eq!(main_track_duration(&next.timeline.main), secs(25.5));
    assert_eq!(video_duration(&next.timeline), secs(29.5));
    assert!(!has(&next, "whoosh-layer"));
    assert_eq!(
        music(&next),
        [(0.0, 29.5)],
        "the music plays on under the card, to the new end"
    );

    let review = tx(ops).review(&p).unwrap();
    assert_eq!(review.removals.moved, ["card"]);
    assert!(review.removals.items.contains(&"whoosh-layer".to_string()));
}

#[test]
fn a_one_second_cut_keeps_what_follows_the_video_one_second_earlier() {
    // A card and a closing song placed after the 30 s video: removing a 1 s pause brings both 1 s earlier.
    let p = project(vec![(
        TrackKind::Overlay,
        "free",
        vec![(still_item("card", 30.0, 3.0), false)],
    )]);
    let song = p.asset("music").unwrap().clone();
    let closing = MusicOpts {
        start_us: secs(30.0),
        ..MusicOpts::default()
    };
    let p = after(&p, &add_music_ops(&p, &song, closing).unwrap());
    let music_id = (p.timeline.tracks.iter().flat_map(|t| &t.items))
        .find(|i| is_music(i))
        .map(|i| i.id.clone())
        .unwrap();
    assert_eq!(span(item_by_id(&p, &music_id)), (30.0, 33.0));

    let ops = with_music_refit(&p, vec![cut(10.0, 11.0)]).unwrap();
    let next = after(&p, &ops);
    assert_eq!(span(item_by_id(&next, "card")), (29.0, 32.0));
    assert_eq!(span(item_by_id(&next, &music_id)), (29.0, 32.0));
    assert_eq!(video_duration(&next.timeline), secs(32.0));
    assert_eq!(
        tx(ops).review(&p).unwrap().removals.moved,
        ["card".to_string(), music_id]
    );
}

#[test]
fn a_card_the_video_grew_under_goes_when_that_footage_is_cut() {
    // A card placed after the 30 s video, then a 10 s clip added at the end: the card now sits over that clip, so
    // removing the clip again removes the card with it (undo brings both back).
    let p = project(vec![(
        TrackKind::Overlay,
        "free",
        vec![(still_item("card", 30.0, 3.0), false)],
    )]);
    let mut clip = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(10.0));
    clip.id = "x".into();
    let grown = after(&p, &[Op::InsertClip { clip, index: None }]);
    assert_eq!(item_by_id(&grown, "card"), item_by_id(&p, "card"));
    let back = after(&grown, &[Op::RemoveClip { id: "x".into() }]);
    assert!(!has(&back, "card"));
}
