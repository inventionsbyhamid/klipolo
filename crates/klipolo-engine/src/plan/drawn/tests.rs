use std::collections::BTreeMap;

use klipolo_model::contrib::{ContribDef, ContribKind};

use super::*;
use crate::plan::pack_params;
use crate::registry::{ShaderEffect, ShaderKind};

const BODY: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";

/// A number 1–64 (default 8), an int 0–10 (default 3), a bool (off) and a choice of three (default "b").
fn def() -> ContribDef {
    ContribDef {
        params: serde_json::from_str(
            r#"[{"key":"steps","label":"S","type":"float","min":1,"max":64,"default":8},
                {"key":"count","label":"C","type":"int","min":0,"max":10,"default":3},
                {"key":"fancy","label":"F","type":"bool","default":false},
                {"key":"mode","label":"M","type":"choice","options":["a","b","c"],"default":"b"}]"#,
        )
        .unwrap(),
        ..ContribDef::new("lib.k3f9q2ma/generator.x.v1", ContribKind::Generator, "X")
    }
}

/// N-m5: a value the engine can't draw as it is (past the definition's own bounds, not finite, the wrong type, an option
/// the definition doesn't have) reads as what it draws, and the renderer packs exactly that.
#[test]
fn a_value_reads_as_what_the_renderer_draws() {
    let def = def();
    let spec = |key: &str| def.param(key).unwrap();
    let n = ParamValue::Number;
    let text = |t: &str| ParamValue::Text(t.into());
    assert_eq!(effective(spec("steps"), Some(&n(100.0))), Some(n(64.0)));
    assert_eq!(effective(spec("steps"), Some(&n(f64::NAN))), Some(n(8.0)));
    assert_eq!(effective(spec("steps"), Some(&text("x"))), Some(n(8.0)));
    assert_eq!(effective(spec("steps"), None), Some(n(8.0)));
    assert_eq!(effective(spec("count"), Some(&n(2.6))), Some(n(3.0)));
    assert_eq!(effective(spec("count"), Some(&n(-4.0))), Some(n(0.0)));
    assert_eq!(
        effective(spec("fancy"), Some(&text("yes"))),
        Some(ParamValue::Bool(false))
    );
    assert_eq!(effective(spec("mode"), Some(&text("z"))), Some(text("b")));
    assert_eq!(effective(spec("mode"), Some(&text("c"))), Some(text("c")));
    let shader = ShaderEffect::assemble(&def, ShaderKind::Generator, BODY).unwrap();
    let raw: BTreeMap<String, ParamValue> = [
        ("steps", n(100.0)),
        ("count", n(2.6)),
        ("fancy", text("yes")),
        ("mode", text("z")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let resolved: BTreeMap<String, ParamValue> = def
        .params
        .iter()
        .filter_map(|s| Some((s.key.clone(), effective(s, raw.get(&s.key))?)))
        .collect();
    assert_eq!(
        pack_params(&shader, &raw).bytes(),
        pack_params(&shader, &resolved).bytes()
    );
}
