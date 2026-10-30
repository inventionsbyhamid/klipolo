//! Where a typed parse failed: each top-level field, param, node (down its children) and tween is tried on its own,
//! and in a node or tween each key beside its `type`, so the problem names the key (`/nodes/0/fill`).

use serde::Deserialize;
use serde_json::{Map, Value};

use super::super::problems::{at, join, quote, Problems};
use super::super::{keys, Node, SceneKey, SceneLimits, Segments, Tween};
use crate::params::ParamSpec;

/// Where the typed parse failed: each top-level field, param, node (down its children) and tween is tried on its
/// own, and in a node or tween each key beside its `type`, so the problem names the key. Only reached when the
/// scene is already refused, so its copies don't matter.
pub(super) fn locate(root: &Map<String, Value>, whole: &serde_json::Error, out: &mut Problems) {
    let was_empty = out.is_empty();
    if let Some(v) = root.get("duration") {
        if f64::deserialize(v).is_err() {
            value_problem(out, "/duration", "duration", "seconds, a number");
        }
    }
    if let Some(v) = root.get("segments") {
        if let Err(e) = Segments::deserialize(v) {
            push_value(
                out,
                "/segments".into(),
                "segments",
                "{\"in\": seconds, \"out\": seconds}",
                &e,
            );
        }
    }
    for (name, what) in [
        ("params", "a list of settings"),
        ("nodes", "a list of nodes"),
        ("timeline", "a list of tweens"),
    ] {
        let Some(v) = root.get(name) else { continue };
        let Some(items) = v.as_array() else {
            value_problem(out, &format!("/{name}"), name, what);
            continue;
        };
        for (i, item) in items.iter().enumerate() {
            let path = at(&format!("/{name}"), i);
            match name {
                "params" => {
                    if let Err(e) = ParamSpec::deserialize(item) {
                        out.push(
                            path,
                            "scene.value",
                            format!("This setting can't be read: {e}."),
                        );
                    }
                }
                "nodes" => locate_node(item, &path, 1, out),
                _ => locate_tween(item, &path, out),
            }
        }
    }
    if was_empty && out.is_empty() {
        out.push(
            "",
            "scene.value",
            format!("The scene doesn't match scene@1: {whole}."),
        );
    }
}

fn locate_node(v: &Value, path: &str, depth: usize, out: &mut Problems) {
    if depth > SceneLimits::MAX_DEPTH {
        return;
    }
    let Some(obj) = v.as_object() else {
        value_problem(out, path, "child", "a node: {\"type\": \"rect\", …}");
        return;
    };
    // A leaf is tried whole first (most are fine); a container's children are tried one by one instead, so its
    // subtree isn't parsed twice.
    let container = obj.contains_key("children") || obj.contains_key("child");
    if !container && Node::deserialize(v).is_ok() {
        return;
    }
    let mut base = Map::new();
    if let Some(t) = obj.get("type") {
        base.insert("type".into(), t.clone());
    }
    if Node::deserialize(&Value::Object(base.clone())).is_err() {
        // A missing or unknown type: the walk named it.
        return;
    }
    let before = out.len();
    for (key, val) in obj {
        let key_path = join(path, key);
        match (key.as_str(), val) {
            ("type", _) => {}
            ("children", Value::Array(items)) => {
                for (i, child) in items.iter().enumerate() {
                    locate_node(child, &at(&key_path, i), depth + 1, out);
                }
            }
            ("child", Value::Object(_)) => locate_node(val, &key_path, depth + 1, out),
            ("animate", Value::Object(props)) => {
                for (prop, list) in props {
                    let prop_path = join(&key_path, prop);
                    match list.as_array() {
                        Some(list) => {
                            for (i, k) in list.iter().enumerate() {
                                if let Err(e) = SceneKey::deserialize(k) {
                                    push_value(
                                        out,
                                        at(&prop_path, i),
                                        "key",
                                        "{\"t\": seconds, \"v\": value}",
                                        &e,
                                    );
                                }
                            }
                        }
                        None => value_problem(
                            out,
                            &prop_path,
                            "keys",
                            "a list of keys: [{\"t\": 0, \"v\": 1}]",
                        ),
                    }
                }
            }
            _ => {
                let mut one = base.clone();
                one.insert(key.clone(), val.clone());
                if let Err(e) = Node::deserialize(&Value::Object(one)) {
                    push_value(out, key_path, key, keys::expects(key), &e);
                }
            }
        }
    }
    if out.len() == before && !out.has_path(path) && (!container || Node::deserialize(v).is_err()) {
        out.push(path, "scene.value", "This node doesn't match scene@1.");
    }
}

fn locate_tween(v: &Value, path: &str, out: &mut Problems) {
    if Tween::deserialize(v).is_ok() {
        return;
    }
    let Some(obj) = v.as_object() else {
        value_problem(
            out,
            path,
            "tween",
            "a tween: {\"to\": \"#id\", \"dur\": 0.5, \"opacity\": 1}",
        );
        return;
    };
    let before = out.len();
    for (key, val) in obj {
        let mut one = Map::new();
        one.insert(key.clone(), val.clone());
        if let Err(e) = Tween::deserialize(&Value::Object(one)) {
            let what = if keys::nested::TWEEN.contains(&key.as_str()) {
                keys::expects(key)
            } else {
                "a number or a string (a colour, \"50%\", \"22px\", a clip or a \"$key\")"
            };
            push_value(out, join(path, key), key, what, &e);
        }
    }
    if out.len() == before {
        out.push(path, "scene.value", "This tween doesn't match scene@1.");
    }
}

fn value_problem(out: &mut Problems, path: &str, key: &str, what: &str) {
    out.push(
        path,
        "scene.value",
        format!("`{}` must be {what}.", quote(key)),
    );
}

/// "`fill` must be …", with serde's own words when they say more than "no variant matched".
fn push_value(out: &mut Problems, path: String, key: &str, what: &str, e: &serde_json::Error) {
    let detail = e.to_string();
    let message = if detail.contains("did not match any variant") {
        format!("`{}` must be {what}.", quote(key))
    } else {
        format!("`{}` must be {what} ({}).", quote(key), quote_long(&detail))
    };
    out.push(path, "scene.value", message);
}

/// serde's message, cut to 200 characters.
fn quote_long(s: &str) -> String {
    let mut out: String = s.chars().take(200).collect();
    if s.chars().nth(200).is_some() {
        out.push('…');
    }
    out
}
