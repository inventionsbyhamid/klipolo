mod common;
use common::*;
use klipolo_model::query::*;
use klipolo_model::timeline::*;

#[test]
fn starts_and_duration() {
    let p = fixture();
    assert_eq!(clip_starts(&p.timeline.main), vec![0, secs(20.0)]);
    assert_eq!(main_track_duration(&p.timeline.main), secs(30.0));
}

#[test]
fn clip_at_is_half_open() {
    let p = fixture();
    assert_eq!(clip_at(&p.timeline.main, 0), Some((0, 0)));
    assert_eq!(clip_at(&p.timeline.main, secs(20.0)), Some((1, secs(20.0))));
    assert_eq!(clip_at(&p.timeline.main, secs(30.0)), None);
}

#[test]
fn anchor_round_trips_and_survives_reorder() {
    let mut p = fixture();
    let anchor = anchor_at(&p.timeline.main, secs(25.0)).unwrap();
    assert_eq!(anchor.clip_id, "b");
    assert_eq!(anchor.source_us, secs(5.0));
    assert_eq!(resolve_attach(&p.timeline.main, &anchor), Some(secs(25.0)));
    p.timeline.main.swap(0, 1);
    assert_eq!(resolve_attach(&p.timeline.main, &anchor), Some(secs(5.0)));
}

#[test]
fn anchor_follows_speed() {
    let mut p = fixture();
    let anchor = anchor_at(&p.timeline.main, secs(25.0)).unwrap();
    p.timeline.main[0].speed = 2.0; // clip a now lasts 10 s
    assert_eq!(resolve_attach(&p.timeline.main, &anchor), Some(secs(15.0)));
}

#[test]
fn anchor_at_end_of_timeline() {
    let p = fixture();
    let anchor = anchor_at(&p.timeline.main, secs(30.0)).unwrap();
    assert_eq!(anchor.source_us, secs(10.0));
    assert_eq!(resolve_attach(&p.timeline.main, &anchor), Some(secs(30.0)));
    assert!(anchor_at(&p.timeline.main, secs(31.0)).is_none());
}

#[test]
fn unresolvable_anchor() {
    let p = fixture();
    let gone = Attach {
        clip_id: "x".into(),
        source: ClipSource::Asset { id: "broll".into() },
        source_us: secs(50.0),
    };
    assert_eq!(resolve_attach(&p.timeline.main, &gone), None);
}

#[test]
fn source_durations_and_fit() {
    let p = fixture();
    assert_eq!(
        source_duration_us(&p, &ClipSource::Recording { id: "rec".into() }),
        Some(secs(60.0))
    );
    assert_eq!(
        source_duration_us(&p, &ClipSource::Asset { id: "nope".into() }),
        None
    );
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.items.push(text_item("t", 2.0, 2.0));
    assert!(fits(&track, 0, secs(2.0), None));
    assert!(!fits(&track, secs(3.0), secs(5.0), None));
    assert!(fits(&track, secs(3.0), secs(5.0), Some("t")));
}

fn rec_clip(id: &str, from: f64, to: f64) -> Clip {
    let mut c = Clip::new(
        ClipSource::Recording { id: "rec".into() },
        secs(from),
        secs(to),
    );
    c.id = id.into();
    c
}

fn rec_attach(clip_id: &str, source_secs: f64) -> Attach {
    Attach {
        clip_id: clip_id.into(),
        source: ClipSource::Recording { id: "rec".into() },
        source_us: secs(source_secs),
    }
}

#[test]
fn anchor_stays_on_its_copy_of_duplicated_footage() {
    let mut p = fixture();
    p.timeline.main = vec![rec_clip("a", 0.0, 20.0), rec_clip("c", 0.0, 20.0)];
    let anchor = anchor_at(&p.timeline.main, secs(35.0)).unwrap();
    assert_eq!(anchor.clip_id, "c");
    assert_eq!(anchor.source_us, secs(15.0));
    assert_eq!(resolve_attach(&p.timeline.main, &anchor), Some(secs(35.0)));
    // Split c at timeline 30 s: the left half keeps id "c", the right half is new clip "c2".
    p.timeline.main[1].source_out_us = secs(10.0);
    p.timeline.main.push(rec_clip("c2", 10.0, 20.0));
    assert_eq!(resolve_attach(&p.timeline.main, &anchor), Some(secs(35.0)));
}

#[test]
fn anchor_prefers_nearest_copy() {
    let mut p = fixture();
    // [c1 rec 0–20 | c2 rec 0–20 | o rec 20–30 | b broll 0–10 | c3 rec 0–20]
    p.timeline.main = vec![
        rec_clip("c1", 0.0, 20.0),
        rec_clip("c2", 0.0, 20.0),
        rec_clip("o", 20.0, 30.0),
        fixture().timeline.main[1].clone(),
        rec_clip("c3", 0.0, 20.0),
    ];
    // o no longer covers 10 s; c2 (one away) beats c1 and c3 (two away).
    assert_eq!(
        resolve_attach(&p.timeline.main, &rec_attach("o", 10.0)),
        Some(secs(30.0))
    );
}

#[test]
fn own_clip_end_wins_over_other_copy() {
    let mut p = fixture();
    // [a rec 0–20 | b broll 0–10 | c rec 10–30]: c covers rec 20 s, but the anchor sits at a's end.
    p.timeline.main.push(rec_clip("c", 10.0, 30.0));
    assert_eq!(
        resolve_attach(&p.timeline.main, &rec_attach("a", 20.0)),
        Some(secs(20.0))
    );
}

#[test]
fn anchor_falls_back_to_first_copy_when_its_clip_is_gone() {
    let mut p = fixture();
    p.timeline.main.push(rec_clip("c", 0.0, 20.0));
    p.timeline.main.push(rec_clip("d", 0.0, 20.0));
    let anchor = anchor_at(&p.timeline.main, secs(5.0)).unwrap();
    assert_eq!(anchor.clip_id, "a");
    p.timeline.main.remove(0); // [b 0–10 s | c 10–30 s | d 30–50 s]
    assert_eq!(resolve_attach(&p.timeline.main, &anchor), Some(secs(15.0)));
}

#[test]
fn fits_tolerates_sub_frame_overlaps() {
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.items.push(text_item("t", 2.0, 2.0)); // [2 s, 4 s)
    assert!(fits(&track, 0, secs(2.0) + 4, None));
    assert!(!fits(&track, 0, secs(2.0) + 5, None));
    assert!(fits(&track, secs(4.0) - 4, secs(6.0), None));
    assert!(!fits(&track, secs(4.0) - 5, secs(6.0), None));
    assert!(
        !fits(&track, secs(2.5), secs(3.0), None),
        "a range overlapping an item by more than the tolerance doesn't fit, even when inside it"
    );
}

#[test]
fn empty_or_reversed_range_never_fits() {
    let track = Track::new(TrackKind::Overlay, "Overlay 1");
    assert!(!fits(&track, secs(3.0), secs(3.0), None));
    assert!(!fits(&track, secs(5.0), secs(3.0), None));
}

#[test]
fn find_item_locates_track_and_index() {
    let mut p = fixture();
    let mut o1 = Track::new(TrackKind::Overlay, "Overlay 1");
    o1.items.push(text_item("t1", 0.0, 2.0));
    let mut o2 = Track::new(TrackKind::Overlay, "Overlay 2");
    o2.items.push(text_item("t2", 0.0, 2.0));
    o2.items.push(video_item("v", 5.0, 2.0, 0.0));
    p.timeline.tracks = vec![o1, o2];
    assert_eq!(find_item(&p, "t1"), Some((0, 0)));
    assert_eq!(find_item(&p, "v"), Some((1, 1)));
    assert_eq!(find_item(&p, "nope"), None);
}

#[test]
fn unknown_recording_has_no_duration() {
    let p = fixture();
    assert_eq!(
        source_duration_us(&p, &ClipSource::Recording { id: "nope".into() }),
        None
    );
}

#[test]
fn empty_main_and_negative_times() {
    assert_eq!(main_track_duration(&[]), 0);
    assert_eq!(clip_at(&[], 0), None);
    assert!(anchor_at(&[], 0).is_none());
    let p = fixture();
    assert_eq!(clip_at(&p.timeline.main, -1), None);
    assert!(anchor_at(&p.timeline.main, -1).is_none());
}
