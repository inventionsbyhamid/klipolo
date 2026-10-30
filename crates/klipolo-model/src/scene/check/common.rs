//! The properties every node may have, checked whatever its type: only those its type takes, its box and
//! transforms, clip, paints, strokes, radius, shadow, mask, motion path and modifiers.

use super::super::keys::{self, ValueKind};
use super::super::path::parse_d;
use super::super::problems::{at, join, quote, Problems};
use super::super::units::{parse_clip, parse_origin, MAX_PX};
use super::super::{Flag, Modifier, Node, NodeType, Origin, Radius, SceneFacts, Val};
use super::paint::paint;
use super::ranges::{limits, range_of};
use super::values::{bind, binding, num_field, range_val, text, val, Want};
use super::{Flat, Repeat, Scope};

pub(super) fn props(s: &Scope, out: &mut Problems, facts: &mut SceneFacts, f: &Flat) {
    let (n, path, rep) = (f.node, f.path.as_str(), f.repeat);
    let p = &n.props;
    fits_type(n, path, out);
    if let Some(bounds) = &n.bounds {
        let box_path = join(path, "box");
        for (i, v) in bounds.iter().enumerate() {
            let value = val(s, out, v, Want::Position, &at(&box_path, i), rep);
            let (lo, hi) = if i < 2 { (-10.0, 10.0) } else { (0.0, 10.0) };
            range_val(s, out, &at(&box_path, i), v, value, (lo, hi));
        }
    }
    num_field(s, out, &p.x, Want::Position, path, "x", rep, limits("x"));
    num_field(s, out, &p.y, Want::Position, path, "y", rep, limits("y"));
    if let Some(o) = &p.origin {
        origin(s, out, o, &join(path, "origin"), rep);
    }
    for (v, key) in [
        (&p.scale, "scale"),
        (&p.scale_x, "scaleX"),
        (&p.scale_y, "scaleY"),
        (&p.rotate, "rotate"),
        (&p.rotate_x, "rotateX"),
        (&p.rotate_y, "rotateY"),
        (&p.perspective, "perspective"),
        (&p.opacity, "opacity"),
        (&p.blur, "blur"),
        (&p.stroke_width, "strokeWidth"),
        (&p.dash_offset, "dashOffset"),
        (&p.inner_radius, "innerRadius"),
        (&p.size, "size"),
        (&p.weight, "weight"),
        (&p.line_height, "lineHeight"),
        (&p.letter_spacing, "letterSpacing"),
    ] {
        num_field(s, out, v, Want::Number, path, key, rep, limits(key));
    }
    num_field(
        s,
        out,
        &p.wrap,
        Want::Position,
        path,
        "wrap",
        rep,
        limits("wrap"),
    );
    flag(s, out, &p.visible, &join(path, "visible"), rep);
    flag(s, out, &p.italic, &join(path, "italic"), rep);
    if let Some(clip) = &p.clip {
        if parse_clip(clip).is_none() {
            out.push(
                join(path, "clip"),
                "scene.clip",
                format!(
                    "\"{}\" isn't a clip: write inset(t r b l round r) or circle(r at x y).",
                    quote(clip)
                ),
            );
        }
    }
    for (paint_value, key) in [(&p.fill, "fill"), (&p.stroke, "stroke")] {
        if let Some(pv) = paint_value {
            paint(s, out, facts, pv, &join(path, key), rep);
        }
    }
    // At most 4 lengths: the path shader's dash takes 4.
    if let Some(dash) = &p.dash {
        if !(1..=4).contains(&dash.len())
            || !dash.iter().all(|d| (0.0..=MAX_PX).contains(d))
            || dash.iter().sum::<f64>() <= 0.0
        {
            out.push(
                join(path, "dash"),
                "scene.value",
                format!("A dash is 1 to 4 lengths of 0 to {MAX_PX} pixels, not all 0."),
            );
        }
    }
    if let Some(trim) = &p.trim {
        let trim_path = join(path, "trim");
        for (v, key, name) in [
            (&trim.start, "start", "trim.start"),
            (&trim.end, "end", "trim.end"),
            (&trim.offset, "offset", "trim.offset"),
        ] {
            num_field(s, out, v, Want::Number, &trim_path, key, rep, limits(name));
        }
    }
    if let Some(radius) = &p.radius {
        let radius_path = join(path, "radius");
        let corners: &[Val] = match radius {
            Radius::All(v) => std::slice::from_ref(v),
            Radius::Corners(c) => c,
        };
        for (i, v) in corners.iter().enumerate() {
            let at_path = if corners.len() == 1 {
                radius_path.clone()
            } else {
                at(&radius_path, i)
            };
            let value = val(s, out, v, Want::Number, &at_path, rep);
            range_val(s, out, &at_path, v, value, limits("radius"));
        }
    }
    if let Some(shadow) = &p.shadow {
        let shadow_path = join(path, "shadow");
        for (v, key, name) in [
            (&shadow.x, "x", "shadow.x"),
            (&shadow.y, "y", "shadow.y"),
            (&shadow.blur, "blur", "shadow.blur"),
            (&shadow.spread, "spread", "shadow.spread"),
        ] {
            num_field(
                s,
                out,
                v,
                Want::Number,
                &shadow_path,
                key,
                rep,
                limits(name),
            );
        }
        if let Some(c) = &shadow.color {
            text(s, out, c, Want::Color, &join(&shadow_path, "color"), rep);
        }
    }
    if let Some(mask) = &p.mask {
        let target = mask.node.strip_prefix('#').and_then(|id| s.by_id(id));
        if target.is_none_or(|t| std::ptr::eq(t.node, n)) {
            out.push(
                join(&join(path, "mask"), "node"),
                "scene.value",
                format!(
                    "A mask names another node by \"#id\"; \"{}\" doesn't.",
                    quote(&mask.node)
                ),
            );
        }
    }
    if let Some(mp) = &p.motion_path {
        let mp_path = join(path, "motionPath");
        motion_path(s, out, facts, &mp.path, &join(&mp_path, "path"));
        num_field(
            s,
            out,
            &mp.progress,
            Want::Number,
            &mp_path,
            "progress",
            rep,
            limits("motionPath.progress"),
        );
    }
    if let Some(mods) = &p.modifiers {
        modifiers(out, n.kind, mods, &join(path, "modifiers"));
    }
}

/// Only properties this type takes (a scene built in code skips the loader's walk); `children` only on groups and
/// repeats.
fn fits_type(n: &Node, path: &str, out: &mut Problems) {
    if let Ok(serde_json::Value::Object(props)) = serde_json::to_value(&n.props) {
        for key in props.keys() {
            if !keys::accepts(n.kind, key) {
                out.push(
                    join(path, key),
                    "scene.unknownProperty",
                    keys::why_not(n.kind, key, path),
                );
            }
        }
    }
    if !n.children.is_empty() && !keys::accepts(n.kind, "children") {
        out.push(
            join(path, "children"),
            "scene.unknownProperty",
            keys::why_not(n.kind, "children", path),
        );
    }
}

pub(super) fn flag(
    s: &Scope,
    out: &mut Problems,
    v: &Option<Flag>,
    path: &str,
    rep: Option<Repeat>,
) {
    if let Some(Flag::Bind(b)) = v {
        match binding(b) {
            Some(b) => {
                bind(s, out, b, Want::Flag, path, rep);
            }
            None => out.push(
                path,
                "scene.value",
                "Write true, false or \"$key\" (an on/off setting).",
            ),
        }
    }
}

fn origin(s: &Scope, out: &mut Problems, o: &Origin, path: &str, rep: Option<Repeat>) {
    match o {
        Origin::Pair(pair) => {
            for (i, v) in pair.iter().enumerate() {
                let value = val(s, out, v, Want::Position, &at(path, i), rep);
                range_val(s, out, &at(path, i), v, value, limits("x"));
            }
        }
        Origin::Css(css) => {
            if parse_origin(css).is_none() {
                out.push(
                    path,
                    "scene.value",
                    format!(
                        "\"{}\" isn't an origin: write \"50% 0%\", \"center\" or [x, y].",
                        quote(css)
                    ),
                );
            }
        }
    }
}

fn motion_path(s: &Scope, out: &mut Problems, facts: &mut SceneFacts, d: &str, path: &str) {
    if let Some(id) = d.strip_prefix('#') {
        if s.by_id(id).is_none_or(|f| f.node.kind != NodeType::Path) {
            out.push(
                path,
                "scene.value",
                format!("\"#{}\" isn't a path node's id.", quote(id)),
            );
        }
        return;
    }
    match parse_d(d) {
        Ok(cmds) => facts.path_commands = facts.path_commands.saturating_add(cmds.len()),
        Err(why) => out.push(path, "scene.path", why),
    }
}

fn modifiers(
    out: &mut Problems,
    kind: NodeType,
    mods: &std::collections::BTreeMap<String, Modifier>,
    path: &str,
) {
    for (prop, m) in mods {
        let prop_path = join(path, prop);
        if !matches!(
            keys::animatable_on(kind, prop),
            Some(ValueKind::Number | ValueKind::Position)
        ) {
            out.push(
                prop_path.clone(),
                "scene.animatable",
                format!(
                    "A modifier moves a number this node animates; `{}` isn't one.",
                    quote(prop)
                ),
            );
        }
        // An amplitude moves the value at most across its property's whole range (opacity 1, blur 256), so the
        // property can't be pushed far outside it; the evaluator clamps the moved value into the range.
        let reach = match range_of(prop) {
            Some((lo, hi)) => hi - lo,
            None => 1e4,
        };
        let ok = match *m {
            Modifier::Wiggle { freq, amp, .. } => freq > 0.0 && freq <= 60.0 && amp.abs() <= reach,
            Modifier::Oscillate {
                freq,
                amp,
                phase,
                decay,
            } => {
                freq > 0.0
                    && freq <= 60.0
                    && amp.abs() <= reach
                    && phase.is_none_or(|ph| ph.abs() <= 1e4)
                    && decay.is_none_or(|d| (0.0..=1e4).contains(&d))
            }
            Modifier::Marquee { speed } => speed.abs() <= 1e5,
        };
        if !ok {
            out.push(
                prop_path,
                "scene.value",
                format!(
                    "A modifier's freq is above 0 and at most 60 a second, its amp at most {reach} for `{}`; phase \
                     and decay go up to 10000 (decay 0 or more), speed up to 100000.",
                    quote(prop)
                ),
            );
        }
    }
}
