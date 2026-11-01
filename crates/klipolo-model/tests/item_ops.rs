mod common;
use common::*;
use klipolo_model::contrib::builtin;
use klipolo_model::motion::Motion;
use klipolo_model::ops::*;
use klipolo_model::query::clip_starts;
use klipolo_model::time::MAX_TIME_US;
use klipolo_model::timeline::*;
use klipolo_model::{apply_ops, Project, RangeUs};

fn add(p: &Project, item: Item, attach: bool) -> Project {
    apply_ops(
        p,
        &[Op::AddItem {
            track: TrackTarget::Auto,
            item,
            attach,
        }],
    )
    .unwrap()
}

fn find<'a>(p: &'a Project, id: &str) -> (usize, &'a Item) {
    p.timeline
        .tracks
        .iter()
        .enumerate()
        .find_map(|(t, track)| track.items.iter().find(|i| i.id == id).map(|i| (t, i)))
        .unwrap()
}

#[test]
fn auto_placement_avoids_overlap() {
    let p = add(&fixture(), text_item("t1", 1.0, 3.0), false);
    let p = add(&p, text_item("t2", 2.0, 3.0), false);
    let p = add(&p, text_item("t3", 6.0, 1.0), false);
    assert_eq!(p.timeline.tracks.len(), 2);
    assert_eq!(find(&p, "t1").0, 0);
    assert_eq!(find(&p, "t2").0, 1);
    assert_eq!(find(&p, "t3").0, 0);
    let music = Item {
        content: ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: 0,
            volume: 0.5,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        ..still_item("m", 0.0, 30.0)
    };
    let p = add(&p, music, false);
    assert_eq!(p.timeline.tracks[find(&p, "m").0].kind, TrackKind::Audio);
}

#[test]
fn new_layers_go_above_the_layers_they_overlap() {
    // Title 0–3 s on Overlay 1, b-roll 1–4 s on Overlay 2: a title at 3.5–6 s overlaps the
    // b-roll, so it goes above it instead of under it on Overlay 1.
    let p = add(&fixture(), text_item("title", 0.0, 3.0), false);
    let p = add(&p, video_item("cutaway", 1.0, 3.0, 0.0), false);
    let p = add(&p, text_item("next", 3.5, 2.5), false);
    let tracks = ["title", "cutaway", "next"].map(|id| find(&p, id).0);
    assert_eq!(tracks, [0, 1, 2]);
}

#[test]
fn attach_flag_sets_anchor() {
    let p = add(&fixture(), text_item("t", 25.0, 1.0), true);
    let (_, item) = find(&p, "t");
    assert_eq!(item.attach.as_ref().unwrap().clip_id, "b");
    let p = apply_ops(
        &p,
        &[Op::SetAttach {
            id: "t".into(),
            attached: false,
        }],
    )
    .unwrap();
    assert!(find(&p, "t").1.attach.is_none());
}

#[test]
fn wrong_track_kind_is_rejected() {
    let p = apply_ops(
        &fixture(),
        &[Op::AddTrack {
            id: Some("aud".into()),
            kind: TrackKind::Audio,
            index: None,
        }],
    )
    .unwrap();
    let err = apply_ops(
        &p,
        &[Op::AddItem {
            track: TrackTarget::Track { id: "aud".into() },
            item: text_item("t", 0.0, 1.0),
            attach: false,
        }],
    );
    assert!(err.is_err());
}

#[test]
fn move_onto_occupied_spot_goes_to_free_track() {
    let p = add(&fixture(), text_item("t1", 1.0, 3.0), false);
    let p = add(&p, text_item("t2", 6.0, 1.0), false);
    let track = p.timeline.tracks[0].id.clone();
    let p = apply_ops(
        &p,
        &[Op::MoveItem {
            id: "t2".into(),
            track_id: Some(track),
            start_us: secs(2.0),
        }],
    )
    .unwrap();
    let (t, item) = find(&p, "t2");
    assert_eq!((t, item.start_us), (1, secs(2.0)));
}

#[test]
fn new_and_bumped_layers_skip_hidden_tracks() {
    // t1 1–4 s on Overlay 1, t2 2–3 s on Overlay 2, which the user then hides.
    let p = add(&fixture(), text_item("t1", 1.0, 3.0), false);
    let p = add(&p, text_item("t2", 2.0, 1.0), false);
    let hidden = p.timeline.tracks[1].id.clone();
    let patch = TrackPatch {
        hidden: Some(true),
        ..Default::default()
    };
    let p = apply_ops(&p, &[Op::UpdateTrack { id: hidden, patch }]).unwrap();
    // Overlay 2 is free at 3.5 s, but a layer added there must stay visible.
    let p = add(&p, text_item("new", 3.5, 0.5), false);
    assert_eq!(find(&p, "new").0, 2);
    // Moving it onto t1 bumps it up, past the hidden track again.
    let track = p.timeline.tracks[0].id.clone();
    let p = apply_ops(
        &p,
        &[Op::MoveItem {
            id: "new".into(),
            track_id: Some(track),
            start_us: secs(1.0),
        }],
    )
    .unwrap();
    let (t, _) = find(&p, "new");
    assert!(t != 1 && !p.timeline.tracks[t].hidden);
}

#[test]
fn trim_and_split_video_items() {
    let p = add(&fixture(), video_item("v", 2.0, 6.0, 1.0), false);
    let p = apply_ops(
        &p,
        &[Op::TrimItem {
            id: "v".into(),
            edge: Edge::Start,
            at_us: secs(3.0),
        }],
    )
    .unwrap();
    let (_, v) = find(&p, "v");
    assert_eq!((v.start_us, v.duration_us), (secs(3.0), secs(5.0)));
    assert!(
        matches!(v.content, ItemContent::Video { source_in_us, .. } if source_in_us == secs(2.0))
    );
    let p = apply_ops(
        &p,
        &[Op::SplitItem {
            id: "v".into(),
            at_us: secs(5.0),
        }],
    )
    .unwrap();
    let items = &p.timeline.tracks[0].items;
    assert_eq!(items.len(), 2);
    assert_eq!(items[1].start_us, secs(5.0));
    assert!(
        matches!(items[1].content, ItemContent::Video { source_in_us, .. } if source_in_us == secs(4.0))
    );
    assert!(apply_ops(
        &p,
        &[Op::TrimItem {
            id: "v".into(),
            edge: Edge::End,
            at_us: secs(3.0)
        }]
    )
    .is_err());
}

#[test]
fn locked_tracks_block_direct_edits() {
    let p = add(&fixture(), text_item("t", 1.0, 1.0), false);
    let track = p.timeline.tracks[0].id.clone();
    let p = apply_ops(
        &p,
        &[Op::UpdateTrack {
            id: track.clone(),
            patch: TrackPatch {
                locked: Some(true),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert!(apply_ops(&p, &[Op::RemoveItem { id: "t".into() }]).is_err());
    assert!(apply_ops(
        &p,
        &[Op::MoveItem {
            id: "t".into(),
            track_id: None,
            start_us: 0
        }]
    )
    .is_err());
    // New layers skip the locked track, and nothing can be moved onto it.
    let p = add(&p, text_item("u", 5.0, 1.0), false);
    assert_eq!(find(&p, "u").0, 1);
    assert_eq!(
        apply_ops(
            &p,
            &[Op::MoveItem {
                id: "u".into(),
                track_id: Some(track),
                start_us: secs(5.0)
            }]
        ),
        Err(refused("That track is locked."))
    );
}

#[test]
fn update_item_patches_layout_and_text() {
    let p = add(&fixture(), text_item("t", 1.0, 1.0), false);
    let mut layout = find(&p, "t").1.layout.clone();
    layout.rect.x = 0.5;
    layout.motion = Some(Motion {
        enter: Some(preset("pop")),
        ..fade_in_out()
    });
    let content = ItemContent::Text {
        text: "Set up billing".into(),
        style: TextStyle::default(),
    };
    let p = apply_ops(
        &p,
        &[Op::UpdateItem {
            id: "t".into(),
            patch: ItemPatch {
                layout: Some(layout),
                content: Some(content),
                duration_us: Some(secs(3.0)),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    let (_, t) = find(&p, "t");
    assert_eq!(t.layout.rect.x, 0.5);
    assert_eq!(t.duration_us, secs(3.0));
    assert!(matches!(&t.content, ItemContent::Text { text, .. } if text == "Set up billing"));
    let music = ItemContent::Audio {
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
    assert!(apply_ops(
        &p,
        &[Op::UpdateItem {
            id: "t".into(),
            patch: ItemPatch {
                content: Some(music),
                ..Default::default()
            }
        }]
    )
    .is_err());
}

#[test]
fn changing_video_layer_speed_keeps_its_footage() {
    // The whole 10 s b-roll at 2× plays in 5 s.
    let p = add(&fixture(), video_item("v", 2.0, 10.0, 0.0), false);
    let mut content = find(&p, "v").1.content.clone();
    if let ItemContent::Video { speed, .. } = &mut content {
        *speed = 2.0;
    }
    let patch = ItemPatch {
        content: Some(content),
        ..Default::default()
    };
    let p = apply_ops(
        &p,
        &[Op::UpdateItem {
            id: "v".into(),
            patch,
        }],
    )
    .unwrap();
    assert_eq!(find(&p, "v").1.duration_us, secs(5.0));
}

#[test]
fn detach_camera_creates_attached_video_layer() {
    let p = apply_ops(
        &fixture(),
        &[Op::DetachSource {
            source_id: "camera".into(),
            clip_id: "a".into(),
        }],
    )
    .unwrap();
    let cam = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| matches!(&i.content, ItemContent::Video { asset_id, .. } if asset_id == "camera"))
        .unwrap();
    // camera offset 0.2 s: camera source 0 aligns with screen 0.2 s, so the layer starts 0.2 s in.
    assert_eq!(cam.start_us, secs(0.2));
    assert_eq!(cam.duration_us, secs(19.8));
    assert!(cam.attach.is_some());
    // Looks like the circular bubble did; unmuting plays the camera's own audio.
    assert_eq!(cam.layout.radius, 0.5);
    assert!(matches!(cam.content, ItemContent::Video { volume, muted: true, .. } if volume == 1.0));
    let a = &p.timeline.main[0];
    for format in [
        klipolo_model::project::FormatKind::Landscape,
        klipolo_model::project::FormatKind::Portrait,
    ] {
        assert_eq!(
            a.camera.as_ref().unwrap().mode_for(format),
            CameraMode::Hidden
        );
    }
    assert!(apply_ops(
        &p,
        &[Op::DetachSource {
            source_id: "camera".into(),
            clip_id: "b".into()
        }]
    )
    .is_err());
}

#[test]
fn ripple_collision_moves_later_item_up() {
    let p = add(&fixture(), text_item("free", 10.0, 5.0), false);
    let track = p.timeline.tracks[0].id.clone();
    let p = apply_ops(
        &p,
        &[Op::AddItem {
            track: TrackTarget::Track { id: track },
            item: text_item("att", 20.0, 5.0),
            attach: true,
        }],
    )
    .unwrap();
    let p = apply_ops(
        &p,
        &[Op::DeleteRanges {
            ranges: vec![RangeUs {
                start: 0,
                end: secs(8.0),
            }],
        }],
    )
    .unwrap();
    assert_eq!(find(&p, "free").0, 0);
    let (t, att) = find(&p, "att");
    assert_eq!((t, att.start_us), (1, secs(12.0)));
}

// --- A split layer keeps its clip, derived ids repeat, layer times are bounded ---

#[test]
fn split_title_at_speed_stays_on_its_clip() {
    // A title at the start of clip b, with both clips at 1.5×.
    let mut p = fixture();
    p.timeline.main[0].speed = 1.5;
    p.timeline.main[1].speed = 1.5;
    let b_start = clip_starts(&p.timeline.main)[1];
    let title = Item {
        start_us: b_start,
        ..text_item("t", 0.0, 2.0)
    };
    let p = add(&p, title, true);
    let p = apply_ops(
        &p,
        &[Op::SplitItem {
            id: "t".into(),
            at_us: b_start + secs(1.0),
        }],
    )
    .unwrap();
    for id in ["t", "t-2"] {
        assert_eq!(find(&p, id).1.attach.as_ref().unwrap().clip_id, "b", "{id}");
    }
    // Presets stay at the outer ends: the enter on the first part, the exit on the second.
    let anims = |id: &str| {
        let m = find(&p, id).1.layout.motion.clone().unwrap_or_default();
        (m.enter.map(|u| u.preset), m.exit.map(|u| u.preset))
    };
    let fade = Some(builtin::motion("fade"));
    assert_eq!(anims("t"), (fade.clone(), None));
    assert_eq!(anims("t-2"), (None, fade));
    let p = apply_ops(
        &p,
        &[Op::MoveClip {
            id: "b".into(),
            index: 0,
        }],
    )
    .unwrap();
    assert_eq!(
        (find(&p, "t").1.start_us, find(&p, "t-2").1.start_us),
        (0, secs(1.0))
    );
}

#[test]
fn same_layer_ops_give_identical_projects() {
    // New tracks, split parts and camera layers get ids derived from what they came from.
    let ops = [
        Op::AddItem {
            track: TrackTarget::Auto,
            item: text_item("t", 1.0, 4.0),
            attach: true,
        },
        Op::SplitItem {
            id: "t".into(),
            at_us: secs(2.0),
        },
        Op::DetachSource {
            source_id: "camera".into(),
            clip_id: "a".into(),
        },
    ];
    let once = apply_ops(&fixture(), &ops).unwrap();
    assert_eq!(
        once,
        apply_ops(&fixture(), &ops).unwrap(),
        "dry run and commit agree"
    );
    let tracks: Vec<&str> = once.timeline.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(tracks, vec!["t-track", "a-camera-track"]);
    assert_eq!(find(&once, "t-2").1.start_us, secs(2.0));
    assert!(
        matches!(&find(&once, "a-camera").1.content, ItemContent::Video { asset_id, .. } if asset_id == "camera")
    );
}

#[test]
fn out_of_range_layer_times_are_refused() {
    // One guard over every op-supplied layer time.
    let p = add(&fixture(), video_item("v", 2.0, 6.0, 1.0), false);
    let early = Item {
        start_us: -1,
        ..text_item("x", 0.0, 1.0)
    };
    let endless = Item {
        duration_us: i64::MAX,
        ..text_item("x", 0.0, 1.0)
    };
    let ops = [
        Op::AddItem {
            track: TrackTarget::Auto,
            item: early,
            attach: true,
        },
        Op::MoveItem {
            id: "v".into(),
            track_id: None,
            start_us: i64::MAX,
        },
        Op::TrimItem {
            id: "v".into(),
            edge: Edge::Start,
            at_us: i64::MIN,
        },
        Op::TrimItem {
            id: "v".into(),
            edge: Edge::End,
            at_us: MAX_TIME_US + 1,
        },
        Op::SplitItem {
            id: "v".into(),
            at_us: i64::MAX,
        },
    ];
    for op in ops {
        assert_eq!(
            apply_ops(&p, std::slice::from_ref(&op)),
            Err(refused("That time is outside the video.")),
            "{op:?}"
        );
    }
    let lengths = [
        Op::AddItem {
            track: TrackTarget::Auto,
            item: endless,
            attach: false,
        },
        Op::UpdateItem {
            id: "v".into(),
            patch: ItemPatch {
                duration_us: Some(-1),
                ..Default::default()
            },
        },
    ];
    for op in lengths {
        assert_eq!(
            apply_ops(&p, std::slice::from_ref(&op)),
            Err(refused("That length isn't valid.")),
            "{op:?}"
        );
    }
}

// --- Regression cases ---

#[test]
fn split_music_keeps_its_fades_at_the_outer_ends() {
    let music = Item {
        content: ItemContent::Audio {
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
        },
        ..still_item("m", 0.0, 10.0)
    };
    let p = add(&fixture(), music, false);
    let p = apply_ops(
        &p,
        &[Op::SplitItem {
            id: "m".into(),
            at_us: secs(4.0),
        }],
    )
    .unwrap();
    let audio = |id: &str| match find(&p, id).1.content {
        ItemContent::Audio {
            source_in_us,
            fade_in_us,
            fade_out_us,
            ..
        } => (source_in_us, fade_in_us, fade_out_us),
        _ => panic!("{id} is not audio"),
    };
    assert_eq!(audio("m"), (0, secs(1.0), 0));
    assert_eq!(audio("m-2"), (secs(4.0), 0, secs(1.0)));
}

#[test]
fn separated_camera_stops_where_its_footage_ends() {
    let mut p = fixture();
    p.assets
        .iter_mut()
        .find(|a| a.id == "camera")
        .unwrap()
        .duration_us = secs(15.0);
    // A run never outlasts its file, so the camera's run ends with its 15 s of footage too.
    p.recordings[0].sources[1].runs[0].duration_us = secs(15.0);
    let p = apply_ops(
        &p,
        &[Op::DetachSource {
            source_id: "camera".into(),
            clip_id: "a".into(),
        }],
    )
    .unwrap();
    let cam = find(&p, "a-camera").1;
    assert_eq!((cam.start_us, cam.duration_us), (secs(0.2), secs(15.0)));
}

#[test]
fn a_camera_is_separated_while_no_layer_already_shows_it() {
    let detach = [Op::DetachSource {
        source_id: "camera".into(),
        clip_id: "a".into(),
    }];
    let already = Err(refused("This clip's camera is already in its own layer."));
    // The same mode in every format.
    let camera = |mode| Op::UpdateClip {
        id: "a".into(),
        patch: ClipPatch {
            camera: Some(CameraLayout {
                mode,
                mode_overrides: Default::default(),
                ..Default::default()
            }),
            ..Default::default()
        },
    };
    let p = apply_ops(&fixture(), &detach).unwrap();
    assert_eq!(apply_ops(&p, &detach), already);
    // Showing the built-in camera again doesn't free it while the camera layer is still there.
    let shown = apply_ops(&p, &[camera(CameraMode::Bubble)]).unwrap();
    assert_eq!(apply_ops(&shown, &detach), already);
    // Once that layer is deleted, the camera can be separated again.
    let removed = apply_ops(
        &p,
        &[Op::RemoveItem {
            id: "a-camera".into(),
        }],
    )
    .unwrap();
    assert!(apply_ops(&removed, &detach).is_ok());
    // A camera the user turned off everywhere can still be separated.
    let off = apply_ops(&fixture(), &[camera(CameraMode::Hidden)]).unwrap();
    assert!(apply_ops(&off, &detach).is_ok());
}

#[test]
fn separated_camera_follows_clip_speed() {
    let p = apply_ops(
        &fixture(),
        &[
            Op::DetachSource {
                source_id: "camera".into(),
                clip_id: "a".into(),
            },
            Op::UpdateClip {
                id: "a".into(),
                patch: ClipPatch {
                    speed: Some(2.0),
                    ..Default::default()
                },
            },
        ],
    )
    .unwrap();
    // Same camera footage (0–19.8 s), played at 2× from 0.1 s: still in sync with the screen.
    let cam = find(&p, "a-camera").1;
    assert_eq!((cam.start_us, cam.duration_us), (secs(0.1), secs(9.9)));
    assert!(matches!(cam.content, ItemContent::Video { speed, .. } if speed == 2.0));
}

#[test]
fn separated_camera_is_split_with_its_clip() {
    // (start, duration, camera source in, speed) of a camera layer.
    let cam = |p: &Project, id: &str| match find(p, id).1 {
        Item {
            start_us,
            duration_us,
            content:
                ItemContent::Video {
                    source_in_us,
                    speed,
                    ..
                },
            ..
        } => (*start_us, *duration_us, *source_in_us, *speed),
        other => panic!("{other:?}"),
    };
    let speed_up = Op::UpdateClip {
        id: "a-2".into(),
        patch: ClipPatch {
            speed: Some(2.0),
            ..Default::default()
        },
    };
    let detach = Op::DetachSource {
        source_id: "camera".into(),
        clip_id: "a".into(),
    };
    let split = Op::Split { at_us: secs(10.0) };
    let p = apply_ops(&fixture(), &[detach, split, speed_up]).unwrap();
    // Camera time = screen time − 0.2 s. a: 0–10 s; a-2 (screen 10–20 s at 2×): 10–15 s.
    assert_eq!(cam(&p, "a-camera"), (secs(0.2), secs(9.8), 0, 1.0));
    assert_eq!(
        cam(&p, "a-camera-2"),
        (secs(10.0), secs(5.0), secs(9.8), 2.0)
    );
    // Moving a-2 after b (15–25 s → a-2 at 20 s) takes its camera along.
    let moved = Op::MoveClip {
        id: "a-2".into(),
        index: 2,
    };
    let p = apply_ops(&p, &[moved]).unwrap();
    assert_eq!(cam(&p, "a-camera").0, secs(0.2));
    assert_eq!(
        cam(&p, "a-camera-2"),
        (secs(20.0), secs(5.0), secs(9.8), 2.0)
    );
}

#[test]
fn split_outside_a_layer_says_so() {
    let p = add(&fixture(), text_item("t", 2.0, 2.0), false);
    let split = |at| Op::SplitItem {
        id: "t".into(),
        at_us: at,
    };
    assert_eq!(
        apply_ops(&p, &[split(secs(5.0))]),
        Err(refused("The playhead isn't over this layer."))
    );
    assert_eq!(
        apply_ops(&p, &[split(secs(2.0) + 5_000)]),
        Err(refused(
            "That's too close to the edge of the layer to split."
        ))
    );
}
