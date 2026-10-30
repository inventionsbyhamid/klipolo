//! A node's own keys (`animate`): a property its type animates, 1 to 64 keys in time order inside the scene, each
//! value of the property's kind and each ease a name Klipolo reads. Tweens' values are checked the same way.

use std::collections::BTreeMap;

use super::super::keys::{self, ValueKind};
use super::super::problems::{at, join, quote, Problems};
use super::super::units::{parse_clip, parse_origin, MAX_PX};
use super::super::{SceneKey, SceneLimits, Unit, Val};
use super::ranges::limits;
use super::values::{range_val, val, Want};
use super::{Flat, Repeat, Scope};
use crate::ease_names;

pub(super) fn node_keys(s: &Scope, out: &mut Problems, f: &Flat) {
    let animate = join(&f.path, "animate");
    for (prop, list) in &f.node.animate {
        let prop_path = join(&animate, prop);
        let Some(kind) = keys::animatable_on(f.node.kind, prop) else {
            out.push(
                prop_path,
                "scene.animatable",
                format!(
                    "`{}` doesn't animate on a {} node.",
                    quote(prop),
                    f.node.kind.name()
                ),
            );
            continue;
        };
        if list.is_empty() || list.len() > SceneLimits::MAX_KEYS {
            out.push(
                prop_path.clone(),
                "scene.tooManyKeys",
                format!(
                    "A property has 1 to {} keys; this one has {}.",
                    SceneLimits::MAX_KEYS,
                    list.len()
                ),
            );
        }
        if matches!(prop.as_str(), "fill.offset" | "fill.angle")
            && !matches!(f.node.props.fill, Some(super::super::Paint::Gradient(_)))
        {
            out.push(
                prop_path.clone(),
                "scene.animatable",
                "Only a gradient fill has an offset and an angle.",
            );
        }
        let mut last = f64::NEG_INFINITY;
        for (i, key) in list.iter().enumerate() {
            let key_path = at(&prop_path, i);
            scene_key(
                s,
                out,
                key,
                (prop.as_str(), kind),
                &key_path,
                f.repeat,
                &mut last,
            );
        }
    }
}

fn scene_key(
    s: &Scope,
    out: &mut Problems,
    key: &SceneKey,
    (prop, kind): (&str, ValueKind),
    path: &str,
    rep: Option<Repeat>,
    last: &mut f64,
) {
    if !(key.t.is_finite() && (0.0..=s.scene.duration).contains(&key.t)) {
        out.push(
            join(path, "t"),
            "scene.keys",
            format!(
                "A key's t is a time inside the scene, 0 to {} s.",
                s.scene.duration
            ),
        );
    } else if key.t < *last {
        out.push(join(path, "t"), "scene.keys", "Keys are in time order.");
    } else {
        *last = key.t;
    }
    if key.unit == Some(Unit::Px) && kind != ValueKind::Position {
        out.push(
            join(path, "unit"),
            "scene.value",
            "\"unit\": \"px\" is for x and y.",
        );
    }
    value(
        s,
        out,
        &key.v,
        (prop, kind),
        &join(path, "v"),
        rep,
        key.unit == Some(Unit::Px),
    );
    if let Some(e) = &key.ease {
        if let Err(why) = ease_names::read(e) {
            out.push(join(path, "ease"), "scene.ease", why);
        }
    }
}

/// An animated value of `prop`, of its kind and in the same range as a static one (`ranges`): a position in box
/// fractions within ±10, or in pixels within ±`MAX_PX`.
pub(super) fn value(
    s: &Scope,
    out: &mut Problems,
    v: &Val,
    (prop, kind): (&str, ValueKind),
    path: &str,
    rep: Option<Repeat>,
    px: bool,
) {
    match (kind, v) {
        (ValueKind::Clip, Val::Str(c)) if parse_clip(c).is_some() => {}
        (ValueKind::Origin, Val::Str(o)) if parse_origin(o).is_some() => {}
        (ValueKind::Clip, _) => out.push(
            path,
            "scene.clip",
            "A clip is inset(t r b l round r) or circle(r at x y).",
        ),
        (ValueKind::Origin, _) => out.push(
            path,
            "scene.value",
            "An origin is \"50% 0%\", \"center\" and the like.",
        ),
        _ => {
            let n = val(s, out, v, Want::of(kind), path, rep);
            let (lo, hi) = if kind == ValueKind::Position && px {
                (-MAX_PX, MAX_PX)
            } else {
                limits(prop)
            };
            range_val(s, out, path, v, n, (lo, hi));
        }
    }
}

/// A tween's property values: each a property every target animates, with a value of its kind.
pub(super) fn vars(
    s: &Scope,
    out: &mut Problems,
    vars: &BTreeMap<String, Val>,
    targets: &[usize],
    path: &str,
) {
    for (name, v) in vars {
        let var_path = join(path, name);
        let Some((_, kind)) = keys::animatable(name) else {
            out.push(
                var_path,
                "scene.unknownProperty",
                format!("`{}` isn't a property that animates.", quote(name)),
            );
            continue;
        };
        let refused = targets
            .iter()
            .filter_map(|&i| s.flat.get(i))
            .find(|f| keys::animatable_on(f.node.kind, name).is_none());
        if let Some(f) = refused {
            out.push(
                var_path.clone(),
                "scene.animatable",
                format!(
                    "`{}` doesn't animate on the {} node at {}.",
                    quote(name),
                    f.node.kind.name(),
                    f.path
                ),
            );
        }
        let px = matches!(v, Val::Str(t) if t.trim_end().ends_with("px"));
        value(s, out, v, (name.as_str(), kind), &var_path, None, px);
    }
}
