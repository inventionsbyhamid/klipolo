//! The envelope clamp: numbers into their range, bools and choices outside their measured values at their
//! default (or the first measured), unreadable values never outside, and nothing to clamp borrows.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use klipolo_model::params::{ParamSpec, ParamValue};

use super::clamped;
use crate::make::envelope::Envelope;

fn specs() -> Vec<ParamSpec> {
    serde_json::from_str(
        r#"[{"key":"steps","label":"Steps","type":"int","min":1,"max":64,"default":8},
            {"key":"fancy","label":"Fancy","type":"bool","default":false},
            {"key":"mode","label":"Mode","type":"choice","options":["a","b","c"],"default":"a"}]"#,
    )
    .unwrap()
}

fn envelope() -> Envelope {
    Envelope {
        ranges: BTreeMap::from([("steps".into(), (1.0, 22.0))]),
        allowed: BTreeMap::from([
            ("fancy".into(), BTreeSet::from([0])),
            ("mode".into(), BTreeSet::from([1, 2])),
        ]),
    }
}

fn values(pairs: &[(&str, ParamValue)]) -> BTreeMap<String, ParamValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn settings_past_the_envelope_draw_at_what_was_measured() {
    let held = values(&[
        ("steps", ParamValue::Number(60.0)),
        ("fancy", ParamValue::Bool(true)),
        ("mode", ParamValue::Text("b".into())),
    ]);
    let out = clamped(&held, &specs(), Some(&envelope()));
    assert_eq!(out.get("steps"), Some(&ParamValue::Number(22.0)));
    assert_eq!(
        out.get("fancy"),
        Some(&ParamValue::Bool(false)),
        "its default"
    );
    assert_eq!(out.get("mode"), Some(&ParamValue::Text("b".into())));
    assert_eq!(
        held.get("steps"),
        Some(&ParamValue::Number(60.0)),
        "the layer's own value stays"
    );
    // A choice at its default, which wasn't measured within class: the first value that was.
    let none = values(&[]);
    let out = clamped(&none, &specs(), Some(&envelope()));
    assert_eq!(out.get("mode"), Some(&ParamValue::Text("b".into())));
}

/// Inside the envelope, or a built-in (no envelope), the layer's own settings are drawn, borrowed: nothing is
/// allocated a frame. A value the renderer can't read (text for a number) is its default, never outside.
#[test]
fn nothing_to_clamp_borrows() {
    let inside = values(&[
        ("steps", ParamValue::Number(12.0)),
        ("mode", ParamValue::Text("c".into())),
        ("fancy", ParamValue::Text("not a bool".into())),
    ]);
    assert!(matches!(
        clamped(&inside, &specs(), Some(&envelope())),
        Cow::Borrowed(_)
    ));
    let wild = values(&[("steps", ParamValue::Number(1e9))]);
    assert!(matches!(clamped(&wild, &specs(), None), Cow::Borrowed(_)));
    let unreadable = values(&[
        ("steps", ParamValue::Text("abc".into())),
        ("mode", ParamValue::Text("b".into())),
    ]);
    assert!(
        matches!(
            clamped(&unreadable, &specs(), Some(&envelope())),
            Cow::Borrowed(_)
        ),
        "drawn at its default 8, inside 1–22"
    );
}
