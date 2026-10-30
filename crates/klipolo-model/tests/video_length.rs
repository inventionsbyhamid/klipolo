//! A video lasts until the later of the main track's end and the latest visual layer's end: a
//! project made only of layers has a length, a layer past the main track's end makes the video longer, and sound
//! layers never do. Hiding a track (or solo hiding it) is a view switch: the length stays.

mod common;
use common::*;
use klipolo_model::history::Document;
use klipolo_model::ops::TrackTarget;
use klipolo_model::summary::ProjectSummary;
use klipolo_model::timeline::{Clip, ClipSource, ItemContent};
use klipolo_model::{Op, Project, Transaction};

fn add(item: klipolo_model::timeline::Item) -> Transaction {
    Transaction {
        label: "Add a layer".into(),
        ops: vec![Op::AddItem {
            track: TrackTarget::Auto,
            item,
            attach: false,
        }],
        origin: None,
        ..Default::default()
    }
}

fn waves(id: &str, start: f64, len: f64) -> klipolo_model::timeline::Item {
    item(
        id,
        start,
        len,
        full(),
        ItemContent::generator("klipolo.builtin/generator.waves".into(), Default::default()),
    )
}

fn song(id: &str, start: f64, len: f64) -> klipolo_model::timeline::Item {
    item(
        id,
        start,
        len,
        full(),
        ItemContent::Audio {
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
        },
    )
}

fn length(p: &Project) -> i64 {
    ProjectSummary::of(p, 0).duration_us
}

/// The fixture's media with a main track of one 5 s clip (b-roll 0–5 s).
fn five_seconds() -> Project {
    let mut p = fixture();
    let mut b = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(5.0));
    b.id = "b".into();
    p.timeline.main = vec![b];
    p
}

#[test]
fn a_project_of_only_layers_lasts_until_its_last_layer_ends() {
    // A user's case: a new empty project, a Waves layer 0–10 s and a text 0–10 s.
    let mut doc = Document::new(Project::new("Canvas")).unwrap();
    assert_eq!(length(doc.project()), 0);
    doc.apply(&add(waves("bg", 0.0, 10.0))).unwrap();
    doc.apply(&add(still_item("title", 0.0, 10.0))).unwrap();
    assert_eq!(length(doc.project()), secs(10.0));
}

#[test]
fn a_layer_past_the_main_track_makes_the_video_longer_and_undo_shrinks_it_back() {
    let mut doc = Document::new(five_seconds()).unwrap();
    assert_eq!(length(doc.project()), secs(5.0));
    doc.apply(&add(still_item("title", 3.0, 5.0))).unwrap();
    assert_eq!(length(doc.project()), secs(8.0));
    assert!(doc.undo().is_some());
    assert_eq!(length(doc.project()), secs(5.0));
    assert!(doc.redo().is_some());
    assert_eq!(length(doc.project()), secs(8.0));
    // Shortening the last layer shrinks the video back, in one step.
    doc.apply(&Transaction {
        label: "Shorten".into(),
        ops: vec![Op::UpdateItem {
            id: "title".into(),
            patch: klipolo_model::ops::ItemPatch {
                duration_us: Some(secs(1.0)),
                ..Default::default()
            },
        }],
        origin: None,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(length(doc.project()), secs(5.0));
}

#[test]
fn a_layer_after_a_gap_counts_to_its_end() {
    let mut p = five_seconds();
    p.timeline
        .tracks
        .push(overlay_track("ov", vec![still_item("late", 7.0, 2.0)]));
    assert_eq!(length(&p), secs(9.0));
}

#[test]
fn a_song_longer_than_the_main_track_doesnt_make_the_video_longer() {
    let mut p = five_seconds();
    let mut music =
        klipolo_model::timeline::Track::new(klipolo_model::timeline::TrackKind::Audio, "Music");
    music.items.push(song("song", 0.0, 60.0));
    p.timeline.tracks.push(music);
    assert_eq!(length(&p), secs(5.0));
    // Nor does a zoom past the end: it only moves the main footage.
    let mut zoom =
        klipolo_model::timeline::Track::new(klipolo_model::timeline::TrackKind::Zoom, "Zoom");
    zoom.items.push(item(
        "z",
        2.0,
        20.0,
        full(),
        ItemContent::Zoom {
            target: klipolo_model::timeline::ZoomTarget::Auto { using: vec![] },
            scale: 2.0,
        },
    ));
    p.timeline.tracks.push(zoom);
    assert_eq!(length(&p), secs(5.0));
}

/// An adjustment box only changes what is below it, so an empty one past the main track adds no frames.
#[test]
fn an_adjustment_box_past_the_main_track_doesnt_make_the_video_longer() {
    let mut p = five_seconds();
    p.timeline.tracks.push(overlay_track(
        "fx",
        vec![item("box", 3.0, 7.0, full(), ItemContent::Adjustment)],
    ));
    assert_eq!(length(&p), secs(5.0));
    // A picture layer under the same span does.
    p.timeline
        .tracks
        .push(overlay_track("ov", vec![still_item("title", 3.0, 4.0)]));
    assert_eq!(length(&p), secs(7.0));
}

#[test]
fn hiding_or_soloing_away_the_track_of_the_last_layer_keeps_the_length() {
    let mut p = five_seconds();
    p.timeline
        .tracks
        .push(overlay_track("ov", vec![still_item("title", 3.0, 5.0)]));
    p.timeline
        .tracks
        .push(overlay_track("other", vec![still_item("logo", 0.0, 1.0)]));
    assert_eq!(length(&p), secs(8.0));
    p.timeline.tracks[0].hidden = true;
    assert_eq!(length(&p), secs(8.0));
    p.timeline.tracks[0].hidden = false;
    p.timeline.tracks[1].solo = true;
    assert_eq!(length(&p), secs(8.0));
}

#[test]
fn the_main_track_keeps_its_own_length_and_clips_stop_there() {
    use klipolo_model::query::{anchor_at, clip_at, main_track_duration, video_duration};
    let mut p = five_seconds();
    p.timeline
        .tracks
        .push(overlay_track("ov", vec![still_item("title", 3.0, 5.0)]));
    assert_eq!(main_track_duration(&p.timeline.main), secs(5.0));
    assert_eq!(video_duration(&p.timeline), secs(8.0));
    // Past the main track's end only layers play: no clip there, and nothing anchors to one.
    assert_eq!(clip_at(&p.timeline.main, secs(6.0)), None);
    assert!(anchor_at(&p.timeline.main, secs(6.0)).is_none());
}
