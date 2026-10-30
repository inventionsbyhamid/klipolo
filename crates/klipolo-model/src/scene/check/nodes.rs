//! One node's checks: its common properties (`common`), what its type needs (`d`, `points`, `text`, `src`, `times` or
//! `rows`, `child`) and what it costs (particles, characters, split units).

use super::super::path::{parse_d, MAX_COORD};
use super::super::problems::{join, Problems};
use super::super::units::MAX_FRACTION;
use super::super::{Node, NodeType, Points, SceneFacts, SceneLimits};
use super::values::{binding, count, num_field, text, Binding, Want};
use super::{Flat, Repeat, Scope};
use crate::params::{ParamKind, MAX_TABLE_ROWS};

/// What a node adds to the scene's costs, before its instances are counted.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct NodeCost {
    pub particles: usize,
    /// Every character a text can show: its longest shown text, its holds and its animators.
    pub chars: usize,
    /// The longest text it shows in place of another: its text or spans, a text key, a tween's text value.
    pub shown: usize,
    /// A split text's units at the most its text can hold.
    pub units: usize,
}

/// How many instances a repeat makes at most, and what its children may bind.
pub(super) fn repeat_count(s: &Scope, node: &Node) -> (usize, Repeat) {
    if let Some(rows) = &node.props.rows {
        let table = match binding(rows) {
            Some(Binding::Param(key)) => s.params.get(key).map(|p| &p.kind),
            _ => None,
        };
        if let Some(ParamKind::Table {
            columns, max_rows, ..
        }) = table
        {
            return (
                (*max_rows).min(MAX_TABLE_ROWS) as usize,
                Repeat::Rows {
                    columns: columns.len(),
                },
            );
        }
        return (0, Repeat::Rows { columns: 0 });
    }
    // The same reading `kinds::repeat` checks; a refused count makes no instances (the scene is refused anyway). A
    // number setting's maximum is rounded up (0.9 makes 1): costs are upper bounds, and the evaluator draws `ceil` too.
    let times = node
        .props
        .times
        .as_ref()
        .and_then(|v| count(s, v).ok())
        .filter(|n| (0.0..=SceneLimits::MAX_INSTANCES as f64).contains(n))
        .map_or(0, |n| n.ceil() as usize);
    (times, Repeat::Times)
}

pub(super) fn check(s: &Scope, out: &mut Problems, facts: &mut SceneFacts, f: &Flat) -> NodeCost {
    let mut cost = NodeCost::default();
    let (n, path, rep) = (f.node, f.path.as_str(), f.repeat);
    let p = &n.props;
    super::common::props(s, out, facts, f);
    view_box(out, p.view_box, &join(path, "viewBox"));
    if let Some(mp) = &p.motion_path {
        view_box(
            out,
            mp.view_box,
            &join(&join(path, "motionPath"), "viewBox"),
        );
    }
    match n.kind {
        NodeType::Star | NodeType::Polygon => {
            let sides = num_field(
                s,
                out,
                &p.sides,
                Want::Number,
                path,
                "sides",
                rep,
                (3.0, 64.0),
            );
            if sides.is_some_and(|v| v.fract() != 0.0) {
                out.push(
                    join(path, "sides"),
                    "scene.value",
                    "Sides are a whole number.",
                );
            }
        }
        NodeType::Path => match &p.d {
            Some(d) => match parse_d(d) {
                Ok(cmds) => facts.path_commands = facts.path_commands.saturating_add(cmds.len()),
                Err(why) => out.push(join(path, "d"), "scene.path", why),
            },
            None => out.push(
                join(path, "d"),
                "scene.required",
                "A path node needs path data (\"d\").",
            ),
        },
        NodeType::Polyline => match &p.points {
            Some(Points::Bind(b)) => {
                let points_path = join(path, "points");
                text(s, out, b, Want::Table, &points_path, rep);
                let columns = match binding(b) {
                    Some(Binding::Param(key)) => match s.params.get(key).map(|p| &p.kind) {
                        Some(ParamKind::Table { columns, .. }) => columns.len(),
                        _ => 1,
                    },
                    _ => 1,
                };
                if columns < 1 {
                    out.push(
                        points_path,
                        "scene.bindingKind",
                        "A polyline's points come from a table setting with a number column: each row's first \
                         number is a value, drawn left to right and rising with it.",
                    );
                }
            }
            Some(Points::List(points)) => {
                let inside = points
                    .iter()
                    .all(|[x, y]| x.abs() <= MAX_FRACTION && y.abs() <= MAX_FRACTION);
                if !(2..=SceneLimits::MAX_PATH_COMMANDS).contains(&points.len()) || !inside {
                    out.push(
                        join(path, "points"),
                        "scene.value",
                        format!(
                            "A polyline has 2 to {} points, fractions of the box from -10 to 10.",
                            SceneLimits::MAX_PATH_COMMANDS
                        ),
                    );
                }
                facts.path_commands = facts.path_commands.saturating_add(points.len());
            }
            None => out.push(
                join(path, "points"),
                "scene.required",
                "A polyline needs points: \"$key\" (a table setting) or [[x, y], …].",
            ),
        },
        NodeType::Text => cost = super::text::text_node(s, out, facts, n, path, rep),
        NodeType::Image => super::kinds::image(s, out, n, path, rep),
        NodeType::Svg => {
            let ok = p.src.as_ref().is_some_and(|src| {
                (1..=64).contains(&src.chars().count()) && !src.chars().any(char::is_control)
            });
            if !ok {
                out.push(
                    join(path, "src"),
                    "scene.required",
                    "An svg node names an SVG of the request's svgs (\"src\": \"logo\").",
                );
            }
        }
        NodeType::Repeat => super::kinds::repeat(s, out, n, path, rep),
        NodeType::Emitter => cost.particles = super::kinds::emitter(s, out, n, path),
        NodeType::Backdrop => {
            if let Some(tint) = &p.tint {
                text(s, out, tint, Want::Color, &join(path, "tint"), rep);
            }
        }
        NodeType::Rect | NodeType::Ellipse | NodeType::Line | NodeType::Group => {}
    }
    cost
}

/// A `viewBox`: four finite numbers within the path data's own reach, its width and height above 0.
fn view_box(out: &mut Problems, v: Option<[f64; 4]>, path: &str) {
    let Some([x, y, w, h]) = v else { return };
    let reach = |v: f64| v.is_finite() && v.abs() <= MAX_COORD;
    if !(reach(x) && reach(y) && reach(w) && reach(h) && w > 0.0 && h > 0.0) {
        out.push(
            path,
            "scene.value",
            format!("A viewBox is [x, y, width, height]: finite (within {MAX_COORD:e}), the width and height above 0."),
        );
    }
}
