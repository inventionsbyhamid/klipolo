//! The loader's walk over the raw JSON, before the typed parse: every key against [`super::keys`] (serde's `flatten`
//! can't deny unknown ones), node nesting, node types, and what a stored scene may not hold (`svg` nodes; image
//! `file`s, until packs exist). Values of the wrong shape are left to the typed parse.

use serde_json::{Map, Value};

use super::keys::{self, nested, ValueKind};
use super::problems::{at, join, quote, Problems};
use super::{NodeType, SceneLimits};

mod top;

/// Which entry point is reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Entry {
    Submission,
    Stored,
}

/// The most nodes the walk reads: five times the cap, so a 201-node scene still reaches the checker (which
/// names the cap), while a 256 KiB scene of thousands of tiny nodes is refused before anything is built.
const WALKED_NODES: usize = 1000;

pub(super) fn walk(root: &Map<String, Value>, entry: Entry, out: &mut Problems) {
    known(root, nested::TOP, "", out);
    for key in ["duration", "nodes"] {
        if !root.contains_key(key) {
            out.push(
                join("", key),
                "scene.required",
                format!("A scene needs \"{key}\"."),
            );
        }
    }
    if let Some(Value::Object(segments)) = root.get("segments") {
        known(segments, nested::SEGMENTS, "/segments", out);
    }
    if let Some(Value::Array(params)) = root.get("params") {
        for (i, p) in params.iter().enumerate() {
            top::param(p, &at("/params", i), out);
        }
    }
    if let Some(Value::Array(nodes)) = root.get("nodes") {
        let mut count = 0;
        for (i, n) in nodes.iter().enumerate() {
            node(n, &at("/nodes", i), 1, entry, &mut count, out);
        }
    }
    // Past the cap the checker refuses the timeline anyway.
    if let Some(Value::Array(tweens)) = root.get("timeline") {
        for (i, t) in tweens.iter().enumerate().take(SceneLimits::MAX_TWEENS) {
            top::tween(t, &at("/timeline", i), out);
        }
    }
}

/// Every key of `obj` is one of `allowed`.
fn known(obj: &Map<String, Value>, allowed: &[&str], path: &str, out: &mut Problems) {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            unknown(out, path, key, allowed);
        }
    }
}

/// The keys of `v`, when it is an object.
fn known_in(v: &Value, allowed: &[&str], path: &str, out: &mut Problems) {
    if let Value::Object(obj) = v {
        known(obj, allowed, path, out);
    }
}

fn unknown(out: &mut Problems, path: &str, key: &str, allowed: &[&str]) {
    if out.full() {
        out.skip();
        return;
    }
    let shown = quote(key);
    let place = if path.is_empty() {
        "the top of the scene"
    } else {
        path
    };
    let message = match keys::suggest(key, allowed) {
        Some(close) => format!("unknown property `{shown}` at {place}; did you mean `{close}`?"),
        None => format!(
            "unknown property `{shown}` at {place}; it takes {}",
            allowed.join(", ")
        ),
    };
    out.push(join(path, key), "scene.unknownProperty", message);
}

fn node(v: &Value, path: &str, depth: usize, entry: Entry, count: &mut usize, out: &mut Problems) {
    let Value::Object(obj) = v else { return };
    *count += 1;
    if *count > WALKED_NODES {
        if *count == WALKED_NODES + 1 {
            out.push(
                "/nodes",
                "scene.tooManyNodes",
                format!(
                    "A scene has at most {} nodes, counting children; this one has more than {WALKED_NODES}.",
                    SceneLimits::MAX_NODES
                ),
            );
        }
        return;
    }
    if depth > SceneLimits::MAX_DEPTH {
        out.push(
            path,
            "scene.tooDeep",
            format!(
                "Nodes nest at most {} deep; this one is at depth {depth}. Flatten its groups.",
                SceneLimits::MAX_DEPTH
            ),
        );
        return;
    }
    let kind = match obj.get("type") {
        Some(Value::String(name)) => match NodeType::from_name(name) {
            Some(kind) => kind,
            None if out.full() => {
                out.skip();
                return;
            }
            None => {
                let names: Vec<&str> = NodeType::ALL.iter().map(|t| t.name()).collect();
                let hint = keys::suggest(name, &names).map_or_else(
                    || format!("types are {}", names.join(", ")),
                    |n| format!("did you mean `{n}`?"),
                );
                out.push(
                    join(path, "type"),
                    "scene.nodeType",
                    format!("unknown node type `{}` at {path}; {hint}", quote(name)),
                );
                return;
            }
        },
        Some(_) => {
            out.push(
                join(path, "type"),
                "scene.nodeType",
                "A node's type is a string such as \"rect\".",
            );
            return;
        }
        None => {
            out.push(
                join(path, "type"),
                "scene.required",
                "A node needs a type: \"rect\", \"text\", …",
            );
            return;
        }
    };
    if kind == NodeType::Svg && entry == Entry::Stored {
        out.push(
            path,
            "scene.svgStored",
            "A stored scene can't hold svg nodes: Klipolo turns them into path nodes when the scene is made.",
        );
    }
    for (key, val) in obj {
        let key_path = join(path, key);
        if !keys::accepts(kind, key) {
            if out.full() {
                out.skip();
            } else {
                out.push(
                    key_path,
                    "scene.unknownProperty",
                    keys::why_not(kind, key, path),
                );
            }
            continue;
        }
        match key.as_str() {
            "file" => out.push(
                key_path,
                "scene.imageFile",
                "An image in a scene comes from an image setting (\"asset\": \"$logo\") or is an emoji; image files \
                 aren't supported yet.",
            ),
            "children" => {
                if let Value::Array(items) = val {
                    for (i, child) in items.iter().enumerate() {
                        node(child, &at(&key_path, i), depth + 1, entry, count, out);
                    }
                }
            }
            "child" => node(val, &key_path, depth + 1, entry, count, out),
            "animate" => {
                if let Value::Object(props) = val {
                    for (prop, list) in props {
                        let prop_path = join(&key_path, prop);
                        if animates(kind, prop, &key_path, out).is_some() {
                            if let Value::Array(list) = list {
                                for (i, k) in list.iter().enumerate() {
                                    known_in(k, nested::KEY, &at(&prop_path, i), out);
                                }
                            }
                        }
                    }
                }
            }
            "modifiers" => {
                if let Value::Object(props) = val {
                    for (prop, m) in props {
                        let prop_path = join(&key_path, prop);
                        match animates(kind, prop, &key_path, out) {
                            Some(ValueKind::Number | ValueKind::Position) => {}
                            Some(_) => out.push(
                                prop_path.clone(),
                                "scene.animatable",
                                format!("Modifiers move numbers; `{}` isn't one.", quote(prop)),
                            ),
                            None => continue,
                        }
                        let allowed = m.get("type").and_then(Value::as_str).and_then(nested::modifier);
                        if let Some(allowed) = allowed {
                            known_in(m, allowed, &prop_path, out);
                        }
                    }
                }
            }
            "fill" | "stroke" => paint(val, &key_path, out),
            "font" => {
                known_in(val, nested::FONT, &key_path, out);
                if let Some(source) = val.get("source") {
                    known_in(source, nested::FONT_SOURCE, &join(&key_path, "source"), out);
                }
            }
            "trim" => known_in(val, nested::TRIM, &key_path, out),
            "shadow" => known_in(val, nested::SHADOW, &key_path, out),
            "mask" => known_in(val, nested::MASK, &key_path, out),
            "motionPath" => known_in(val, nested::MOTION_PATH, &key_path, out),
            "outline" => known_in(val, nested::OUTLINE, &key_path, out),
            "count" => known_in(val, nested::COUNT, &key_path, out),
            "reveal" => known_in(val, nested::REVEAL, &key_path, out),
            "scramble" => known_in(val, nested::SCRAMBLE, &key_path, out),
            "cone" => known_in(val, nested::CONE, &key_path, out),
            "stagger" => known_in(val, nested::STAGGER, &key_path, out),
            "spans" | "holds" => {
                if let Value::Array(items) = val {
                    for (i, item) in items.iter().enumerate() {
                        let item_path = at(&key_path, i);
                        if key == "holds" {
                            known_in(item, nested::HOLD, &item_path, out);
                            continue;
                        }
                        known_in(item, nested::SPAN, &item_path, out);
                        if let Some(fill) = item.get("fill") {
                            paint(fill, &join(&item_path, "fill"), out);
                        }
                        if let Some(bg) = item.get("background") {
                            known_in(bg, nested::SPAN_BACKGROUND, &join(&item_path, "background"), out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// What `prop`'s values are when it animates on a node of this type; otherwise a problem, and None.
fn animates(kind: NodeType, prop: &str, path: &str, out: &mut Problems) -> Option<ValueKind> {
    if let Some(values) = keys::animatable_on(kind, prop) {
        return Some(values);
    }
    let prop_path = join(path, prop);
    if keys::animatable(prop).is_some() {
        out.push(
            prop_path,
            "scene.animatable",
            format!(
                "`{}` doesn't animate on a {} node (at {path}).",
                quote(prop),
                kind.name()
            ),
        );
    } else {
        let names: Vec<&str> = keys::animatable_names(kind).collect();
        unknown(out, path, prop, &names);
    }
    None
}

fn paint(v: &Value, path: &str, out: &mut Problems) {
    if let Value::Object(obj) = v {
        let allowed = if obj.contains_key("generator") {
            nested::GENERATOR
        } else {
            nested::GRADIENT
        };
        known(obj, allowed, path, out);
    }
}
