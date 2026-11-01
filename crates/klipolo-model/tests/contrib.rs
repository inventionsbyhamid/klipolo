//! Plugin seams: contribution ids, effect stacks, params, the file format (4), newer projects.
mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::*;
use klipolo_model::contrib::{
    builtin, unsupported_requirements, AssetPath, Catalog, ContribDef, ContribKind,
};
use klipolo_model::cursor_style::*;
use klipolo_model::ops::*;
use klipolo_model::params::{ParamKind, ParamSpec, ParamValue, PointN, VisibleWhen};
use klipolo_model::project::{
    FormatKind, GameInfo, Marker, MarkerKind, RecordingKind, SourceMix, SourceRole,
};
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::{
    EffectInstance, Framing, FramingMode, Item, ItemContent, Transition, ZoomTarget,
};
use klipolo_model::{apply_ops, load_project, validate, ModelError};

fn blur() -> EffectInstance {
    EffectInstance::new(builtin::EFFECT_BLUR).with("radius", ParamValue::Number(20.0))
}

fn on_clip(id: &str) -> EffectTarget {
    EffectTarget::Clip { id: id.into() }
}

/// A still layer (no title fade) showing `content`.
fn layer(id: &str, content: ItemContent) -> Item {
    Item {
        content,
        ..still_item(id, 0.0, 2.0)
    }
}

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

#[test]
fn files_from_older_test_builds_are_refused_plainly() {
    let mut p = serde_json::to_value(fixture()).unwrap();
    for old in [1, 2] {
        p["version"] = old.into();
        let error = load_project(&p.to_string()).unwrap_err();
        assert_eq!(error, ModelError::OlderVersion);
        assert_eq!(
            error.to_string(),
            "This project was made with an older test version of Klipolo and can't be opened."
        );
    }
    assert_eq!(
        load_project(&serde_json::to_string(&fixture()).unwrap()).unwrap(),
        fixture()
    );
}

#[test]
fn effect_ops_edit_a_stack() {
    let p = apply_ops(
        &fixture(),
        &[
            Op::AddEffect {
                target: on_clip("a"),
                effect: blur(),
                index: None,
            },
            Op::AddEffect {
                target: on_clip("a"),
                effect: EffectInstance::new(builtin::EFFECT_VIGNETTE),
                index: Some(0),
            },
        ],
    )
    .unwrap();
    let ids: Vec<_> = p.timeline.main[0]
        .effects
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["fx-2", "fx"],
        "empty ids are derived within the stack; index 0 goes first"
    );

    let radius = |r: f64| {
        Some(BTreeMap::from([(
            "radius".to_string(),
            ParamValue::Number(r),
        )]))
    };
    let p = apply_ops(
        &p,
        &[
            Op::UpdateEffect {
                target: on_clip("a"),
                id: "fx".into(),
                patch: EffectPatch {
                    params: radius(30.0),
                    ..Default::default()
                },
            },
            Op::MoveEffect {
                target: on_clip("a"),
                id: "fx".into(),
                index: 0,
            },
            Op::UpdateEffect {
                target: on_clip("a"),
                id: "fx-2".into(),
                patch: EffectPatch {
                    enabled: Some(false),
                    ..Default::default()
                },
            },
        ],
    )
    .unwrap();
    let stack = &p.timeline.main[0].effects;
    assert_eq!(
        (stack[0].id.as_str(), &stack[0].params["radius"]),
        ("fx", &ParamValue::Number(30.0))
    );
    assert!(!stack[1].enabled);

    let p = apply_ops(
        &p,
        &[
            Op::UpdateEffect {
                target: on_clip("a"),
                id: "fx".into(),
                patch: EffectPatch {
                    reset: Some(vec!["radius".into()]),
                    ..Default::default()
                },
            },
            Op::RemoveEffect {
                target: on_clip("a"),
                id: "fx-2".into(),
            },
        ],
    )
    .unwrap();
    assert_eq!(p.timeline.main[0].effects.len(), 1);
    assert!(
        p.timeline.main[0].effects[0].params.is_empty(),
        "reset keys fall back to the defaults"
    );
    assert!(matches!(
        apply_ops(
            &p,
            &[Op::RemoveEffect {
                target: on_clip("a"),
                id: "nope".into()
            }]
        ),
        Err(ModelError::NotFound { kind: "effect", .. })
    ));
}

#[test]
fn validation_checks_installed_schemas_and_keeps_unknown_effects() {
    let add = |effect: EffectInstance| {
        apply_ops(
            &fixture(),
            &[Op::AddEffect {
                target: on_clip("a"),
                effect,
                index: None,
            }],
        )
    };
    let invalid = |m: &str| m.to_string();
    assert_eq!(
        add(EffectInstance::new(builtin::EFFECT_BLUR).with("radius", ParamValue::Number(100.0)))
            .unwrap_err()
            .to_string(),
        invalid("Radius must be between 0 and 64.")
    );
    assert_eq!(
        add(EffectInstance::new(builtin::look("warm")))
            .unwrap_err()
            .to_string(),
        invalid("Warm isn't an effect.")
    );
    assert_eq!(
        add(EffectInstance::new("not an id"))
            .unwrap_err()
            .to_string(),
        invalid("An effect has an invalid id (not an id).")
    );

    // A plugin that isn't installed: kept as it is, including settings no installed schema describes.
    let glow = EffectInstance::new("com.acme.neon/glow")
        .with("color", ParamValue::Text("#00E5FF".into()))
        .with("future", ParamValue::Point(PointN { x: 0.5, y: 0.5 }));
    let p = add(glow.clone()).unwrap();
    let reopened = load_project(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(reopened.timeline.main[0].effects[0].params, glow.params);

    let nine: Vec<Op> = (0..9)
        .map(|_| Op::AddEffect {
            target: on_clip("a"),
            effect: EffectInstance::new(builtin::EFFECT_VIGNETTE),
            index: None,
        })
        .collect();
    assert_eq!(
        apply_ops(&fixture(), &nine).unwrap_err().to_string(),
        invalid("A clip or layer can have up to eight effects.")
    );
}

#[test]
fn split_pieces_keep_their_stacks() {
    let mut p = fixture();
    p.timeline.main[0].effects = vec![EffectInstance {
        id: "fx".into(),
        ..blur()
    }];
    let p = apply_ops(&p, &[Op::Split { at_us: secs(5.0) }]).unwrap();
    assert_eq!(p.timeline.main[0].effects, p.timeline.main[1].effects);
    assert_eq!(p.timeline.main[1].effects[0].id, "fx");
}

#[test]
fn newer_projects_open_read_only_or_explain_what_they_need() {
    let mut p = fixture();
    p.requires.insert("templates@1".into());
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(
        unsupported_requirements(&load_project(&json).unwrap()),
        ["templates@1"]
    );
    let unreadable = json.replacen(r#""type":"recording""#, r#""type":"template""#, 1);
    assert_eq!(
        load_project(&unreadable).unwrap_err(),
        ModelError::NewerFeatures("templates@1".into())
    );
    // A value beyond this build's range (a newer Klipolo may allow 8× speed) asks for an update too, not a fix.
    p.timeline.main[0].speed = 8.0;
    let too_fast = serde_json::to_string(&p).unwrap();
    assert_eq!(
        load_project(&too_fast).unwrap_err(),
        ModelError::NewerFeatures("templates@1".into())
    );
    p.requires.clear();
    assert!(matches!(
        load_project(&serde_json::to_string(&p).unwrap()),
        Err(ModelError::Invalid(_))
    ));
}

#[test]
fn removals_from_chat_polish_and_plugins_need_review() {
    let cut = Op::DeleteRanges {
        ranges: vec![RangeUs {
            start: 0,
            end: secs(1.0),
        }],
    };
    assert_eq!(op_risk(&cut), OpRisk::Removal);
    assert_eq!(
        op_risk(&Op::AddEffect {
            target: on_clip("a"),
            effect: blur(),
            index: None
        }),
        OpRisk::Additive
    );
    let tx = |origin| Transaction {
        label: "Cut".into(),
        ops: vec![cut.clone()],
        origin,
        ..Default::default()
    };
    let p = fixture();
    assert!(!tx(None).needs_review(Some(&p)) && !tx(Some(Origin::User)).needs_review(Some(&p)));
    assert!(tx(Some(Origin::Chat)).needs_review(Some(&p)));
    assert!(tx(Some(Origin::Plugin {
        id: "com.acme.neon".into(),
        version: "1.2.0".into()
    }))
    .needs_review(None));

    // A trim disguised as a new length: judged against the project, shorter takes content away, longer doesn't.
    let p = apply_ops(
        &p,
        &[Op::AddItem {
            track: TrackTarget::Auto,
            item: text_item("t", 0.0, 2.0),
            attach: false,
        }],
    )
    .unwrap();
    let resize = |seconds: f64| Transaction {
        label: "Resize".into(),
        ops: vec![Op::UpdateItem {
            id: "t".into(),
            patch: ItemPatch {
                duration_us: Some(secs(seconds)),
                ..Default::default()
            },
        }],
        origin: Some(Origin::Chat),
        ..Default::default()
    };
    assert_eq!(resize(1.0).risk_against(&p), OpRisk::Removal);
    assert_eq!(resize(3.0).risk_against(&p), OpRisk::Additive);
    assert!(resize(1.0).needs_review(Some(&p)) && !resize(3.0).needs_review(Some(&p)));
    assert_eq!(
        resize(3.0).risk(),
        OpRisk::Removal,
        "without the project, conservatively"
    );
    let split = Transaction {
        ops: vec![Op::Split { at_us: secs(5.0) }],
        ..resize(1.0)
    };
    assert_eq!(
        split.risk_against(&p),
        OpRisk::Additive,
        "split pieces keep everything"
    );
}

#[test]
fn pack_paths_are_portable_and_stay_inside_their_pack() {
    assert_eq!(
        AssetPath::parse("pack://com.acme.neon@1.2.0/luts/teal.cube"),
        Some(AssetPath::Pack {
            plugin: "com.acme.neon",
            version: "1.2.0",
            path: "luts/teal.cube"
        })
    );
    assert_eq!(
        AssetPath::parse("/Users/me/a.mp4"),
        Some(AssetPath::File("/Users/me/a.mp4"))
    );
    for path in [
        "pack://com.acme.neon@1.2.0/luts\\..\\..\\secret.cube",
        "pack://com.acme.neon@1.2.0/C:/Windows/secret.cube",
        "pack://com.acme.neon@1.2.0/luts/a:b.cube",
        "pack://com.acme.neon@1.2.0/luts/a\0.cube",
    ] {
        assert_eq!(AssetPath::parse(path), None, "{path}");
    }
    let mut p = fixture();
    p.assets[0].path = "pack://com.acme.neon@1.2.0/../../secret.mp4".into();
    assert_eq!(
        validate(&p).unwrap_err().to_string(),
        "Media screen has an invalid file path."
    );
}

#[test]
fn public_shapes_are_frozen() {
    let mut p = fixture();
    p.requires.insert("effects@1".into());
    // Format 4's recording fields: a game take with a marker, and a clip's own mix of its mic.
    let rec = &mut p.recordings[0];
    rec.kind = RecordingKind::Gameplay;
    rec.sources[0].role = SourceRole::Game;
    rec.sources[0].label = "capture.source.game".into();
    rec.game = Some(GameInfo {
        app_id: "com.example.racer".into(),
        name: "Racer".into(),
        profile_id: Some("profile".into()),
    });
    // The cursor layer: the take's cursor file, burned in from 9 s by the fallback (shape only: game takes keep no
    // separate cursor).
    rec.cursor = Some(RecordingCursor {
        path: "/media/rec.cursor".into(),
        capture: CursorCapture::BurnedFrom { at_us: 9_000_000 },
        clicks: ClickSource::Sampled,
    });
    rec.markers.push(Marker {
        id: "m1".into(),
        at_us: 12_500_000,
        kind: MarkerKind::Hotkey,
        label: Some("Boss fight".into()),
    });
    // A mic on input 1 of a two-input interface, played centred.
    p.timeline.main[0].source_mix.insert(
        "mic".into(),
        SourceMix {
            volume: 0.5,
            centre_from: Some(0),
            ..SourceMix::default()
        },
    );
    p.timeline.main[0].look.preset = Some(builtin::look("warm"));
    // A clip's own cursor style, and a portrait crop following the cursor a little closer.
    p.timeline.main[0].cursor = Some(CursorStyle {
        look: CursorLook::Dot,
        hide_when_still_us: Some(2_000_000),
        click: ClickEffect::RippleAndPress,
        ..CursorStyle::default()
    });
    p.timeline.main[0].framing.insert(
        FormatKind::Portrait,
        Framing {
            mode: FramingMode::Fill,
            focus_x: 0.5,
            focus_y: 0.5,
            follow: Some(Follow { closer: 1.5 }),
        },
    );
    p.timeline.main[0].effects = vec![EffectInstance {
        id: "fx".into(),
        ..blur()
    }];
    let gradient = layer(
        "g",
        ItemContent::generator(
            builtin::GENERATOR_GRADIENT.into(),
            BTreeMap::from([("angle".into(), ParamValue::Number(45.0))]),
        ),
    );
    let mut adjust = layer("adj", ItemContent::Adjustment);
    // An Auto zoom planned from clicks and typing only.
    let smart = layer(
        "smart",
        ItemContent::Zoom {
            target: ZoomTarget::Auto {
                using: vec![FocusKind::Clicks, FocusKind::Typing],
            },
            scale: 2.0,
        },
    );
    adjust.effects = vec![EffectInstance {
        id: "fx".into(),
        ..EffectInstance::new(builtin::EFFECT_PIXELATE)
    }];
    // Sync: a synced recording with a sounding camera and a mic that starts inside its file.
    let sync = sync_shapes(&mut p);
    let p = apply_ops(
        &p,
        &[
            Op::AddItem {
                track: TrackTarget::Auto,
                item: gradient,
                attach: false,
            },
            Op::AddItem {
                track: TrackTarget::Auto,
                item: adjust,
                attach: false,
            },
            Op::AddItem {
                track: TrackTarget::Auto,
                item: smart,
                attach: false,
            },
        ],
    )
    .unwrap();
    assert_golden(golden("project-v4.json"), &p);

    let mut ops = vec![
        Op::AddEffect {
            target: EffectTarget::Item { id: "adj".into() },
            effect: blur(),
            index: Some(0),
        },
        Op::UpdateEffect {
            target: on_clip("a"),
            id: "fx".into(),
            patch: EffectPatch {
                enabled: Some(true),
                params: Some(BTreeMap::from([("radius".into(), ParamValue::Number(8.0))])),
                reset: Some(vec!["old".into()]),
            },
        },
        Op::MoveEffect {
            target: on_clip("a"),
            id: "fx".into(),
            index: 0,
        },
        Op::RemoveEffect {
            target: EffectTarget::Item { id: "adj".into() },
            id: "fx".into(),
        },
        Op::SetTransition {
            clip_id: "a".into(),
            transition: Some(Transition {
                effect: builtin::TRANSITION_WIPE.into(),
                duration_us: 400_000,
                params: BTreeMap::from([("direction".into(), ParamValue::Text("up".into()))]),
            }),
        },
        Op::FixWords {
            asset_id: "mic".into(),
            start_us: 1_200_000,
            end_us: 1_600_000,
            text: Some("Notion".into()),
        },
        Op::SetWordLook {
            asset_id: "mic".into(),
            ranges: vec![
                klipolo_model::time::RangeUs {
                    start: 1_200_000,
                    end: 1_600_000,
                },
                klipolo_model::time::RangeUs {
                    start: 2_000_000,
                    end: 2_400_000,
                },
            ],
            look: klipolo_model::captions::looks::Look {
                color: Some("#32D74B".into()),
                emphasis: Some(true),
                size: Some(2),
                highlight: Some(true),
                hidden: Some(false),
                line_break: Some(true),
            },
        },
        Op::UpdateRecordingSource {
            recording_id: "rec".into(),
            source_id: "mic".into(),
            patch: SourcePatch {
                label: Some("Voice".into()),
                nudge_us: Some(-40_000),
                ..SourcePatch::default()
            },
        },
        Op::SetClipSourceMix {
            clip_id: "a".into(),
            source_id: "mic".into(),
            mix: Some(SourceMix {
                volume: 0.5,
                ..SourceMix::default()
            }),
        },
        Op::DetachSource {
            clip_id: "a".into(),
            source_id: "camera".into(),
        },
        Op::AddMarker {
            recording_id: "rec".into(),
            marker: Marker {
                id: "m2".into(),
                at_us: 30_000_000,
                kind: MarkerKind::Manual,
                label: None,
            },
        },
        Op::RemoveMarker {
            recording_id: "rec".into(),
            id: "m1".into(),
        },
        Op::SetCursorStyle {
            target: CursorTarget::Clips {
                ids: vec!["a".into()],
            },
            style: Some(CursorStyle {
                smoothing: Smoothing::Light,
                ..CursorStyle::default()
            }),
        },
        Op::SetCursorStyle {
            target: CursorTarget::Project,
            style: Some(CursorStyle::default()),
        },
    ];
    ops.extend(sync);
    let tx = Transaction {
        label: "Effects".into(),
        ops,
        origin: Some(Origin::Plugin {
            id: "com.acme.neon".into(),
            version: "1.2.0".into(),
        }),
        ..Default::default()
    };
    assert_golden(golden("ops-v4.json"), &tx);
}

#[test]
fn builtin_catalog_json_is_current() {
    let defs: Vec<_> = Catalog::builtin().defs().cloned().collect();
    assert_eq!(
        defs.iter().filter(|d| d.kind == ContribKind::Look).count(),
        7
    );
    assert_golden(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/lib/builtin-catalog.json"),
        &defs,
    );
}

/// A setting may apply only while a choice has certain values (the mask's corner radius, for rounded and frame). It
/// reads and writes as the manifest writes it, is left out when it always applies, and a definition whose condition
/// names no choice setting, a value that isn't an option, or a loop is refused when it loads.
#[test]
fn settings_can_depend_on_a_choice() {
    let manifest = serde_json::json!({
        "key": "cornerRadius", "label": "Corner radius", "type": "float", "min": 0, "max": 0.5, "default": 0.2,
        "visibleWhen": { "key": "shape", "in": ["rounded", "frame"] },
    });
    let spec: ParamSpec = serde_json::from_value(manifest.clone()).unwrap();
    let json = serde_json::to_value(&spec).unwrap();
    assert_eq!(json["visibleWhen"], manifest["visibleWhen"]);
    assert_eq!(serde_json::from_value::<ParamSpec>(json).unwrap(), spec);

    let catalog = Catalog::builtin();
    let mask = catalog.get(&builtin::EFFECT_MASK.into()).unwrap();
    assert_eq!(
        mask.param("cornerRadius").unwrap().visible_when,
        spec.visible_when
    );
    let feather = serde_json::to_value(mask.param("feather").unwrap()).unwrap();
    assert!(feather.get("visibleWhen").is_none(), "{feather}");
    for def in catalog.defs() {
        def.check_conditions()
            .unwrap_or_else(|e| panic!("{}: {e}", def.id));
    }

    let choice = |key: &str, options: &[&str]| ParamSpec {
        key: key.into(),
        label: key.into(),
        visible_when: None,
        kind: ParamKind::Choice {
            options: options.iter().map(|o| (*o).to_string()).collect(),
            default: options[0].into(),
        },
    };
    let when = |spec: ParamSpec, key: &str, values: &[&str]| ParamSpec {
        visible_when: Some(VisibleWhen {
            key: key.into(),
            values: values.iter().map(|v| (*v).to_string()).collect(),
        }),
        ..spec
    };
    let check = |params: Vec<ParamSpec>| {
        ContribDef {
            params,
            ..ContribDef::new("com.acme.fx/frame", ContribKind::Effect, "Frame")
        }
        .check_conditions()
    };
    let shape = || choice("shape", &["round", "square"]);
    let radius = || ParamSpec {
        key: "radius".into(),
        label: "Radius".into(),
        visible_when: None,
        kind: ParamKind::Angle { default: 0.0 },
    };
    assert_eq!(
        check(vec![shape(), when(radius(), "shape", &["round"])]),
        Ok(())
    );
    assert_eq!(
        check(vec![shape(), when(radius(), "corner", &["round"])]),
        Err("com.acme.fx/frame: \"radius\" depends on \"corner\", which isn't a choice setting of this contribution.".into())
    );
    assert_eq!(
        check(vec![radius(), when(shape(), "radius", &["round"])]),
        Err("com.acme.fx/frame: \"shape\" depends on \"radius\", which isn't a choice setting of this contribution.".into())
    );
    assert_eq!(
        check(vec![shape(), when(radius(), "shape", &["round", "oval"])]),
        Err("com.acme.fx/frame: \"radius\" depends on \"shape\" being \"oval\", which isn't one of its options.".into())
    );
    assert_eq!(
        check(vec![shape(), when(radius(), "shape", &[])]),
        Err("com.acme.fx/frame: \"radius\" lists no value of \"shape\" that shows it.".into())
    );
    let style = choice("style", &["solid", "dashed"]);
    assert_eq!(
        check(vec![
            when(shape(), "style", &["solid"]),
            when(style, "shape", &["round"]),
        ]),
        Err(
            "com.acme.fx/frame: \"shape\" depends on itself through its settings' conditions."
                .into()
        )
    );
}
