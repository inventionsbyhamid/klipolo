//! Fills and strokes: a colour, a gradient (one shape, 2–16 stops in order) or a generator the catalog knows, whose
//! settings are values its definition takes or bindings to scene settings of the same kind.

use std::sync::OnceLock;

use super::super::problems::{at, join, quote, Problems};
use super::super::units::MAX_FRACTION;
use super::super::{Paint, SceneFacts};
use super::ranges::limits;
use super::values::{bind, binding, num_field, param_range, text, Want};
use super::{Repeat, Scope};
use crate::contrib::ContribKind;
use crate::params::{ParamKind, ParamSpec, ParamValue};
use crate::project::Project;

/// A fill or stroke: a colour, a gradient or a generator the catalog knows, with its settings.
pub(super) fn paint(
    s: &Scope,
    out: &mut Problems,
    facts: &mut SceneFacts,
    p: &Paint,
    path: &str,
    repeat: Option<Repeat>,
) {
    match p {
        Paint::Color(c) => {
            text(s, out, c, Want::Color, path, repeat);
        }
        Paint::Gradient(g) => {
            // Points within ±10 boxes, as positions are; a radius above 0 and at most 10 boxes.
            let inside = |v: &f64| v.abs() <= MAX_FRACTION;
            let shape_ok = match (&g.linear, &g.radial) {
                (Some(l), None) => l.iter().all(inside),
                (None, Some([x, y, r])) => inside(x) && inside(y) && *r > 0.0 && *r <= MAX_FRACTION,
                _ => false,
            };
            if !shape_ok {
                out.push(
                    path,
                    "scene.value",
                    "A gradient has exactly one of \"linear\": [x0, y0, x1, y1] or \"radial\": [cx, cy, r], fractions \
                     of the box from -10 to 10 (r above 0).",
                );
            }
            let stops = join(path, "stops");
            if !(2..=16).contains(&g.stops.len()) {
                out.push(
                    stops.clone(),
                    "scene.value",
                    "A gradient has 2 to 16 stops: [[offset, colour], …].",
                );
            }
            let mut last = 0.0;
            for (i, (offset, color)) in g.stops.iter().enumerate() {
                let stop = at(&stops, i);
                if !(offset.is_finite() && (last..=1.0).contains(offset)) {
                    out.push(
                        at(&stop, 0),
                        "scene.value",
                        "Stop offsets go from 0 to 1, in order.",
                    );
                } else {
                    last = *offset;
                }
                text(s, out, color, Want::Color, &at(&stop, 1), repeat);
            }
            num_field(
                s,
                out,
                &g.angle,
                Want::Number,
                path,
                "angle",
                repeat,
                limits("fill.angle"),
            );
            num_field(
                s,
                out,
                &g.offset,
                Want::Number,
                path,
                "offset",
                repeat,
                limits("fill.offset"),
            );
        }
        Paint::Generator(g) => {
            let def = s
                .catalog
                .get(&g.generator)
                .filter(|d| d.kind == ContribKind::Generator);
            let Some(def) = def else {
                out.push(
                    join(path, "generator"),
                    "scene.unknownGenerator",
                    format!(
                        "`{}` isn't a generator Klipolo knows; a fill names an installed or library generator.",
                        quote(g.generator.as_str())
                    ),
                );
                return;
            };
            if !facts.generator_fills.contains(&g.generator) {
                facts.generator_fills.push(g.generator.clone());
            }
            let params = join(path, "params");
            for (key, value) in &g.params {
                let value_path = join(&params, key);
                match def.param(key) {
                    None => out.push(
                        value_path,
                        "scene.generator",
                        format!("{} has no setting `{}`.", g.generator, quote(key)),
                    ),
                    Some(spec) => generator_value(s, out, spec, value, &value_path, repeat),
                }
            }
        }
    }
}

/// A generator's setting inside a fill: a binding to a scene setting of the same kind, or a value its definition
/// takes.
fn generator_value(
    s: &Scope,
    out: &mut Problems,
    spec: &ParamSpec,
    value: &ParamValue,
    path: &str,
    repeat: Option<Repeat>,
) {
    if let ParamValue::Text(t) = value {
        if let Some(b) = binding(t) {
            let want = match spec.kind {
                ParamKind::Float { .. } | ParamKind::Int { .. } | ParamKind::Angle { .. } => {
                    Want::Number
                }
                ParamKind::Color { .. } => Want::Color,
                ParamKind::Text { .. } | ParamKind::Choice { .. } => Want::Text,
                ParamKind::Bool { .. } => Want::Flag,
                ParamKind::Font { .. } => Want::Font,
                ParamKind::Asset { .. } => Want::Asset,
                ParamKind::Table { .. } => Want::Table,
                ParamKind::Point { .. } => {
                    out.push(
                        path,
                        "scene.bindingKind",
                        "A point setting can't be bound in a scene.",
                    );
                    return;
                }
            };
            bind(s, out, b, want, path, repeat);
            // A bound number setting stays inside the generator's own range (rows are clamped when compiled).
            let own = match spec.kind {
                ParamKind::Float { min, max, .. } => Some((min, max)),
                ParamKind::Int { min, max, .. } => Some((min as f64, max as f64)),
                ParamKind::Angle { .. } => Some((-360.0, 360.0)),
                _ => None,
            };
            if let (Some((lo, hi)), Some((from, to))) = (own, param_range(s, t)) {
                if from < lo || to > hi {
                    out.push(
                        path,
                        "scene.generator",
                        format!(
                            "`{}` goes from {from} to {to}, past the generator setting \"{}\"'s {lo} to {hi}.",
                            quote(t),
                            quote(&spec.label)
                        ),
                    );
                }
            }
            return;
        }
    }
    static EMPTY: OnceLock<Project> = OnceLock::new();
    let project = EMPTY.get_or_init(|| Project::new(""));
    if matches!(spec.kind, ParamKind::Asset { .. }) || spec.check(value, project).is_err() {
        out.push(
            path,
            "scene.generator",
            format!(
                "This value doesn't fit the generator's setting \"{}\".",
                quote(&spec.label)
            ),
        );
    }
}

/// The different gradients `scene` draws, each once however often it is drawn: its nodes' fills and strokes and its
/// spans' fills. A gradient's colours can only come from scene settings, so every repeat instance draws the same one.
pub(super) fn gradients(scene: &super::super::Scene) -> usize {
    fn walk<'a>(nodes: &'a [super::super::Node], seen: &mut Vec<&'a super::super::Gradient>) {
        for n in nodes {
            let p = &n.props;
            let spans = p.spans.iter().flatten().filter_map(|s| s.fill.as_ref());
            for paint in [&p.fill, &p.stroke].into_iter().flatten().chain(spans) {
                if let Paint::Gradient(g) = paint {
                    if !seen.contains(&g) {
                        seen.push(g);
                    }
                }
            }
            walk(&n.children, seen);
            if let Some(child) = &p.child {
                walk(std::slice::from_ref(child.as_ref()), seen);
            }
        }
    }
    let mut seen = Vec::new();
    walk(&scene.nodes, &mut seen);
    seen.len()
}
