mod common;
use common::*;
use klipolo_model::contrib::builtin;
use klipolo_model::layout::Layout;
use klipolo_model::motion::{sample, value_at, Blend, Ease, Key, Motion, MotionKeys, MotionUse};
use klipolo_model::ops::{Edge, ItemPatch, Op};
use klipolo_model::query::find_item;
use klipolo_model::{apply_ops, Project};

fn key(t: f64, v: f64) -> Key {
    Key {
        t_us: secs(t),
        v,
        ease: None,
    }
}

fn slide_in() -> MotionUse {
    MotionUse {
        preset: builtin::motion("slide"),
        params: [(
            "from".into(),
            klipolo_model::params::ParamValue::Text("top".into()),
        )]
        .into(),
        duration_us: secs(0.5),
        ease: None,
    }
}

/// A 4 s title at 2 s that moves right by 0.2 between 1 s and 3 s into it, slides in and fades out.
fn moving() -> Project {
    let mut p = fixture();
    let mut title = text_item("t", 2.0, 4.0);
    title.layout.motion = Some(Motion {
        enter: Some(slide_in()),
        exit: Some(MotionUse {
            preset: builtin::motion("fade"),
            params: Default::default(),
            duration_us: secs(0.3),
            ease: None,
        }),
        looping: None,
        keys: Some(MotionKeys {
            x: vec![key(1.0, 0.0), key(3.0, 0.2)],
            ..Default::default()
        }),
    });
    p.timeline.tracks = vec![overlay_track("T", vec![title])];
    apply_ops(&p, &[]).unwrap()
}

fn layout(p: &Project, id: &str) -> Layout {
    let (t, i) = find_item(p, id).unwrap();
    p.timeline.tracks[t].items[i].layout.clone()
}

#[test]
fn keys_ease_between_values_and_hold_outside() {
    let keys = vec![
        key(1.0, 0.0),
        Key {
            ease: Some(Ease::EaseInOut),
            ..key(3.0, 0.2)
        },
    ];
    assert_eq!(value_at(&keys, secs(0.0), 0.0), 0.0);
    assert!(
        (value_at(&keys, secs(2.0), 0.0) - 0.1).abs() < 1e-6,
        "ease-in-out is symmetric"
    );
    assert_eq!(value_at(&keys, secs(9.0), 0.0), 0.2);
    assert_eq!(
        value_at(&[], secs(1.0), 1.0),
        1.0,
        "no keys: the rest value"
    );
    let m = moving();
    let s = sample(
        layout(&m, "t").motion.as_ref().unwrap(),
        secs(2.0),
        secs(4.0),
    );
    assert!((s.dx - 0.1).abs() < 1e-9 && s.scale == 1.0 && s.opacity == 1.0);
}

#[test]
fn eases_start_at_zero_end_at_one_and_overshoot_only_where_named() {
    for ease in [
        Ease::Linear,
        Ease::Ease,
        Ease::EaseOut,
        Ease::Power2InOut,
        Ease::BackOut,
        Ease::ElasticOut,
        Ease::BounceOut,
        Ease::CubicBezier {
            x1: 0.4,
            y1: 0.0,
            x2: 0.2,
            y2: 1.0,
        },
    ] {
        assert!(
            ease.apply(0.0).abs() < 1e-9 && (ease.apply(1.0) - 1.0).abs() < 1e-9,
            "{ease:?}"
        );
    }
    assert!(Ease::BackOut.apply(0.8) > 1.0, "back overshoots");
    assert!((0..=100)
        .map(|i| Ease::Power3Out.apply(f64::from(i) / 100.0))
        .all(|v| (0.0..=1.0).contains(&v)));
}

#[test]
fn a_split_keeps_keys_on_each_side_with_one_at_the_cut() {
    let p = apply_ops(
        &moving(),
        &[Op::SplitItem {
            id: "t".into(),
            at_us: secs(4.0),
        }],
    )
    .unwrap();
    let first = layout(&p, "t").motion.unwrap();
    let second_id = p.timeline.tracks[0].items[1].id.clone();
    let second = layout(&p, &second_id).motion.unwrap();
    assert_eq!(
        first.keys.as_ref().unwrap().x,
        vec![key(1.0, 0.0), key(2.0, 0.1)],
        "the cut holds the value there"
    );
    assert_eq!(
        second.keys.as_ref().unwrap().x,
        vec![key(0.0, 0.1), key(1.0, 0.2)]
    );
    assert!(first.enter.is_some() && first.exit.is_none());
    assert!(second.enter.is_none() && second.exit.is_some());
}

#[test]
fn trims_shift_collapse_and_drop_keys_and_shorten_presets() {
    let p = apply_ops(
        &moving(),
        &[Op::TrimItem {
            id: "t".into(),
            edge: Edge::Start,
            at_us: secs(4.0),
        }],
    )
    .unwrap();
    assert_eq!(
        layout(&p, "t").motion.unwrap().keys.unwrap().x,
        vec![key(0.0, 0.1), key(1.0, 0.2)],
        "keys keep their timeline place"
    );
    let p = apply_ops(
        &moving(),
        &[Op::TrimItem {
            id: "t".into(),
            edge: Edge::End,
            at_us: secs(4.2),
        }],
    )
    .unwrap();
    // Keys past the new end go, and one at the end holds the slide where it was at 2.2 s, so it doesn't snap back.
    let trimmed = layout(&p, "t").motion.unwrap();
    let x = &trimmed.keys.as_ref().unwrap().x;
    assert_eq!(
        x.iter().map(|k| k.t_us).collect::<Vec<_>>(),
        [secs(1.0), secs(2.2)]
    );
    let was = sample(
        &layout(&moving(), "t").motion.unwrap(),
        secs(2.2),
        secs(4.0),
    )
    .dx;
    assert!((sample(&trimmed, secs(2.2), secs(2.2)).dx - was).abs() < 1e-9 && was > 0.1);
    let p = apply_ops(
        &moving(),
        &[Op::TrimItem {
            id: "t".into(),
            edge: Edge::End,
            at_us: secs(2.2),
        }],
    )
    .unwrap();
    let m = layout(&p, "t").motion.unwrap();
    assert_eq!(
        (m.enter.unwrap().duration_us, m.exit.unwrap().duration_us),
        (secs(0.2), secs(0.2)),
        "a preset lasts at most the layer's length"
    );
}

#[test]
fn validation_refuses_what_the_spec_rules_out() {
    let set = |m: Motion| {
        let mut p = moving();
        p.timeline.tracks[0].items[0].layout.motion = Some(m);
        klipolo_model::validate(&p).map_err(|e| e.coded().code)
    };
    let keys = |k: MotionKeys| Motion {
        keys: Some(k),
        ..Default::default()
    };
    assert_eq!(
        set(keys(MotionKeys {
            scale: vec![key(1.0, 25.0)],
            ..Default::default()
        })),
        Err("model.motion.badValue".into())
    );
    assert_eq!(
        set(keys(MotionKeys {
            opacity: vec![key(1.0, 1.5)],
            ..Default::default()
        })),
        Err("model.motion.badValue".into())
    );
    assert_eq!(
        set(keys(MotionKeys {
            x: vec![key(2.0, 0.0), key(1.0, 0.1)],
            ..Default::default()
        })),
        Err("model.motion.unsorted".into())
    );
    assert_eq!(
        set(keys(MotionKeys {
            x: vec![key(9.0, 0.0)],
            ..Default::default()
        })),
        Err("model.motion.outside".into())
    );
    assert_eq!(
        set(keys(MotionKeys {
            x: (0..65)
                .map(|i| Key {
                    t_us: i,
                    v: 0.0,
                    ease: None
                })
                .collect(),
            ..Default::default()
        })),
        Err("model.motion.tooManyKeys".into())
    );
    // Extreme Bézier eases are refused (they'd size a layer's targets past any frame); a real
    // overshoot is kept.
    let bezier = |y1: f64| Ease::CubicBezier {
        x1: 0.3,
        y1,
        x2: 0.7,
        y2: 1.0,
    };
    let eased = |y1: f64| {
        keys(MotionKeys {
            scale: vec![
                key(0.0, 1.0),
                Key {
                    ease: Some(bezier(y1)),
                    ..key(1.0, 2.0)
                },
            ],
            ..Default::default()
        })
    };
    assert_eq!(set(eased(2.5)), Ok(()));
    assert_eq!(set(eased(1e6)), Err("model.motion.badEase".into()));
    let popped = Motion {
        enter: Some(MotionUse {
            ease: Some(bezier(-1e300)),
            ..slide_in()
        }),
        ..Default::default()
    };
    assert_eq!(set(popped), Err("model.motion.badEase".into()));
    let unknown = Motion {
        enter: Some(MotionUse {
            preset: builtin::motion("swirl"),
            ..slide_in()
        }),
        ..Default::default()
    };
    assert_eq!(set(unknown), Ok(()), "an unknown preset is kept");
    // A built-in's settings are checked against its schema, and a built-in of another kind isn't an animation.
    let slide_from = |side: &str| Motion {
        enter: Some(MotionUse {
            params: [(
                "from".into(),
                klipolo_model::params::ParamValue::Text(side.into()),
            )]
            .into(),
            ..slide_in()
        }),
        ..Default::default()
    };
    assert_eq!(set(slide_from("left")), Ok(()));
    assert_eq!(set(slide_from("middle")), Err("model.param.invalid".into()));
    let look = Motion {
        enter: Some(MotionUse {
            preset: builtin::look("warm"),
            ..slide_in()
        }),
        ..Default::default()
    };
    assert_eq!(set(look), Err("model.motion.wrongKind".into()));
    let mut p = fixture();
    let mut music = text_item("m", 0.0, 4.0);
    music.content = klipolo_model::timeline::ItemContent::Audio {
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
    music.layout.blend = Some(Blend::Multiply);
    let mut track = overlay_track("A", vec![music]);
    track.kind = klipolo_model::timeline::TrackKind::Audio;
    p.timeline.tracks = vec![track];
    assert_eq!(
        klipolo_model::validate(&p).map_err(|e| e.coded().code),
        Err("model.motion.notVisual".into())
    );
}

#[test]
fn motion_and_blend_are_left_out_of_the_json_when_unset() {
    let still = serde_json::to_value(Layout::new(
        klipolo_model::layout::Anchor::Center,
        klipolo_model::layout::BoxN {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
    ))
    .unwrap();
    assert!(still.get("motion").is_none() && still.get("blend").is_none());
    let m = serde_json::to_value(layout(&moving(), "t").motion).unwrap();
    assert!(m.get("loop").is_none() && m["enter"]["preset"] == "klipolo.builtin/motion.slide");
}

#[test]
fn a_shorter_layer_keeps_valid_motion() {
    let p = apply_ops(
        &moving(),
        &[Op::UpdateItem {
            id: "t".into(),
            patch: ItemPatch {
                duration_us: Some(secs(1.5)),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(
        layout(&p, "t").motion.unwrap().keys.unwrap().x,
        vec![key(1.0, 0.0), key(1.5, 0.05)],
        "the value at the new end holds"
    );
}

#[test]
fn every_built_in_preset_has_a_well_formed_id() {
    let catalog = klipolo_model::contrib::Catalog::builtin();
    let words: Vec<_> = builtin::MOTION_PRESETS
        .iter()
        .chain(&builtin::MOTION_LOOPS)
        .collect();
    assert!(words.iter().all(|w| builtin::motion(w).is_well_formed()));
    // The catalogue defines exactly these 13 (the engine's registry is built from the same words).
    assert!(words.iter().all(|w| catalog
        .get(&builtin::motion(w))
        .is_some_and(|d| d.kind == klipolo_model::contrib::ContribKind::Motion)));
    assert_eq!(
        catalog
            .of_kind(klipolo_model::contrib::ContribKind::Motion)
            .count(),
        13
    );
}

fn times(m: &Motion) -> Vec<i64> {
    m.keys
        .as_ref()
        .map_or_else(Vec::new, |k| k.x.iter().map(|k| k.t_us).collect())
}

/// A cut through moving layers keeps their motion valid: keys stay on what is kept and go with what is cut, and a
/// video layer's pieces each keep their stretch, the enter on the first and the exit on the last.
#[test]
fn a_cut_through_moving_layers_keeps_keys_on_what_stays() {
    let still = moving();
    let mut video = video_item("v", 2.0, 4.0, 0.0);
    video.layout.motion = layout(&still, "t").motion;
    let mut title = text_item("t", 2.0, 4.0);
    title.layout.motion = video.layout.motion.clone();
    let mut p = fixture();
    p.timeline.tracks.clear();
    let add = |item| Op::AddItem {
        track: klipolo_model::ops::TrackTarget::Auto,
        item,
        attach: true,
    };
    let p = apply_ops(&p, &[add(title), add(video)]).unwrap();
    let cut = Op::DeleteRanges {
        ranges: vec![klipolo_model::RangeUs {
            start: secs(3.5),
            end: secs(4.5),
        }],
    };
    let p = apply_ops(&p, &[cut]).unwrap();
    let t = layout(&p, "t").motion.unwrap();
    assert_eq!(
        times(&t),
        vec![secs(1.0), secs(2.0)],
        "the key at 5 s moved with its moment to 4 s"
    );
    let first = layout(&p, "v").motion.unwrap();
    let second = layout(&p, "v-2").motion.unwrap();
    assert_eq!(
        (times(&first), first.enter.is_some(), first.exit.is_some()),
        (vec![secs(1.0), secs(1.5)], true, false)
    );
    assert_eq!(
        (
            times(&second),
            second.enter.is_some(),
            second.exit.is_some()
        ),
        (vec![0, secs(0.5)], false, true)
    );
}

/// The camera bubble's keys are in its recording's time; separated into a layer, they move into the layer's time.
#[test]
fn a_separated_camera_keeps_its_keys_on_the_content() {
    let mut p = fixture();
    let camera = p.timeline.main[0].camera.as_mut().unwrap();
    camera.bubble.motion = Some(Motion {
        keys: Some(MotionKeys {
            x: vec![key(5.0, 0.0), key(10.0, 0.1)],
            ..Default::default()
        }),
        ..Default::default()
    });
    let p = apply_ops(
        &p,
        &[Op::DetachSource {
            source_id: "camera".into(),
            clip_id: "a".into(),
        }],
    )
    .unwrap();
    // The camera starts 0.2 s into the recording, so the layer's first frame is the recording's 0.2 s.
    assert_eq!(
        times(&layout(&p, "a-camera").motion.unwrap()),
        vec![secs(4.8), secs(9.8)]
    );
    // A split at 10 s cuts the layer 9.8 s in: each piece keeps its side, with the value held at the cut.
    let split = apply_ops(&p, &[Op::Split { at_us: secs(10.0) }]).unwrap();
    assert_eq!(
        times(&layout(&split, "a-camera").motion.unwrap()),
        vec![secs(4.8), secs(9.8)]
    );
    assert_eq!(
        times(&layout(&split, "a-camera-2").motion.unwrap()),
        vec![0]
    );
    // At 2× the clip's camera layer plays the same footage in half the time, and its keys stay on it.
    let faster = apply_ops(
        &p,
        &[Op::UpdateClip {
            id: "a".into(),
            patch: klipolo_model::ops::ClipPatch {
                speed: Some(2.0),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(
        times(&layout(&faster, "a-camera").motion.unwrap()),
        vec![secs(2.4), secs(4.9)]
    );
}

/// A video layer at a new speed plays the same footage, so its keys rescale with it rather than being cut off.
#[test]
fn a_speed_change_rescales_keys_onto_the_same_footage() {
    let mut p = fixture();
    let mut video = video_item("v", 0.0, 8.0, 0.0);
    video.layout.motion = Some(Motion {
        keys: Some(MotionKeys {
            x: vec![key(4.0, 0.1), key(8.0, 0.2)],
            ..Default::default()
        }),
        ..Default::default()
    });
    p.timeline.tracks = vec![overlay_track("T", vec![video])];
    let mut content = match &p.timeline.tracks[0].items[0].content {
        c @ klipolo_model::timeline::ItemContent::Video { .. } => c.clone(),
        _ => unreachable!(),
    };
    if let klipolo_model::timeline::ItemContent::Video { speed, .. } = &mut content {
        *speed = 2.0;
    }
    let faster = apply_ops(
        &p,
        &[Op::UpdateItem {
            id: "v".into(),
            patch: ItemPatch {
                content: Some(content.clone()),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(
        times(&layout(&faster, "v").motion.unwrap()),
        vec![secs(2.0), secs(4.0)]
    );
    // A patch that brings the layer's layout along with the new speed (as the inspector sends it) rescales that
    // layout's keys the same way, rather than cutting them off at the new length.
    let with_layout = apply_ops(
        &p,
        &[Op::UpdateItem {
            id: "v".into(),
            patch: ItemPatch {
                content: Some(content),
                layout: Some(layout(&p, "v")),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(
        times(&layout(&with_layout, "v").motion.unwrap()),
        vec![secs(2.0), secs(4.0)]
    );
}

/// A cut that takes a key away keeps the values on each side of the join, so a rise and fall isn't flattened.
#[test]
fn a_cut_through_a_key_holds_the_values_at_the_join() {
    let mut m = Motion {
        keys: Some(MotionKeys {
            x: vec![key(1.0, 0.0), key(2.0, 1.0), key(3.0, 0.0)],
            ..Default::default()
        }),
        ..Default::default()
    };
    m.cut(&[(secs(1.5), secs(2.5))], secs(4.0), secs(3.0));
    let x = &m.keys.as_ref().unwrap().x;
    assert_eq!(
        x.iter().map(|k| k.t_us).collect::<Vec<_>>(),
        [secs(1.0), secs(1.5) - 1, secs(2.0)]
    );
    assert!(
        (value_at(x, secs(1.5) - 1, 0.0) - 0.5).abs() < 1e-9,
        "{x:?}"
    );
}

/// A value held at a new edge stays within what the property allows: an eased overshoot (backOut passes 1 on the way)
/// is clamped, so neither a trim nor a main-track cut through the fade is refused.
#[test]
fn a_held_value_stays_within_what_the_property_allows() {
    let mut title = text_item("t", 2.0, 4.0);
    title.layout.motion = Some(Motion {
        keys: Some(MotionKeys {
            opacity: vec![
                key(0.0, 0.0),
                Key {
                    ease: Some(Ease::BackOut),
                    ..key(1.0, 1.0)
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    });
    let mut p = fixture();
    p.timeline.tracks.clear();
    let p = apply_ops(
        &p,
        &[Op::AddItem {
            track: klipolo_model::ops::TrackTarget::Auto,
            item: title,
            attach: true,
        }],
    )
    .unwrap();
    let trimmed = apply_ops(
        &p,
        &[Op::TrimItem {
            id: "t".into(),
            edge: Edge::End,
            at_us: secs(2.5),
        }],
    )
    .unwrap();
    let opacity = layout(&trimmed, "t").motion.unwrap().keys.unwrap().opacity;
    assert_eq!(
        opacity.last().map(|k| (k.t_us, k.v)),
        Some((secs(0.5), 1.0))
    );
    let cut = Op::DeleteRanges {
        ranges: vec![klipolo_model::RangeUs {
            start: secs(2.5),
            end: secs(3.5),
        }],
    };
    let cut = apply_ops(&p, &[cut]).unwrap();
    let opacity = layout(&cut, "t").motion.unwrap().keys.unwrap().opacity;
    assert!(opacity.iter().all(|k| k.v <= 1.0), "{opacity:?}");
}

/// An effect target is sized for the largest scale the keys reach, overshoot between keys included: a spring between
/// scale keys 1 and 4 swings to about 6.57.
#[test]
fn max_key_scale_counts_a_springs_overshoot_between_keys() {
    let spring = Ease::Spring {
        stiffness: 100.0,
        damping: 1.0,
        mass: 1.0,
    };
    let keys = vec![
        key(0.0, 1.0),
        Key {
            t_us: secs(1.0),
            v: 4.0,
            ease: Some(spring),
        },
    ];
    let drawn = (0..=10_000)
        .map(|i| value_at(&keys, secs(f64::from(i) / 10_000.0), 1.0))
        .fold(0.0, f64::max);
    let m = Motion {
        keys: Some(MotionKeys {
            scale: keys,
            ..Default::default()
        }),
        ..Default::default()
    };
    let reach = klipolo_model::motion::max_key_scale(&m);
    assert!(drawn > 6.5, "the spring overshoots: {drawn}");
    assert!(
        reach >= drawn - 1e-9 && reach < drawn * 1.01,
        "{reach} for {drawn}"
    );
}
