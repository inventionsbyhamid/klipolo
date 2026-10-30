//! Images, repeats and emitters: what each needs, its settings in range, and the particles an emitter keeps alive.

use super::super::problems::{at, join, Problems};
use super::super::{Node, NodeType, SceneLimits, Val, DEFAULT_PARTICLE_LIFE};
use super::values::{binding, count, range, range_val, seconds, text, val, Want};
use super::{Repeat, Scope};

pub(super) fn image(s: &Scope, out: &mut Problems, n: &Node, path: &str, rep: Option<Repeat>) {
    let p = &n.props;
    match (&p.asset, &p.emoji) {
        (Some(asset), None) => {
            text(s, out, asset, Want::Asset, &join(path, "asset"), rep);
        }
        (None, Some(emoji)) => {
            let emoji_path = join(path, "emoji");
            if binding(emoji).is_some() {
                text(s, out, emoji, Want::Text, &emoji_path, rep);
            } else if !(1..=16).contains(&emoji.chars().count())
                || emoji.chars().any(char::is_control)
            {
                out.push(
                    emoji_path,
                    "scene.value",
                    "An emoji is 1 to 16 characters: \"🎉\".",
                );
            }
        }
        _ => out.push(
            join(path, "asset"),
            "scene.required",
            "An image node has an image setting (\"asset\": \"$key\") or an emoji, one of them.",
        ),
    }
    if p.file.is_some() {
        out.push(
            join(path, "file"),
            "scene.imageFile",
            "Image files aren't supported in scenes yet: use an image setting or an emoji.",
        );
    }
}

pub(super) fn repeat(s: &Scope, out: &mut Problems, n: &Node, path: &str, rep: Option<Repeat>) {
    let p = &n.props;
    match (&p.times, &p.rows) {
        (Some(_), None) => {
            count_field(
                s,
                out,
                &p.times,
                path,
                "times",
                (0.0, SceneLimits::MAX_INSTANCES as f64),
            );
        }
        (None, Some(rows)) => {
            text(s, out, rows, Want::Table, &join(path, "rows"), rep);
        }
        _ => out.push(
            join(path, "times"),
            "scene.required",
            "A repeat has times (a number) or rows (\"$key\", a table setting), one of them.",
        ),
    }
    if let Some(gap) = &p.gap {
        for (i, v) in gap.iter().enumerate() {
            let gap_path = at(&join(path, "gap"), i);
            let value = val(s, out, v, Want::Position, &gap_path, rep);
            range_val(s, out, &gap_path, v, value, (-10.0, 10.0));
        }
    }
    if n.children.is_empty() {
        out.push(
            join(path, "children"),
            "scene.required",
            "A repeat repeats its children: give it at least one.",
        );
    }
}

/// An emitter's settings; the particles it can have alive at once.
pub(super) fn emitter(s: &Scope, out: &mut Problems, n: &Node, path: &str) -> usize {
    let p = &n.props;
    match &p.child {
        None => out.push(
            join(path, "child"),
            "scene.required",
            "An emitter needs a child: the shape each particle draws.",
        ),
        Some(c) => {
            let shape = matches!(
                c.kind,
                NodeType::Rect
                    | NodeType::Ellipse
                    | NodeType::Star
                    | NodeType::Polygon
                    | NodeType::Path
                    | NodeType::Image
            );
            if !shape {
                out.push(
                    join(path, "child"),
                    "scene.value",
                    "A particle is a rect, ellipse, star, polygon, path or image (an emoji).",
                );
            }
        }
    }
    let burst = count_field(s, out, &p.burst, path, "burst", (0.0, 1e6)).unwrap_or(0.0);
    let rate = count_field(s, out, &p.rate, path, "rate", (0.0, 1e6)).unwrap_or(0.0);
    if p.burst.is_none() && p.rate.is_none() {
        out.push(
            join(path, "burst"),
            "scene.required",
            "An emitter needs a burst or a rate.",
        );
    }
    seconds(s, out, p.at, &join(path, "at"));
    if let Some(cone) = p.cone {
        let ok = cone
            .angle
            .is_none_or(|a| a.is_finite() && a.abs() <= 3600.0)
            && cone.spread.is_none_or(|v| (0.0..=360.0).contains(&v));
        if !ok {
            out.push(
                join(path, "cone"),
                "scene.value",
                "A cone's angle is finite and its spread 0 to 360 degrees.",
            );
        }
    }
    let ranges = [
        (p.speed, "speed", (0.0, 1e5)),
        (p.spin, "spin", (-1e5, 1e5)),
        (p.life, "life", (0.01, 60.0)),
    ];
    for (r, key, (lo, hi)) in ranges {
        if let Some(r) = r {
            if !r.values().iter().all(|v| (lo..=hi).contains(v)) {
                out.push(
                    join(path, key),
                    "scene.value",
                    format!("An emitter's {key} goes from {lo} to {hi}."),
                );
            }
        }
    }
    for (v, key, (lo, hi)) in [
        (p.gravity, "gravity", (-1e5, 1e5)),
        (p.drag, "drag", (0.0, 100.0)),
    ] {
        range(out, &join(path, key), v, lo, hi);
    }
    for (list, key, hi) in [
        (&p.size_over_life, "sizeOverLife", 100.0),
        (&p.opacity_over_life, "opacityOverLife", 1.0),
    ] {
        if let Some(list) = list {
            if list.is_empty() || list.len() > 8 || !list.iter().all(|v| (0.0..=hi).contains(v)) {
                out.push(
                    join(path, key),
                    "scene.value",
                    format!("{key} is 1 to 8 values from 0 to {hi}."),
                );
            }
        }
    }
    let life = p.life.map_or(DEFAULT_PARTICLE_LIFE, |l| l.max());
    let alive = burst + rate * life;
    if alive.is_finite() && alive > 0.0 {
        alive.ceil().min(1e9) as usize
    } else {
        0
    }
}

/// A count field (`times`, `burst`, `rate`): its largest value from `lo` to `hi` (a literal one whole), or None with
/// a problem ([`count`]).
fn count_field(
    s: &Scope,
    out: &mut Problems,
    v: &Option<Val>,
    path: &str,
    key: &str,
    (lo, hi): (f64, f64),
) -> Option<f64> {
    let v = v.as_ref()?;
    let path = join(path, key);
    match count(s, v) {
        Ok(n) if !(lo..=hi).contains(&n) => out.push(
            path,
            "scene.value",
            format!("{key} goes from {lo} to {hi}; this one reaches {n}."),
        ),
        Ok(n) if matches!(v, Val::Num(_)) && n.fract() != 0.0 => {
            out.push(path, "scene.value", format!("{key} is a whole number."))
        }
        Ok(n) => return Some(n),
        Err((code, why)) => out.push(path, code, why),
    }
    None
}
