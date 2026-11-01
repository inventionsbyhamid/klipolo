mod common;
use common::*;
use klipolo_model::ops::*;
use klipolo_model::query::*;
use klipolo_model::time::MAX_TIME_US;
use klipolo_model::timeline::*;
use klipolo_model::{apply_ops, Project, RangeUs};

const FRAME: i64 = 33_334;

fn with_items(mut p: Project, items: Vec<(Item, bool)>) -> Project {
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.id = "track".into();
    for (mut item, attached) in items {
        if attached {
            item.attach = anchor_at(&p.timeline.main, item.start_us);
        }
        track.items.push(item);
    }
    track.items.sort_by_key(|i| i.start_us);
    p.timeline.tracks.push(track);
    p
}

fn item<'a>(p: &'a Project, id: &str) -> Option<&'a Item> {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
}

fn range(a: f64, b: f64) -> RangeUs {
    RangeUs {
        start: secs(a),
        end: secs(b),
    }
}

#[test]
fn insert_and_move_clips() {
    let p = fixture();
    let clip = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(2.0));
    let id = clip.id.clone();
    let p = apply_ops(
        &p,
        &[Op::InsertClip {
            clip,
            index: Some(0),
        }],
    )
    .unwrap();
    assert_eq!(p.timeline.main[0].id, id);
    let p = apply_ops(
        &p,
        &[Op::MoveClip {
            id: id.clone(),
            index: 99,
        }],
    )
    .unwrap();
    assert_eq!(p.timeline.main.last().unwrap().id, id);
}

#[test]
fn split_keeps_source_contiguous() {
    let mut p = fixture();
    p.timeline.main[0].fade_in_us = secs(1.0);
    p.timeline.main[0].fade_out_us = secs(1.0);
    let p = apply_ops(&p, &[Op::Split { at_us: secs(5.0) }]).unwrap();
    let (a1, a2) = (&p.timeline.main[0], &p.timeline.main[1]);
    assert_eq!(a1.id, "a");
    assert_ne!(a2.id, "a");
    assert_eq!((a1.source_out_us, a2.source_in_us), (secs(5.0), secs(5.0)));
    assert_eq!(
        (a1.fade_in_us, a1.fade_out_us, a2.fade_in_us, a2.fade_out_us),
        (secs(1.0), 0, 0, secs(1.0))
    );
    assert_eq!(main_track_duration(&p.timeline.main), secs(30.0));
    assert!(
        apply_ops(
            &p,
            &[Op::Split {
                at_us: secs(5.0) + 1
            }]
        )
        .is_err(),
        "too close to an edge"
    );
}

#[test]
fn delete_range_ripples_attached_but_not_detached() {
    let p = with_items(
        fixture(),
        vec![
            (text_item("att", 15.0, 2.0), true),
            (text_item("free", 18.0, 1.0), false),
        ],
    );
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![range(5.0, 10.0)],
        }],
    )
    .unwrap();
    assert_eq!(main_track_duration(&p.timeline.main), secs(25.0));
    assert_eq!(p.timeline.main[0].id, "a", "first piece keeps the id");
    assert_eq!(item(&p, "att").unwrap().start_us, secs(10.0));
    assert_eq!(item(&p, "free").unwrap().start_us, secs(18.0));
}

#[test]
fn delete_range_removes_or_shortens_static_items() {
    let p = with_items(
        fixture(),
        vec![
            (text_item("inside", 6.0, 2.0), true),
            (text_item("straddle", 9.0, 3.0), true),
        ],
    );
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![range(5.0, 10.0)],
        }],
    )
    .unwrap();
    assert!(item(&p, "inside").is_none());
    let s = item(&p, "straddle").unwrap();
    assert_eq!((s.start_us, s.duration_us), (secs(5.0), secs(2.0)));
}

#[test]
fn delete_range_splits_attached_video() {
    let p = with_items(fixture(), vec![(video_item("v", 2.0, 6.0, 1.0), true)]);
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![range(4.0, 5.0)],
        }],
    )
    .unwrap();
    let pieces: Vec<&Item> = p.timeline.tracks[0].items.iter().collect();
    assert_eq!(pieces.len(), 2);
    assert_eq!(
        (pieces[0].start_us, pieces[0].duration_us),
        (secs(2.0), secs(2.0))
    );
    assert_eq!(
        (pieces[1].start_us, pieces[1].duration_us),
        (secs(4.0), secs(3.0))
    );
    match &pieces[1].content {
        ItemContent::Video { source_in_us, .. } => assert_eq!(*source_in_us, secs(4.0)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn messy_ranges_are_merged_and_clamped() {
    let p = fixture();
    // Range bounds must lie in [0, MAX_TIME_US], so `range(-3.0, 1.0)` is refused (see
    // `out_of_range_times_are_refused_without_panicking`); it starts at 0 here. Ends past the timeline are still
    // clamped.
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![range(12.0, 40.0), range(0.0, 1.0), range(11.0, 13.0)],
        }],
    )
    .unwrap();
    assert_eq!(main_track_duration(&p.timeline.main), secs(10.0));
    assert!(apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![range(50.0, 60.0)]
        }]
    )
    .is_err());
}

#[test]
fn deleting_everything_leaves_valid_empty_timeline() {
    let p = with_items(
        fixture(),
        vec![
            (text_item("t", 3.0, 2.0), true),
            (text_item("free", 3.0, 2.0), false),
        ],
    );
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![range(0.0, 30.0)],
        }],
    )
    .unwrap();
    assert!(p.timeline.main.is_empty());
    assert!(item(&p, "t").is_none());
    assert!(item(&p, "free").is_some());
}

#[test]
fn fades_are_clamped_after_cuts() {
    let mut p = fixture();
    p.timeline.main[1].fade_in_us = secs(4.0);
    p.timeline.main[1].fade_out_us = secs(4.0);
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![range(21.0, 28.0)],
        }],
    )
    .unwrap();
    let b = p.timeline.main.iter().find(|c| c.id == "b").unwrap();
    assert!(b.fade_in_us + b.fade_out_us <= b.duration_us());
}

#[test]
fn trim_start_inward_ripples_and_outward_extends() {
    let p = with_items(fixture(), vec![(text_item("t", 22.0, 1.0), true)]);
    let p = apply_ops(
        &p,
        &[Op::TrimClip {
            id: "a".into(),
            edge: Edge::Start,
            source_us: secs(4.0),
        }],
    )
    .unwrap();
    assert_eq!(p.timeline.main[0].source_in_us, secs(4.0));
    assert_eq!(item(&p, "t").unwrap().start_us, secs(18.0));
    let p = apply_ops(
        &p,
        &[Op::TrimClip {
            id: "a".into(),
            edge: Edge::End,
            source_us: secs(30.0),
        }],
    )
    .unwrap();
    assert_eq!(p.timeline.main[0].source_out_us, secs(30.0));
    assert_eq!(item(&p, "t").unwrap().start_us, secs(28.0));
    assert!(apply_ops(
        &p,
        &[Op::TrimClip {
            id: "a".into(),
            edge: Edge::End,
            source_us: secs(4.0)
        }]
    )
    .is_err());
}

#[test]
fn remove_clip_takes_its_attached_items() {
    let p = with_items(
        fixture(),
        vec![
            (text_item("on_a", 3.0, 1.0), true),
            (text_item("on_b", 25.0, 1.0), true),
        ],
    );
    let p = apply_ops(&p, &[Op::RemoveClip { id: "a".into() }]).unwrap();
    assert!(item(&p, "on_a").is_none());
    assert_eq!(item(&p, "on_b").unwrap().start_us, secs(5.0));
}

#[test]
fn speed_and_move_carry_attached_items() {
    let p = with_items(fixture(), vec![(text_item("t", 22.0, 1.0), true)]);
    let p2 = apply_ops(
        &p,
        &[Op::UpdateClip {
            id: "a".into(),
            patch: ClipPatch {
                speed: Some(2.0),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(item(&p2, "t").unwrap().start_us, secs(12.0));
    let p3 = apply_ops(
        &p,
        &[Op::MoveClip {
            id: "b".into(),
            index: 0,
        }],
    )
    .unwrap();
    assert_eq!(item(&p3, "t").unwrap().start_us, secs(2.0));
}

#[test]
fn non_unit_speed_does_not_drift() {
    let mut p = fixture();
    p.timeline.main[0].speed = 1.5;
    let p = with_items(p, vec![(text_item("t", 4.0, 1.0), true)]);
    let p = apply_ops(
        &p,
        &[
            Op::Split { at_us: secs(2.0) },
            Op::Split { at_us: secs(6.0) },
            Op::DeleteRanges {
                ranges: vec![range(7.0, 8.0)],
            },
            Op::Split { at_us: secs(3.0) },
        ],
    )
    .unwrap();
    assert!((item(&p, "t").unwrap().start_us - secs(4.0)).abs() <= FRAME);
}

// --- Attached layers stay on the piece that holds their content ---

fn attach<'a>(p: &'a Project, id: &str) -> &'a Attach {
    item(p, id).and_then(|i| i.attach.as_ref()).unwrap()
}

#[test]
fn split_moves_later_attachments_to_the_new_piece() {
    let mut p = with_items(
        fixture(),
        vec![
            (text_item("before", 1.0, 1.0), true),
            (text_item("at_cut", 5.0, 1.0), true),
            (text_item("after", 12.0, 1.0), true),
        ],
    );
    // Anchored to the very end of clip a (where b starts on the timeline).
    let mut at_end = text_item("at_end", 20.0, 1.0);
    at_end.attach = Some(Attach {
        clip_id: "a".into(),
        source: ClipSource::Recording { id: "rec".into() },
        source_us: secs(20.0),
    });
    p.timeline.tracks[0].items.push(at_end);

    let next = apply_ops(&p, &[Op::Split { at_us: secs(5.0) }]).unwrap();
    let second = next.timeline.main[1].id.clone();
    assert_eq!(attach(&next, "before").clip_id, "a");
    for id in ["at_cut", "after", "at_end"] {
        let before = item(&p, id).unwrap().start_us;
        assert_eq!(
            attach(&next, id).clip_id,
            second,
            "{id} moves to the right piece"
        );
        assert_eq!(
            resolve_attach(&next.timeline.main, attach(&next, id)),
            Some(before),
            "{id} stays put"
        );
        assert_eq!(item(&next, id).unwrap().start_us, before);
    }

    // main: right piece (source 5–20 s, 0–15 s), a (0–5 s, 15–20 s), b (20–30 s).
    let moved = apply_ops(
        &next,
        &[Op::MoveClip {
            id: second,
            index: 0,
        }],
    )
    .unwrap();
    assert_eq!(item(&moved, "at_cut").unwrap().start_us, 0);
    assert_eq!(item(&moved, "after").unwrap().start_us, secs(7.0));
    assert_eq!(item(&moved, "at_end").unwrap().start_us, secs(15.0));
    assert_eq!(item(&moved, "before").unwrap().start_us, secs(16.0));
}

#[test]
fn deleting_inside_a_clip_keeps_later_attachments_on_the_right_piece() {
    let p = with_items(fixture(), vec![(text_item("t", 12.0, 1.0), true)]);
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![range(5.0, 8.0)],
        }],
    )
    .unwrap();
    let right = &p.timeline.main[1];
    assert_eq!((right.id.as_str(), right.source_in_us), ("a-2", secs(8.0)));
    assert_eq!(attach(&p, "t").clip_id, "a-2");
    assert_eq!(item(&p, "t").unwrap().start_us, secs(9.0));
    // main: a (0–5 s), b (5–15 s), a-2 (source 8–20 s, 15–27 s).
    let p = apply_ops(
        &p,
        &[Op::MoveClip {
            id: "a-2".into(),
            index: 2,
        }],
    )
    .unwrap();
    assert_eq!(item(&p, "t").unwrap().start_us, secs(19.0));
}

#[test]
fn slivers_dropped_by_a_cut_ripple_layers_too() {
    // Removing all but 5 ms of clip a drops that sliver, so layers on b move by the full 20 s.
    let p = with_items(fixture(), vec![(text_item("t", 21.0, 1.0), true)]);
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: 0,
                end: secs(20.0) - 5_000,
            }],
        }],
    )
    .unwrap();
    assert_eq!(
        p.timeline
            .main
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["b"]
    );
    assert_eq!(item(&p, "t").unwrap().start_us, secs(1.0));
    assert_eq!(attach(&p, "t").source_us, secs(1.0));
}

#[test]
fn attached_layers_ripple_from_their_anchor_even_if_the_stored_start_is_stale() {
    let mut p = with_items(fixture(), vec![(text_item("t", 12.0, 1.0), true)]);
    p.timeline.tracks[0].items[0].start_us = 0; // as in a hand-edited file; the anchor says 12 s
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![range(5.0, 8.0)],
        }],
    )
    .unwrap();
    assert_eq!(item(&p, "t").unwrap().start_us, secs(9.0));
}

// --- Derived pieces get deterministic ids ---

#[test]
fn same_ops_give_identical_projects() {
    let p = with_items(
        fixture(),
        vec![
            (video_item("v", 2.0, 6.0, 1.0), true),
            (text_item("free", 13.5, 1.0), false),
            (text_item("att", 15.0, 2.0), true),
        ],
    );
    // The cut splits clip a and layer v, and ripples "att" onto "free", bumping it to a new track.
    let ops = [
        Op::DeleteRanges {
            ranges: vec![range(4.0, 5.0)],
        },
        Op::Split { at_us: secs(2.0) },
    ];
    let once = apply_ops(&p, &ops).unwrap();
    assert_eq!(
        once,
        apply_ops(&p, &ops).unwrap(),
        "dry run and commit agree"
    );
    let clips: Vec<&str> = once.timeline.main.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(clips, vec!["a", "a-3", "a-2", "b"]);
    assert!(item(&once, "v-2").is_some());
    assert_eq!(once.timeline.tracks[1].id, "att-track");
    assert_eq!(once.timeline.tracks[1].items[0].id, "att");
}

#[test]
fn derived_ids_skip_ids_already_in_use() {
    let p = with_items(fixture(), vec![(text_item("a-2", 25.0, 1.0), false)]);
    let p = apply_ops(
        &p,
        &[
            Op::Split { at_us: secs(5.0) },
            Op::Split { at_us: secs(10.0) },
        ],
    )
    .unwrap();
    let clips: Vec<&str> = p.timeline.main.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(clips, vec!["a", "a-3", "a-3-2", "b"]);
}

// --- Op times are bounded before arithmetic ---

#[test]
fn out_of_range_times_are_refused_without_panicking() {
    let p = fixture();
    let outside = [
        Op::Split { at_us: i64::MAX },
        Op::Split { at_us: -1 },
        Op::TrimClip {
            id: "a".into(),
            edge: Edge::Start,
            source_us: i64::MIN,
        },
        Op::TrimClip {
            id: "a".into(),
            edge: Edge::End,
            source_us: MAX_TIME_US + 1,
        },
        Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: secs(1.0),
                end: i64::MAX,
            }],
        },
        Op::DeleteRanges {
            ranges: vec![range(-3.0, 1.0)],
        },
    ];
    for op in outside {
        assert_eq!(
            apply_ops(&p, std::slice::from_ref(&op)),
            Err(refused("That time is outside the video.")),
            "{op:?}"
        );
    }
    assert_eq!(
        apply_ops(&p, &[Op::Split { at_us: MAX_TIME_US }]),
        Err(refused("There's no clip at that time to split."))
    );
}

#[test]
fn clip_indices_clamp() {
    let clip = Clip::new(ClipSource::Asset { id: "broll".into() }, 0, secs(2.0));
    let id = clip.id.clone();
    let p = apply_ops(
        &fixture(),
        &[Op::InsertClip {
            clip,
            index: Some(usize::MAX),
        }],
    )
    .unwrap();
    assert_eq!(p.timeline.main.last().unwrap().id, id);
    let p = apply_ops(
        &p,
        &[Op::MoveClip {
            id: "a".into(),
            index: usize::MAX,
        }],
    )
    .unwrap();
    assert_eq!(p.timeline.main.last().unwrap().id, "a");
}

/// Projects that fail validation in ways that stress op arithmetic (ops apply before validation).
fn corrupt_projects() -> Vec<Project> {
    // Huge times: clip spans, starts and layer pieces saturate.
    let mut huge = fixture();
    let a = &mut huge.timeline.main[0];
    a.source_out_us = i64::MAX;
    a.fade_in_us = i64::MIN;
    a.fade_out_us = i64::MAX;
    huge.timeline.main[1].source_in_us = i64::MIN;
    let mut audio = still_item("aud", 1.0, 2.0);
    audio.duration_us = i64::MAX;
    audio.content = ItemContent::Audio {
        asset_id: "music".into(),
        source_in_us: i64::MAX,
        volume: 1.0,
        muted: false,
        fade_in_us: i64::MIN,
        fade_out_us: i64::MAX,
        effect: None,
        drift_ppm: 0.0,
        centre_from: None,
        tie: None,
        duck: None,
    };
    audio.attach = Some(Attach {
        clip_id: "a".into(),
        source: ClipSource::Recording { id: "rec".into() },
        source_us: secs(1.0),
    });
    let mut track = Track::new(TrackKind::Audio, "Audio 1");
    track.items.push(audio);
    huge.timeline.tracks.push(track);

    // Degenerate speeds: negative, infinite and empty durations.
    let mut negative = fixture();
    negative.timeline.main[1].source_in_us = secs(5.0);
    negative.timeline.main[1].speed = -1.0;
    let mut degenerate = fixture();
    degenerate.timeline.main[0].speed = 0.0;
    degenerate.timeline.main[1].speed = f64::NAN;
    // A huge clip that starts after another one.
    let mut late = fixture();
    late.timeline.main[1].source_out_us = i64::MAX;
    vec![huge, late, negative, degenerate]
}

#[test]
fn ops_on_corrupt_projects_never_panic() {
    let ops = [
        Op::Split { at_us: secs(1.0) },
        Op::Split { at_us: secs(25.0) },
        Op::DeleteRanges {
            ranges: vec![range(1.0, 2.0)],
        },
        Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: 0,
                end: MAX_TIME_US,
            }],
        },
        Op::TrimClip {
            id: "a".into(),
            edge: Edge::Start,
            source_us: secs(1.0),
        },
        Op::TrimClip {
            id: "b".into(),
            edge: Edge::Start,
            source_us: secs(1.0),
        },
        Op::TrimClip {
            id: "b".into(),
            edge: Edge::End,
            source_us: secs(1.0),
        },
        Op::RemoveClip { id: "a".into() },
        Op::RemoveClip { id: "b".into() },
        Op::MoveClip {
            id: "b".into(),
            index: 0,
        },
        Op::UpdateClip {
            id: "a".into(),
            patch: ClipPatch {
                volume: Some(0.5),
                ..Default::default()
            },
        },
    ];
    for p in corrupt_projects() {
        for op in &ops {
            // Ok (e.g. the corrupt clips were cut away) or Err, but never a panic.
            let _ = apply_ops(&p, std::slice::from_ref(op));
        }
    }
}

// --- JSON shapes ---

#[test]
fn main_op_json_shapes() {
    let update = Op::UpdateClip {
        id: "a".into(),
        patch: ClipPatch {
            fade_in_us: Some(500_000),
            ..Default::default()
        },
    };
    let json = serde_json::to_value(&update).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"type": "updateClip", "id": "a", "patch": {"fadeInUs": 500000}})
    );
    assert_eq!(serde_json::from_value::<Op>(json).unwrap(), update);
    assert_eq!(
        serde_json::to_value(Op::TrimClip {
            id: "a".into(),
            edge: Edge::End,
            source_us: 7
        })
        .unwrap(),
        serde_json::json!({"type": "trimClip", "id": "a", "edge": "end", "sourceUs": 7})
    );
    assert_eq!(
        serde_json::to_value(Op::Split { at_us: 5 }).unwrap(),
        serde_json::json!({"type": "split", "atUs": 5})
    );
    assert_eq!(
        serde_json::to_value(Op::DeleteRanges {
            ranges: vec![RangeUs { start: 1, end: 2 }]
        })
        .unwrap(),
        serde_json::json!({"type": "deleteRanges", "ranges": [{"start": 1, "end": 2}]})
    );
}

// --- Fix 1: layers are re-anchored in source time, so speed rounding never changes their clip ---

/// Fixture with clip a at `speed` and a title attached at the very start of clip b.
fn title_on_b(speed: f64) -> Project {
    let mut p = fixture();
    p.timeline.main[0].speed = speed;
    let mut title = text_item("title", 0.0, 1.0);
    title.start_us = clip_starts(&p.timeline.main)[1];
    with_items(p, vec![(title, true)])
}

/// The title is still anchored to the start of b, sits there, and moves with b.
fn check_title_on_b(p: &Project, what: &str) {
    let b = p.timeline.main.iter().position(|c| c.id == "b").unwrap();
    assert_eq!(attach(p, "title").clip_id, "b", "{what}");
    assert_eq!(attach(p, "title").source_us, 0, "{what}");
    assert_eq!(
        item(p, "title").unwrap().start_us,
        clip_starts(&p.timeline.main)[b],
        "{what}"
    );
    let moved = apply_ops(
        p,
        &[Op::MoveClip {
            id: "b".into(),
            index: 0,
        }],
    )
    .unwrap();
    assert_eq!(
        item(&moved, "title").unwrap().start_us,
        0,
        "{what}: moving b carries the title"
    );
}

#[test]
fn a_title_at_the_start_of_the_next_clip_stays_there() {
    let trim = |edge, source_us| Op::TrimClip {
        id: "a".into(),
        edge,
        source_us,
    };
    let delete = |start, end| Op::DeleteRanges {
        ranges: vec![RangeUs { start, end }],
    };
    let cases = [
        (1.5, trim(Edge::End, secs(19.0))),
        (1.5, trim(Edge::Start, 390_001)),
        (1.5, delete(secs(5.0), secs(6.0))),
        (0.3, trim(Edge::End, secs(19.0))),
        (0.3, trim(Edge::Start, 390_001)),
        (0.3, delete(69_936, 103_290)),
    ];
    for (speed, op) in cases {
        let next = apply_ops(&title_on_b(speed), std::slice::from_ref(&op)).unwrap();
        check_title_on_b(&next, &format!("{speed}x {op:?}"));
    }
}

#[test]
fn layer_ownership_survives_cut_sweeps() {
    for speed in [1.5, 0.25] {
        let p = title_on_b(speed);
        let a_len = p.timeline.main[0].duration_us();
        for n in 1..=50_i64 {
            let source = n * 390_001;
            let at = n * (a_len / 51) + n;
            let ops = [
                Op::TrimClip {
                    id: "a".into(),
                    edge: Edge::End,
                    source_us: source,
                },
                Op::TrimClip {
                    id: "a".into(),
                    edge: Edge::Start,
                    source_us: source,
                },
                Op::DeleteRanges {
                    ranges: vec![RangeUs {
                        start: at,
                        end: at + 33_367 + 7 * n,
                    }],
                },
            ];
            for op in ops {
                let next = apply_ops(&p, std::slice::from_ref(&op)).unwrap();
                check_title_on_b(&next, &format!("{speed}x {op:?}"));
            }
        }
    }
}

// --- Fix 1 minors ---

#[test]
fn speeding_up_clamps_fades_unless_the_patch_sets_them() {
    let mut p = fixture();
    p.timeline.main[0].fade_in_us = secs(4.0);
    p.timeline.main[0].fade_out_us = secs(4.0);
    let speed = |fades: Option<(i64, i64)>| Op::UpdateClip {
        id: "a".into(),
        patch: ClipPatch {
            speed: Some(4.0),
            fade_in_us: fades.map(|f| f.0),
            fade_out_us: fades.map(|f| f.1),
            ..Default::default()
        },
    };
    let next = apply_ops(&p, &[speed(None)]).unwrap();
    let a = &next.timeline.main[0];
    assert_eq!(a.duration_us(), secs(5.0));
    assert_eq!((a.fade_in_us, a.fade_out_us), (secs(4.0), secs(1.0)));
    assert_eq!(
        apply_ops(&p, &[speed(Some((secs(4.0), secs(4.0))))]),
        Err(refused("Clip fades can't be longer than the clip."))
    );
}

#[test]
fn speeding_up_with_one_fade_set_clamps_the_other() {
    let mut p = fixture();
    p.timeline.main[0].fade_in_us = secs(4.0);
    p.timeline.main[0].fade_out_us = secs(4.0);
    let speed = |fade_in_us, fade_out_us| Op::UpdateClip {
        id: "a".into(),
        patch: ClipPatch {
            speed: Some(4.0),
            fade_in_us,
            fade_out_us,
            ..Default::default()
        },
    };
    let fades = |op: Op| {
        let next = apply_ops(&p, &[op]).unwrap();
        (
            next.timeline.main[0].fade_in_us,
            next.timeline.main[0].fade_out_us,
        )
    };
    // The clip is now 5 s: the fade the patch didn't set shrinks to fit beside the one it did.
    assert_eq!(fades(speed(Some(secs(2.0)), None)), (secs(2.0), secs(3.0)));
    assert_eq!(fades(speed(None, Some(secs(2.0)))), (secs(3.0), secs(2.0)));
    assert_eq!(
        apply_ops(&p, &[speed(Some(secs(6.0)), None)]),
        Err(refused("Clip fades can't be longer than the clip."))
    );
}

#[test]
fn split_leaves_anchors_past_the_clip_end_alone() {
    // c1 and c2 are copies of recording time 20–30 s; "t" is anchored to clip a at 25 s, which a
    // doesn't cover, so it resolves to the copy nearest a (c1).
    let mut p = fixture();
    let copy = |id: &str| {
        let mut c = Clip::new(
            ClipSource::Recording { id: "rec".into() },
            secs(20.0),
            secs(30.0),
        );
        c.id = id.into();
        c
    };
    let (a, b) = (p.timeline.main[0].clone(), p.timeline.main[1].clone());
    p.timeline.main = vec![copy("c1"), a, b, copy("c2")];
    let mut t = text_item("t", 5.0, 1.0);
    t.attach = Some(Attach {
        clip_id: "a".into(),
        source: ClipSource::Recording { id: "rec".into() },
        source_us: secs(25.0),
    });
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.items.push(t);
    p.timeline.tracks.push(track);

    let next = apply_ops(&p, &[Op::Split { at_us: secs(15.0) }]).unwrap();
    assert_eq!(attach(&next, "t").clip_id, "a");
    assert_eq!(item(&next, "t").unwrap().start_us, secs(5.0), "still on c1");
}

/// Sound layers have no speed, so a clip whose sound plays from separated layers keeps its speed rather than drift from
/// them (sync must hold): one change, and one inside a run of speed changes (`speed_run`), are refused with a code. The
/// same speed, and another clip, still change.
#[test]
fn a_clip_whose_sound_was_separated_keeps_its_speed() {
    let detach = Op::DetachSource {
        clip_id: "a".into(),
        source_id: "mic".into(),
    };
    let p = apply_ops(&fixture(), &[detach]).unwrap();
    let speed = |id: &str, speed: f64| Op::UpdateClip {
        id: id.into(),
        patch: ClipPatch {
            speed: Some(speed),
            ..Default::default()
        },
    };
    // Named by where the clip plays ("The clip at 0:00 has its sound on its own track…").
    let drift = klipolo_model::ModelError::Invalid(
        klipolo_model::coded::CodedError::new("model.source.speedSeparated").with("atUs", 0i64),
    );
    assert_eq!(apply_ops(&p, &[speed("a", 2.0)]).unwrap_err(), drift);
    let run = [speed("b", 2.0), speed("b", 1.5), speed("a", 2.0)];
    assert_eq!(apply_ops(&p, &run).unwrap_err(), drift, "in a run too");
    let fine = [speed("a", 1.0), speed("b", 2.0), speed("b", 0.5)];
    assert!(apply_ops(&p, &fine).is_ok());
}
