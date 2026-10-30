//! The scene's keys outside its nodes: each param's keys by its type, and each tween's own keys and the properties it
//! animates.

use serde_json::Value;

use super::super::keys::{self, nested};
use super::super::problems::{join, Problems};
use super::{known, known_in, unknown};

pub(super) fn param(v: &Value, path: &str, out: &mut Problems) {
    let Value::Object(obj) = v else { return };
    // An unknown `type` is the typed parse's to name.
    if let Some(allowed) = obj
        .get("type")
        .and_then(Value::as_str)
        .and_then(nested::param)
    {
        known(obj, allowed, path, out);
    }
    if let Some(when) = obj.get("visibleWhen") {
        known_in(when, nested::VISIBLE_WHEN, &join(path, "visibleWhen"), out);
    }
}

pub(super) fn tween(v: &Value, path: &str, out: &mut Problems) {
    let Value::Object(obj) = v else { return };
    for (key, val) in obj {
        match key.as_str() {
            "stagger" => known_in(val, nested::STAGGER, &join(path, key), out),
            "start" => {
                if let Value::Object(start) = val {
                    let start_path = join(path, key);
                    for prop in start.keys() {
                        if keys::animatable(prop).is_none() {
                            let names: Vec<&str> =
                                keys::ANIMATABLE.iter().map(|(n, _, _)| *n).collect();
                            unknown(out, &start_path, prop, &names);
                        }
                    }
                }
            }
            k if nested::TWEEN.contains(&k) => {}
            k if keys::animatable(k).is_some() => {}
            k => {
                let mut names: Vec<&str> = nested::TWEEN.to_vec();
                names.extend(keys::ANIMATABLE.iter().map(|(n, _, _)| *n));
                unknown(out, path, k, &names);
            }
        }
    }
}
