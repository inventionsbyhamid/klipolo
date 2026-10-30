//! The measurement's plan, its probe combinations and the envelope's clamp on the CPU.

use std::collections::{BTreeMap, BTreeSet};

use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::params::ParamValue;

use super::*;

fn def(params: &str) -> ContribDef {
    ContribDef {
        params: serde_json::from_str(params).unwrap(),
        ..ContribDef::new("lib.k3f9q2ma/generator.t.v1", ContribKind::Generator, "T")
    }
}

/// A bool or choice with no value is its default; when the envelope left the default out, it clamps to the first
/// allowed value.
#[test]
fn a_missing_bool_or_choice_whose_default_was_left_out_clamps_to_an_allowed_value() {
    let d = def(
        r#"[{"key":"fancy","label":"F","type":"bool","default":true},
            {"key":"mode","label":"M","type":"choice","options":["a","b","c"],"default":"c"}]"#,
    );
    let env = Envelope {
        ranges: BTreeMap::new(),
        allowed: BTreeMap::from([
            ("fancy".to_owned(), BTreeSet::from([0])),
            ("mode".to_owned(), BTreeSet::from([0, 1])),
        ]),
    };
    let mut params = Params::new();
    clamp(&d, &env, &mut params);
    assert_eq!(params.get("fancy"), Some(&ParamValue::Bool(false)));
    assert_eq!(params.get("mode"), Some(&ParamValue::Text("a".into())));
}

/// The clamp writes back what the renderer draws, so every reader of the settings (the margin's
/// too) sees it: a radius held past the definition's maximum is written at the maximum, an option the definition
/// doesn't have at the default; a setting the layer leaves out stays out while its default is inside.
#[test]
fn the_clamp_writes_back_what_is_drawn() {
    let d = def(
        r#"[{"key":"radius","label":"R","type":"float","min":0,"max":20,"default":4},
            {"key":"mode","label":"M","type":"choice","options":["a","b"],"default":"a"},
            {"key":"gain","label":"G","type":"float","min":0,"max":1,"default":0.5}]"#,
    );
    let env = Envelope {
        ranges: BTreeMap::from([
            ("radius".to_owned(), (0.0, 20.0)),
            ("gain".to_owned(), (0.0, 1.0)),
        ]),
        allowed: BTreeMap::from([("mode".to_owned(), BTreeSet::from([0, 1]))]),
    };
    let mut params = Params::from([
        ("radius".to_owned(), ParamValue::Number(35.0)),
        ("mode".to_owned(), ParamValue::Text("z".into())),
    ]);
    clamp(&d, &env, &mut params);
    assert_eq!(params.get("radius"), Some(&ParamValue::Number(20.0)));
    assert_eq!(params.get("mode"), Some(&ParamValue::Text("a".into())));
    assert_eq!(params.get("gain"), None);
}

fn costly(list: &[(&str, End)]) -> Vec<CostlyParam> {
    list.iter()
        .map(|(key, end)| CostlyParam {
            key: (*key).into(),
            costly_end: *end,
        })
        .collect()
}

fn num(params: &Params, key: &str) -> f64 {
    match params.get(key) {
        Some(ParamValue::Number(x)) => *x,
        other => panic!("{key}: {other:?}"),
    }
}

const NUMBERS: &str = r#"[{"key":"a","label":"A","type":"int","min":1,"max":64,"default":8},
    {"key":"b","label":"B","type":"float","min":0,"max":1,"default":0.5},
    {"key":"c","label":"C","type":"float","min":0,"max":1,"default":0.5},
    {"key":"d","label":"D","type":"float","min":0,"max":1,"default":0.5},
    {"key":"e","label":"E","type":"float","min":0,"max":1,"default":0.5},
    {"key":"f","label":"F","type":"float","min":0,"max":1,"default":0.5},
    {"key":"fancy","label":"F","type":"bool","default":false},
    {"key":"mode","label":"M","type":"choice","options":["x","y","z"],"default":"x"}]"#;

/// The worst corner puts every one-ended number at its costly end; the first two numbers costly at both ends get
/// corners of their own and any more move together; they are measured inside their range; every other bool and choice
/// value is listed.
#[test]
fn the_plan_groups_numbers_costly_at_both_ends_and_lists_every_value() {
    let d = def(NUMBERS);
    let list = costly(&[
        ("a", End::Max),
        ("b", End::Min),
        ("c", End::Both),
        ("d", End::Both),
        ("e", End::Both),
        ("f", End::Both),
        ("fancy", End::Max),
        ("mode", End::Max),
    ]);
    let p = plan(&d, &list, &Params::new());
    assert_eq!(p.numbers.len(), 6, "{:#?}", p.numbers);
    assert!(
        p.numbers
            .iter()
            .all(|c| num(c, "a") == 64.0 && num(c, "b") == 0.0)
    );
    let last = &p.numbers[5];
    assert_eq!(
        (num(last, "e"), num(last, "f")),
        (1.0, 1.0),
        "moved together"
    );
    assert_eq!((num(last, "c"), num(last, "d")), (0.5, 0.5));
    assert_eq!(p.interior, ["c", "d", "e", "f"]);
    assert_eq!(
        p.values,
        vec![
            ("fancy".to_owned(), 1),
            ("mode".to_owned(), 1),
            ("mode".to_owned(), 2)
        ]
    );
    assert!(corners(&d, &list).len() <= MAX_CORNERS);
}

/// More corners than `MAX_CORNERS` are cut there, the defaults first.
#[test]
fn corners_are_capped() {
    let d = def(
        r#"[{"key":"c","label":"C","type":"float","min":0,"max":1,"default":0.5},
            {"key":"d","label":"D","type":"float","min":0,"max":1,"default":0.5},
            {"key":"mode","label":"M","type":"choice","options":["a","b","c","d","e","f","g","h","i","j"],"default":"a"},
            {"key":"x","label":"X","type":"bool","default":false},
            {"key":"y","label":"Y","type":"bool","default":false}]"#,
    );
    let list = costly(&[
        ("c", End::Both),
        ("d", End::Both),
        ("mode", End::Max),
        ("x", End::Max),
        ("y", End::Max),
    ]);
    let all = corners(&d, &list);
    assert_eq!(all.len(), MAX_CORNERS);
    assert_eq!(all[0], defaults(&d));
    assert_eq!(
        plan(&d, &list, &Params::new()).values.len(),
        7 + 2,
        "8 options at most"
    );
}

/// The probe combinations move every probed setting and interior number together by index, so every
/// probe value of every setting is measured however many there are, in at most 9 combinations; a setting with fewer
/// values stays at its own afterwards.
#[test]
fn probe_combinations_cover_every_value_of_every_setting() {
    let d = def(
        r##"[{"key":"p","label":"P","type":"point","default":{"x":0.25,"y":0.25}},
             {"key":"q","label":"Q","type":"point","default":{"x":0.75,"y":0.75}},
             {"key":"tint","label":"T","type":"color","default":"#123456"},
             {"key":"n","label":"N","type":"int","min":1,"max":22,"default":1}]"##,
    );
    let list = costly(&[("p", End::Probe), ("q", End::Probe), ("tint", End::Probe)]);
    let p = plan(&d, &list, &Params::new());
    let combos = probe_combos(&d, &p.base, &p.probed, &[("n".into(), (1.0, 22.0))]);
    assert_eq!(combos.len(), 9);
    assert!(combos.len() <= MAX_PROBES);
    for (key, set) in &p.probed {
        for v in &set[1..] {
            assert!(
                combos.iter().any(|c| c.get(key) == Some(v)),
                "{key} at {v:?}"
            );
        }
    }
    let n: Vec<f64> = combos.iter().map(|c| num(c, "n")).collect();
    assert_eq!(
        n[..3],
        [6.0, 12.0, 17.0],
        "25, 50 and 75 % of 1–22, rounded"
    );
    assert_eq!(n[3..], [1.0; 6], "then its own value");
    assert_eq!(combos[8].get("tint"), p.base.get("tint"));
}

/// A narrowed int moves toward the default and is rounded toward it, so what is measured is what the range covers.
#[test]
fn toward_and_range_round_an_int_toward_the_default() {
    let d = def(NUMBERS);
    let base = defaults(&d);
    let mut corner = base.clone();
    corner.insert("a".into(), ParamValue::Number(63.0));
    assert_eq!(
        num(&toward(&d, &base, &corner, 0.5), "a"),
        35.0,
        "35.5 down"
    );
    corner.insert("a".into(), ParamValue::Number(1.0));
    assert_eq!(num(&toward(&d, &base, &corner, 0.5), "a"), 5.0, "4.5 up");
    let a = d.param("a").unwrap();
    assert_eq!(range(a, &base, (false, true), 0.25), Some((1.0, 22.0)));
    assert_eq!(range(a, &base, (true, false), 0.5), Some((5.0, 64.0)));
    let b = d.param("b").unwrap();
    assert_eq!(range(b, &base, (false, true), 0.5), Some((0.0, 0.75)));
    assert_eq!(
        range(d.param("fancy").unwrap(), &base, (true, true), 0.5),
        None
    );
}

/// A colour's probes are black, white and the primaries, opaque when it has alpha, then its own colour clear; a
/// point's are its default and the 3 × 3 grid.
#[test]
fn probe_values_of_colours_and_points() {
    let d = def(
        r##"[{"key":"c","label":"C","type":"color","default":"#11223344","alpha":true},
             {"key":"k","label":"K","type":"color","default":"#000000"},
             {"key":"p","label":"P","type":"point","default":{"x":0.25,"y":0.25}}]"##,
    );
    let text = |key: &str| -> Vec<String> {
        probe_values(d.param(key).unwrap())
            .into_iter()
            .map(|v| match v {
                ParamValue::Text(t) => t,
                other => panic!("{other:?}"),
            })
            .collect()
    };
    let c = text("c");
    assert_eq!(c.len(), 7);
    assert!(c[1..6].iter().all(|v| v.ends_with("FF") && v.len() == 9));
    assert_eq!(c[6], "#11223300");
    assert_eq!(text("k").len(), 5, "black is its default");
    assert_eq!(probe_values(d.param("p").unwrap()).len(), 10);
}

/// A Measure that gives a probed colour or point another value still probes that setting's default too, so
/// the default's cost is never left out (fails closed).
#[test]
fn a_probed_setting_given_another_value_still_probes_its_default() {
    let d = def(
        r##"[{"key":"tint","label":"T","type":"color","default":"#123456"},
             {"key":"p","label":"P","type":"point","default":{"x":0.25,"y":0.25}}]"##,
    );
    let list = costly(&[("tint", End::Probe), ("p", End::Probe)]);
    let mut values = Params::new();
    values.insert("tint".into(), ParamValue::Text("#FF00FF".into()));
    let elsewhere = klipolo_model::params::PointN { x: 0.9, y: 0.1 };
    values.insert("p".into(), ParamValue::Point(elsewhere));
    let p = plan(&d, &list, &values);
    assert_eq!(
        p.base.get("tint"),
        Some(&ParamValue::Text("#FF00FF".into()))
    );
    let combos = probe_combos(&d, &p.base, &p.probed, &[]);
    let default_tint = ParamValue::Text("#123456".into());
    assert!(
        combos.iter().any(|c| c.get("tint") == Some(&default_tint)),
        "the colour's default"
    );
    let default_p = ParamValue::Point(klipolo_model::params::PointN { x: 0.25, y: 0.25 });
    assert!(
        combos.iter().any(|c| c.get("p") == Some(&default_p)),
        "the point's default"
    );
}
