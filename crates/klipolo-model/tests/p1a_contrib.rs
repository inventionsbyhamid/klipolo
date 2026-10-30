//! What contributions say about themselves: the new definition fields, padding, table settings, font
//! settings that hold a `FontRef`, library ids, JSON schemas, and `scenes@1` in a project's requirements.
mod common;

use klipolo_model::contrib::{ContribDef, ContribKind, PaddingSpec, Resolution};
use klipolo_model::params::{ParamKind, ParamSpec, ParamValue, TableRow};
use std::collections::BTreeMap;

#[test]
fn contribution_json_round_trips_with_the_new_fields_left_out_when_unset() {
    let plain = ContribDef::new(
        "lib.k3f9q2ma/generator.waves.v1",
        ContribKind::Generator,
        "Waves",
    );
    let json = serde_json::to_string(&plain).unwrap();
    assert!(!json.contains("padding") && !json.contains("provenance") && !json.contains("tags"));
    let full = ContribDef {
        description: "Crossing swells".into(),
        tags: vec!["water".into()],
        resolution: Some(Resolution::Auto),
        fps: Some(12),
        padding: Some(PaddingSpec {
            px: 4.0,
            param: Some("radius".into()),
            per_unit: 2.0,
            max_px: 64.0,
        }),
        ..plain
    };
    let back: ContribDef = serde_json::from_str(&serde_json::to_string(&full).unwrap()).unwrap();
    assert_eq!(back, full);
}

#[test]
fn padding_grows_with_its_setting_and_stops_at_its_cap() {
    let def = ContribDef {
        params: vec![ParamSpec {
            key: "radius".into(),
            label: "Radius".into(),
            visible_when: None,
            kind: ParamKind::Float {
                min: 0.0,
                max: 200.0,
                default: 12.0,
                step: None,
                unit: None,
            },
        }],
        ..ContribDef::new("lib.k3f9q2ma/effect.glow.v1", ContribKind::Effect, "Glow")
    };
    let pad = PaddingSpec {
        px: 4.0,
        param: Some("radius".into()),
        per_unit: 2.0,
        max_px: 64.0,
    };
    assert_eq!(pad.px_for(&BTreeMap::new(), &def), 28.0); // 4 + 12 × 2 from the default
    let big = BTreeMap::from([("radius".to_owned(), ParamValue::Number(200.0))]);
    assert_eq!(pad.px_for(&big, &def), 64.0);
    let absurd = PaddingSpec {
        max_px: 10_000.0,
        ..pad
    };
    assert_eq!(
        absurd.px_for(&big, &def),
        klipolo_model::contrib::MAX_PADDING_PX
    );
}

/// The margin reads its setting as the shader draws it, within the setting's own bounds: a layer
/// still holding 35 for a radius whose maximum is now 20 pads for 20 (60 px), never 105.
#[test]
fn a_margin_reads_its_setting_within_the_settings_own_bounds() {
    let def = ContribDef {
        params: vec![ParamSpec {
            key: "radius".into(),
            label: "Radius".into(),
            visible_when: None,
            kind: ParamKind::Float {
                min: 0.0,
                max: 20.0,
                default: 4.0,
                step: None,
                unit: None,
            },
        }],
        ..ContribDef::new("lib.k3f9q2ma/effect.glow.v2", ContribKind::Effect, "Glow")
    };
    let pad = PaddingSpec {
        px: 0.0,
        param: Some("radius".into()),
        per_unit: 3.0,
        max_px: 120.0,
    };
    let held = BTreeMap::from([("radius".to_owned(), ParamValue::Number(35.0))]);
    assert_eq!(pad.px_for(&held, &def), 60.0);
    let below = BTreeMap::from([("radius".to_owned(), ParamValue::Number(-5.0))]);
    assert_eq!(pad.px_for(&below, &def), 0.0);
}

#[test]
fn rows_are_checked_against_their_table() {
    let spec = ParamSpec {
        key: "data".into(),
        label: "Data".into(),
        visible_when: None,
        kind: ParamKind::Table {
            columns: vec!["value".into()],
            max_rows: 3,
            default: vec![],
        },
    };
    let p = klipolo_model::Project::new("t");
    let row = |l: &str, v: f64| TableRow {
        label: l.into(),
        values: vec![v],
    };
    assert!(spec
        .check(&ParamValue::Rows(vec![row("a", 1.0), row("b", 2.0)]), &p)
        .is_ok());
    assert!(spec
        .check(&ParamValue::Rows(vec![row("a", 1.0); 4]), &p)
        .is_err()); // too many rows
    assert!(spec
        .check(
            &ParamValue::Rows(vec![TableRow {
                label: "a".into(),
                values: vec![1.0, 2.0]
            }]),
            &p
        )
        .is_err()); // columns
    assert!(spec
        .check(&ParamValue::Rows(vec![row("a", f64::NAN)]), &p)
        .is_err());
    assert!(spec
        .check(&ParamValue::Rows(vec![row(&"x".repeat(41), 1.0)]), &p)
        .is_err());
}

#[test]
fn a_scene_layer_keeps_scenes_at_1_in_requires_and_removing_it_drops_it() {
    let mut p = common::project_with_overlay(vec![]);
    common::apply(
        &mut p,
        common::add_generator_op("lt", "klipolo.builtin/scene.lower-third"),
    );
    assert!(p.requires.contains("scenes@1"));
    common::apply(
        &mut p,
        klipolo_model::ops::Op::RemoveItem { id: "lt".into() },
    );
    assert!(!p.requires.contains("scenes@1"));
    common::apply(
        &mut p,
        common::add_generator_op("g", "klipolo.builtin/generator.gradient"),
    );
    assert!(
        !p.requires.contains("scenes@1"),
        "a shader generator isn't a scene"
    );
}

#[test]
fn library_ids_are_recognised_and_param_types_have_json_schemas() {
    assert!(klipolo_model::ContribId::from("lib.k3f9q2ma/generator.waves.v1").is_library());
    assert!(!klipolo_model::ContribId::from("com.acme.neon/glow").is_library());
    let schema =
        serde_json::to_value(schemars::schema_for!(klipolo_model::params::ParamSpec)).unwrap();
    assert!(
        schema.to_string().contains("table"),
        "the Table kind is in the schema"
    );
}

#[test]
fn a_font_setting_holds_a_font_ref_and_round_trips() {
    use klipolo_model::font::{FontRef, FontSource};
    let google = FontRef {
        family: "Montserrat".into(),
        source: FontSource::Google {
            version: "v31".into(),
        },
    };
    let v = ParamValue::Font(google.clone());
    let back: ParamValue = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
    assert_eq!(back, v, "not read as a point, a colour or rows");
    let spec = ParamSpec {
        key: "font".into(),
        label: "Font".into(),
        visible_when: None,
        kind: ParamKind::Font {
            default: FontRef::inter(),
        },
    };
    let p = klipolo_model::Project::new("t");
    assert!(spec.check(&v, &p).is_ok());
    assert!(spec
        .check(
            &ParamValue::Font(FontRef {
                family: "".into(),
                source: FontSource::Installed
            }),
            &p
        )
        .is_err());
}

/// A definition's padding is finite and never negative (the size caps, checked where definitions load).
#[test]
fn a_padding_that_isnt_finite_or_is_negative_is_refused() {
    let def = |padding| ContribDef {
        padding: Some(padding),
        ..ContribDef::new("lib.k3f9q2ma/effect.glow.v1", ContribKind::Effect, "Glow")
    };
    let ok = PaddingSpec {
        px: 4.0,
        param: None,
        per_unit: 0.0,
        max_px: 64.0,
    };
    assert!(klipolo_model::validate::check_padding(&def(ok.clone())).is_ok());
    for bad in [
        PaddingSpec {
            px: -1.0,
            ..ok.clone()
        },
        PaddingSpec {
            per_unit: f32::NAN,
            ..ok.clone()
        },
        PaddingSpec {
            max_px: f32::INFINITY,
            ..ok.clone()
        },
    ] {
        let err = klipolo_model::validate::check_padding(&def(bad)).unwrap_err();
        assert_eq!(err.coded().code, "model.contrib.padding");
    }
}
