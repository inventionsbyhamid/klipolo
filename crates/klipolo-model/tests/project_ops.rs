mod common;
use common::*;
use klipolo_model::layout::{resolve_rect, BoxN};
use klipolo_model::ops::*;
use klipolo_model::project::FormatKind::{Landscape, Portrait, Square};
use klipolo_model::project::*;
use klipolo_model::timeline::{Attach, ClipSource, Item, ItemContent, Track, TrackKind};
use klipolo_model::{apply_ops, ModelError};

#[test]
fn rename_and_settings() {
    let p = fixture();
    let settings = Settings {
        fps: 60,
        formats: vec![FormatKind::Landscape, FormatKind::Portrait],
        primary_format: FormatKind::Landscape,
    };
    let next = apply_ops(
        &p,
        &[
            Op::Rename {
                name: "Billing tutorial".into(),
            },
            Op::SetSettings {
                settings: settings.clone(),
            },
        ],
    )
    .unwrap();
    assert_eq!(next.name, "Billing tutorial");
    assert_eq!(next.settings, settings);
    assert_eq!(p.name, "Fixture", "input must be untouched");
}

#[test]
fn failed_batch_changes_nothing() {
    let p = fixture();
    let err = apply_ops(
        &p,
        &[
            Op::Rename { name: "ok".into() },
            Op::Rename { name: "".into() },
        ],
    )
    .unwrap_err();
    assert!(matches!(err, ModelError::Invalid(_)));
}

#[test]
fn track_lifecycle() {
    let p = fixture();
    let p = apply_ops(
        &p,
        &[
            Op::AddTrack {
                id: Some("t1".into()),
                kind: TrackKind::Overlay,
                index: None,
            },
            Op::AddTrack {
                id: Some("t2".into()),
                kind: TrackKind::Audio,
                index: Some(0),
            },
        ],
    )
    .unwrap();
    assert_eq!(
        p.timeline
            .tracks
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        vec!["t2", "t1"]
    );
    assert_eq!(p.timeline.tracks[1].name, "Overlay 1");
    let p = apply_ops(
        &p,
        &[
            Op::MoveTrack {
                id: "t2".into(),
                index: 5,
            },
            Op::UpdateTrack {
                id: "t1".into(),
                patch: TrackPatch {
                    name: Some("Titles".into()),
                    locked: Some(true),
                    ..Default::default()
                },
            },
        ],
    )
    .unwrap();
    assert_eq!(p.timeline.tracks[1].id, "t2");
    assert_eq!(p.timeline.tracks[0].name, "Titles");
    assert!(p.timeline.tracks[0].locked);
    let p = apply_ops(&p, &[Op::RemoveTrack { id: "t2".into() }]).unwrap();
    assert_eq!(p.timeline.tracks.len(), 1);
    assert_eq!(
        apply_ops(&p, &[Op::RemoveTrack { id: "t1".into() }]),
        Err(refused("That track is locked."))
    );
    let missing = apply_ops(&p, &[Op::RemoveTrack { id: "zzz".into() }]).unwrap_err();
    assert!(matches!(&missing, ModelError::NotFound { id, .. } if id == "zzz"));
    assert_eq!(missing.to_string(), "That track no longer exists.");
}

#[test]
fn assets_and_luts() {
    let p = fixture();
    assert!(
        apply_ops(&p, &[Op::RemoveAsset { id: "broll".into() }]).is_err(),
        "used by clip b"
    );
    let p = apply_ops(&p, &[Op::RemoveAsset { id: "logo".into() }]).unwrap();
    assert!(p.asset("logo").is_none());
    let lut = ColorLut {
        id: "lut".into(),
        name: "Teal".into(),
        path: "/luts/teal.cube".into(),
    };
    let mut p = apply_ops(&p, &[Op::AddLut { lut }]).unwrap();
    p.timeline.main[0].look.lut_id = Some("lut".into());
    let p = apply_ops(&p, &[Op::RemoveLut { id: "lut".into() }]).unwrap();
    assert!(p.luts.is_empty());
    assert_eq!(
        p.timeline.main[0].look.lut_id, None,
        "references are cleared"
    );
}

#[test]
fn op_json_shape() {
    let op = Op::AddTrack {
        id: None,
        kind: TrackKind::Overlay,
        index: None,
    };
    let json = serde_json::to_value(&op).unwrap();
    assert_eq!(json["type"], "addTrack");
    assert_eq!(
        json,
        serde_json::json!({"type": "addTrack", "id": null, "kind": "overlay", "index": null})
    );
    let patch = Op::UpdateTrack {
        id: "t".into(),
        patch: TrackPatch {
            locked: Some(true),
            ..Default::default()
        },
    };
    let back: Op =
        serde_json::from_str(r#"{"type":"updateTrack","id":"t","patch":{"locked":true}}"#).unwrap();
    assert_eq!(back, patch);
}

#[test]
fn unset_patch_fields_are_omitted_from_json() {
    // TrackPatch fields are `name?: string` etc. in TypeScript, so unset fields must not serialize as null.
    let op = Op::UpdateTrack {
        id: "t".into(),
        patch: TrackPatch {
            locked: Some(true),
            ..Default::default()
        },
    };
    assert_eq!(
        serde_json::to_value(&op).unwrap(),
        serde_json::json!({"type": "updateTrack", "id": "t", "patch": {"locked": true}})
    );
}

// --- Op-supplied track positions never panic ---

#[test]
fn out_of_range_track_positions_clamp() {
    let p = fixture();
    let p = apply_ops(
        &p,
        &[
            Op::AddTrack {
                id: Some("t1".into()),
                kind: TrackKind::Overlay,
                index: None,
            },
            Op::AddTrack {
                id: Some("t2".into()),
                kind: TrackKind::Zoom,
                index: Some(usize::MAX),
            },
        ],
    )
    .unwrap();
    assert_eq!(track_ids(&p), vec!["t1", "t2"]);
    assert_eq!(p.timeline.tracks[1].name, "Zoom");
    let p = apply_ops(
        &p,
        &[Op::MoveTrack {
            id: "t1".into(),
            index: usize::MAX,
        }],
    )
    .unwrap();
    assert_eq!(track_ids(&p), vec!["t2", "t1"]);
}

fn track_ids(p: &Project) -> Vec<&str> {
    p.timeline.tracks.iter().map(|t| t.id.as_str()).collect()
}

// --- Overlaps within OVERLAP_TOLERANCE_US count as touching ---

/// Fixture plus one overlay track: "t" attached to clip a at 1 s (2 s long, stored start stale),
/// then "u" (1 s long) starting `overlap_us` before t ends.
fn near_touching(overlap_us: i64) -> Project {
    let mut p = fixture();
    let mut attached = text_item("t", 0.0, 2.0);
    attached.attach = Some(Attach {
        clip_id: "a".into(),
        source: ClipSource::Recording { id: "rec".into() },
        source_us: secs(1.0),
    });
    let mut next_to = text_item("u", 3.0, 1.0);
    next_to.start_us = secs(3.0) - overlap_us;
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.items = vec![attached, next_to];
    p.timeline.tracks.push(track);
    p
}

#[test]
fn sub_frame_overlaps_stay_on_their_track() {
    let next = apply_ops(
        &near_touching(3),
        &[Op::Rename {
            name: "Renamed".into(),
        }],
    )
    .unwrap();
    assert_eq!(next.timeline.tracks.len(), 1);
    let items = &next.timeline.tracks[0].items;
    assert_eq!(
        items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        vec!["t", "u"]
    );
    assert_eq!(items[0].start_us, secs(1.0), "attached start is re-derived");
}

#[test]
fn real_overlaps_move_to_a_new_track_above() {
    let next = apply_ops(
        &near_touching(5),
        &[Op::Rename {
            name: "Renamed".into(),
        }],
    )
    .unwrap();
    assert_eq!(next.timeline.tracks.len(), 2);
    assert_eq!(next.timeline.tracks[0].items[0].id, "t");
    assert_eq!(next.timeline.tracks[1].name, "Overlay 2");
    assert_eq!(next.timeline.tracks[1].kind, TrackKind::Overlay);
    assert_eq!(next.timeline.tracks[1].items[0].id, "u");
}

// --- Changing the primary format rebases layouts ---

const PORTRAIT_TITLE: BoxN = BoxN {
    x: 0.1,
    y: 0.6,
    w: 0.8,
    h: 0.2,
};

/// Fixture in Landscape (primary) + Portrait, with an overlay track holding a text layer "t"
/// (hand-placed in Portrait) and a video layer "v" (auto-placed everywhere).
fn two_formats() -> Project {
    let mut p = fixture();
    p.settings.formats = vec![Landscape, Portrait];
    let mut title = text_item("t", 1.0, 2.0);
    title.layout.overrides.insert(Portrait, PORTRAIT_TITLE);
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.items = vec![title, video_item("v", 4.0, 2.0, 0.0)];
    p.timeline.tracks.push(track);
    p
}

fn set_formats(primary: FormatKind, formats: &[FormatKind]) -> Op {
    Op::SetSettings {
        settings: Settings {
            fps: 30,
            formats: formats.to_vec(),
            primary_format: primary,
        },
    }
}

fn layer<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap()
}

/// The new primary's own place becomes the rect; the old primary's rect stays as its place, though 16:9 is no longer
/// listed, so adding 16:9 back finds every layer where it was.
#[test]
fn new_primary_adopts_its_override_and_keeps_the_old_primarys_place() {
    let p = two_formats();
    let next = apply_ops(&p, &[set_formats(Portrait, &[Portrait])]).unwrap();
    let t = &layer(&next, "t").layout;
    assert_eq!(t.rect, PORTRAIT_TITLE);
    assert_eq!(
        t.overrides
            .iter()
            .map(|(f, r)| (*f, *r))
            .collect::<Vec<_>>(),
        vec![(Landscape, layer(&p, "t").layout.rect)]
    );
    let back = apply_ops(&next, &[set_formats(Landscape, &[Landscape, Portrait])]).unwrap();
    for id in ["t", "v"] {
        assert_eq!(
            layer(&back, id).layout.rect,
            layer(&p, id).layout.rect,
            "{id}"
        );
    }
}

#[test]
fn switching_primary_keeps_both_formats_looking_the_same() {
    let p = two_formats();
    let next = apply_ops(&p, &[set_formats(Portrait, &[Landscape, Portrait])]).unwrap();
    for id in ["t", "v"] {
        let (before, after) = (&layer(&p, id).layout, &layer(&next, id).layout);
        assert_eq!(
            resolve_rect(after, Portrait, Landscape),
            before.rect,
            "{id} in landscape"
        );
        assert_eq!(
            after.overrides.get(&Landscape),
            Some(&before.rect),
            "{id} keeps landscape exactly"
        );
        assert_eq!(
            resolve_rect(after, Portrait, Portrait),
            resolve_rect(before, Landscape, Portrait),
            "{id} in portrait"
        );
        assert!(
            !after.overrides.contains_key(&Portrait),
            "{id} has no override under its primary"
        );
    }
}

/// A format taken off the list keeps every layer's place for it in the file: listed again, the layers are where they
/// were placed.
#[test]
fn removing_a_secondary_format_keeps_its_overrides_for_when_it_comes_back() {
    let mut p = two_formats();
    p.settings.formats.push(Square);
    let square = BoxN {
        x: 0.2,
        y: 0.2,
        w: 0.6,
        h: 0.1,
    };
    p.timeline.tracks[0].items[0]
        .layout
        .overrides
        .insert(Square, square);
    let next = apply_ops(&p, &[set_formats(Landscape, &[Landscape, Square])]).unwrap();
    let (before, after) = (&layer(&p, "t").layout, &layer(&next, "t").layout);
    assert_eq!(after.rect, before.rect);
    assert_eq!(after.overrides, before.overrides);
    let back = apply_ops(
        &next,
        &[set_formats(Landscape, &[Landscape, Square, Portrait])],
    )
    .unwrap();
    assert_eq!(
        resolve_rect(&layer(&back, "t").layout, Landscape, Portrait),
        PORTRAIT_TITLE
    );
}

/// A template layer (a motion graphic that fills the frame) placed with `Layout::full_frame` covers the whole canvas
/// in 16:9, 9:16 and 1:1, starting in a 16:9-only project and through every format change after: formats added later,
/// the primary switched, a format dropped and added back. A layer whose rect merely covers the 16:9 canvas is derived
/// in 9:16 as the square at the top instead.
#[test]
fn a_full_frame_layer_fills_every_format_through_format_changes() {
    use klipolo_model::contrib::ContribId;
    use klipolo_model::layout::{Anchor, Layout};
    let template = |id: &str, start: f64| Item {
        id: id.into(),
        start_us: klipolo_model::time::secs_to_us(start),
        duration_us: 2_000_000,
        attach: None,
        layout: Layout::full_frame(),
        content: ItemContent::generator(
            ContribId::builtin("scene.lower-third"),
            Default::default(),
        ),
        effects: Vec::new(),
    };
    let mut p = fixture();
    p.settings.formats = vec![Landscape];
    let mut plain = template("plain", 4.0);
    plain.layout = Layout::new(Anchor::TopLeft, BoxN::FULL);
    let mut track = Track::new(TrackKind::Overlay, "Titles");
    track.items = vec![template("card", 1.0), plain];
    p.timeline.tracks.push(track);
    let fills = |p: &Project, id: &str| {
        [Landscape, Portrait, Square]
            .map(|f| resolve_rect(&layer(p, id).layout, p.settings.primary_format, f) == BoxN::FULL)
    };
    assert_eq!(fills(&p, "card"), [true; 3]);
    assert_eq!(
        fills(&p, "plain"),
        [true, false, true],
        "derived in 9:16, not full"
    );
    for (primary, formats) in [
        (Landscape, &[Landscape][..]),
        (Landscape, &[Landscape, Portrait, Square][..]),
        (Portrait, &[Landscape, Portrait, Square][..]),
        (Portrait, &[Portrait][..]),
        (Square, &[Square, Landscape][..]),
        (Landscape, &[Landscape, Portrait, Square][..]),
    ] {
        p = apply_ops(&p, &[set_formats(primary, formats)]).unwrap();
        assert_eq!(
            fills(&p, "card"),
            [true; 3],
            "after {primary:?} {formats:?}"
        );
    }
}

/// The camera bubble follows the new primary (captions keep a centre per format instead: `tests/caption_styles.rs`).
#[test]
fn the_camera_follows_the_new_primary() {
    let p = two_formats();
    let next = apply_ops(&p, &[set_formats(Portrait, &[Landscape, Portrait])]).unwrap();
    let (cam_before, cam_after) = (
        p.timeline.main[0].camera.as_ref().unwrap(),
        next.timeline.main[0].camera.as_ref().unwrap(),
    );
    let (before, after) = (&cam_before.bubble, &cam_after.bubble);
    assert_eq!(resolve_rect(after, Portrait, Landscape), before.rect);
    assert_eq!(
        resolve_rect(after, Portrait, Portrait),
        resolve_rect(before, Landscape, Portrait)
    );
    assert_eq!(
        cam_after.mode_overrides, cam_before.mode_overrides,
        "per-format camera modes are untouched"
    );
}

// --- Batch failures, ids and settings ---

#[test]
fn mid_batch_failure_leaves_input_unchanged() {
    let p = fixture();
    let before = p.clone();
    let err = apply_ops(
        &p,
        &[
            Op::Rename {
                name: "Changed".into(),
            },
            Op::RemoveTrack { id: "zzz".into() },
        ],
    )
    .unwrap_err();
    assert!(matches!(err, ModelError::NotFound { .. }), "{err:?}");
    assert_eq!(p, before);
}

#[test]
fn bumped_layers_skip_locked_tracks() {
    let mut p = near_touching(5);
    let mut locked = Track::new(TrackKind::Overlay, "Locked");
    locked.id = "locked".into();
    locked.locked = true;
    p.timeline.tracks.push(locked);
    let next = apply_ops(
        &p,
        &[Op::Rename {
            name: "Renamed".into(),
        }],
    )
    .unwrap();
    assert_eq!(
        track_ids(&next),
        vec![p.timeline.tracks[0].id.as_str(), "u-track", "locked"]
    );
    assert_eq!(next.timeline.tracks[1].items[0].id, "u");
    assert!(
        next.timeline.tracks[2].items.is_empty(),
        "nothing lands on a locked track"
    );
}

#[test]
fn bumped_tracks_never_take_a_waiting_layer_id() {
    // "y" and "y-track" both collide with "k"; the track made for "y" can't be called "y-track".
    let mut p = fixture();
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.id = "o".into();
    track.items = vec![
        text_item("k", 0.0, 10.0),
        text_item("y", 1.0, 1.0),
        text_item("y-track", 3.0, 1.0),
    ];
    p.timeline.tracks.push(track);
    let next = apply_ops(
        &p,
        &[Op::Rename {
            name: "Renamed".into(),
        }],
    )
    .unwrap();
    assert_eq!(track_ids(&next), vec!["o", "y-track-2"]);
    assert_eq!(next.timeline.tracks[1].items.len(), 2);
}

#[test]
fn bump_created_tracks_have_deterministic_ids() {
    let rename = [Op::Rename {
        name: "Renamed".into(),
    }];
    let p = near_touching(5);
    let once = apply_ops(&p, &rename).unwrap();
    assert_eq!(once.timeline.tracks[1].id, "u-track");
    assert_eq!(
        once,
        apply_ops(&p, &rename).unwrap(),
        "dry run and commit agree"
    );

    let mut taken = near_touching(5);
    let mut zoom = Track::new(TrackKind::Zoom, "Zoom");
    zoom.id = "u-track".into();
    taken.timeline.tracks.push(zoom);
    let next = apply_ops(&taken, &rename).unwrap();
    assert_eq!(next.timeline.tracks[1].id, "u-track-2");
}

#[test]
fn new_tracks_take_the_first_free_number() {
    let add = |id: &str, kind: TrackKind| Op::AddTrack {
        id: Some(id.into()),
        kind,
        index: None,
    };
    let p = apply_ops(
        &fixture(),
        &[
            add("o1", TrackKind::Overlay),
            add("o2", TrackKind::Overlay),
            add("o3", TrackKind::Overlay),
            add("z1", TrackKind::Zoom),
            add("z2", TrackKind::Zoom),
            Op::RemoveTrack { id: "o2".into() },
            Op::RemoveTrack { id: "z1".into() },
            add("o4", TrackKind::Overlay),
            add("z3", TrackKind::Zoom),
        ],
    )
    .unwrap();
    let names: Vec<&str> = p.timeline.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Overlay 1", "Overlay 3", "Zoom 2", "Overlay 2", "Zoom"]
    );
}

fn remove_asset_message(p: &Project, id: &str) -> String {
    match apply_ops(p, &[Op::RemoveAsset { id: id.into() }]) {
        Err(e @ ModelError::Invalid(_)) => e.to_string(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn remove_asset_explains_what_still_uses_it() {
    let mut on_timeline = fixture();
    let mut image = still_item("img", 1.0, 2.0);
    image.content = ItemContent::Image {
        asset_id: "logo".into(),
    };
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.items.push(image);
    on_timeline.timeline.tracks.push(track);
    assert_eq!(
        remove_asset_message(&on_timeline, "logo"),
        "This media is still used in the video. Remove it from the timeline first."
    );

    let mut background = fixture();
    background.background.kind = BackgroundKind::Image;
    background.background.asset_id = Some("logo".into());
    assert_eq!(
        remove_asset_message(&background, "logo"),
        "This media is the project background. Choose another background first."
    );

    assert_eq!(
        remove_asset_message(&fixture(), "screen"),
        "This media is part of a recording and can't be removed on its own."
    );
}

// --- Generated track ids avoid every id and repeat exactly ---

#[test]
fn bump_track_ids_avoid_every_project_id() {
    let mut p = near_touching(5);
    p.luts.push(ColorLut {
        id: "u-track".into(),
        name: "Teal".into(),
        path: "/luts/teal.cube".into(),
    });
    let next = apply_ops(
        &p,
        &[Op::Rename {
            name: "Renamed".into(),
        }],
    )
    .unwrap();
    assert_eq!(next.timeline.tracks[1].id, "u-track-2");
}

#[test]
fn unnamed_tracks_get_deterministic_ids() {
    let add = |kind| Op::AddTrack {
        id: None,
        kind,
        index: None,
    };
    let mut p = fixture();
    p.luts.push(ColorLut {
        id: "audio-track".into(),
        name: "Teal".into(),
        path: "/luts/teal.cube".into(),
    });
    let ops = [
        add(TrackKind::Overlay),
        add(TrackKind::Overlay),
        add(TrackKind::Zoom),
        add(TrackKind::Audio),
    ];
    let once = apply_ops(&p, &ops).unwrap();
    assert_eq!(
        once,
        apply_ops(&p, &ops).unwrap(),
        "dry run and commit agree"
    );
    assert_eq!(
        track_ids(&once),
        vec![
            "overlay-track",
            "overlay-track-2",
            "zoom-track",
            "audio-track-2"
        ]
    );
}

// --- Solo is a track setting ---

#[test]
fn solo_is_a_track_setting_and_one_undo_step() {
    use klipolo_model::history::Document;
    use klipolo_model::patch::{apply, diff};
    use klipolo_model::Transaction;

    let add = |id: &str, kind| Op::AddTrack {
        id: Some(id.into()),
        kind,
        index: None,
    };
    let solo = |id: &str, on| Op::UpdateTrack {
        id: id.into(),
        patch: TrackPatch {
            solo: Some(on),
            ..Default::default()
        },
    };
    // (Clip ids are taken: track ids must differ from them.)
    let p = apply_ops(
        &fixture(),
        &[
            add("ov", TrackKind::Overlay),
            add("au", TrackKind::Audio),
            add("zm", TrackKind::Zoom),
        ],
    )
    .unwrap();
    // Off is omitted from JSON; on is written.
    assert!(serde_json::to_value(&p.timeline.tracks[0]).unwrap()["solo"].is_null());
    let soloed = apply_ops(&p, &[solo("ov", true), solo("au", true)]).unwrap();
    assert!(soloed.timeline.tracks.iter().take(2).all(|t| t.solo));
    assert_eq!(
        serde_json::to_value(&soloed.timeline.tracks[0]).unwrap()["solo"],
        true
    );
    assert_eq!(
        serde_json::to_value(solo("ov", true)).unwrap()["patch"],
        serde_json::json!({"solo": true})
    );
    // The main track always plays and isn't a `Track`; a zoom track neither plays nor shows.
    assert!(apply_ops(&p, &[solo("zm", true)]).is_err());
    // The UI's patch carries it, and applying it gives exactly the soloed project.
    let patch = diff(&p, &soloed);
    assert!(patch.tracks.as_ref().is_some_and(|t| t[0].solo));
    assert_eq!(apply(&p, &patch).unwrap(), soloed);
    // One step to undo, back to exactly what was there.
    let mut doc = Document::new(p.clone()).unwrap();
    doc.apply(&Transaction {
        label: "Solo track".into(),
        ops: vec![solo("ov", true)],
        origin: None,
        ..Default::default()
    })
    .unwrap();
    assert!(doc.project().timeline.tracks[0].solo);
    doc.undo();
    assert_eq!(*doc.project(), p);
}
