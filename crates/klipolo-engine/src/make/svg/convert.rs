//! A parsed SVG's paths as scene `path` nodes: each visible path's data in the SVG's own units (its
//! transforms applied), its `viewBox` the SVG's size so every path fits its box together, its fill and stroke (colours
//! with their opacities and the groups' around them, gradients turned into the scene's: fractions of the viewBox), its
//! width, caps, joins, dashes and fill rule. Close enough rather than exact: a non-uniform transform's stroke takes its
//! mean scale, a radial gradient's focus and a gradient's spread (reflect, repeat) are dropped, and a group's opacity
//! multiplies its paths'. Numbers keep a ten-thousandth of the view; a view or coordinate past the scene's reach, or more
//! path data than a stored scene holds, is refused.

use std::fmt::Write;

use klipolo_model::scene::path::MAX_COORD;
use klipolo_model::scene::{
    FillRule, LineCap, LineJoin, Node, NodeType, Paint, Props, SceneLimits, SceneProblem, Val,
};
use usvg::tiny_skia_path::{PathSegment, Transform};

use super::paint::paint;
use super::scan::unsupported;
use super::{MAX_SVG_PATHS, picture, problem};

/// The most path data the paths of one SVG write: what a stored scene (256 KiB) holds, with room for the rest.
pub const MAX_PATH_TEXT: usize = 192 << 10;

/// Where the paths go: the view they share, the decimals their numbers keep (a ten-thousandth of the view), and the
/// path data written so far.
struct Out {
    view: [f64; 4],
    decimals: usize,
    bytes: usize,
    nodes: Vec<Node>,
}

fn too_large(what: &str) -> Vec<SceneProblem> {
    problem(
        "svg.tooLarge",
        format!(
            "This SVG's {what} reaches past what a motion graphic holds. In your design tool, scale it down or \
             simplify it, then import again."
        ),
    )
}

/// Every visible filled or stroked path of `tree`, as path nodes in paint order.
pub(super) fn paths(tree: &usvg::Tree) -> Result<Vec<Node>, Vec<SceneProblem>> {
    let size = tree.size();
    let (w, h) = (f64::from(size.width()), f64::from(size.height()));
    let within = |v: f64| v.is_finite() && v <= MAX_COORD;
    if !(within(w) && within(h)) {
        return Err(too_large("size"));
    }
    let decimals = (-(w.max(h) * 1e-4).log10()).ceil().clamp(0.0, 6.0) as usize;
    let mut out = Out {
        view: [0.0, 0.0, w, h],
        decimals,
        bytes: 0,
        nodes: Vec::new(),
    };
    group(tree.root(), 1.0, &mut out)?;
    if out.bytes > MAX_PATH_TEXT {
        return Err(too_large("path data"));
    }
    Ok(out.nodes)
}

/// `v` to `decimals` places, without trailing zeros.
fn number(d: &mut String, v: f32, decimals: usize) {
    let start = d.len();
    let _ = write!(d, " {:.*}", decimals, v);
    if d[start..].contains('.') {
        while d.ends_with('0') {
            d.pop();
        }
        if d.ends_with('.') {
            d.pop();
        }
    }
    if &d[start..] == " -0" {
        d.truncate(start);
        d.push_str(" 0");
    }
}

fn group(g: &usvg::Group, opacity: f32, out: &mut Out) -> Result<(), Vec<SceneProblem>> {
    // The scan refused <clipPath>, <mask> and <filter>: a clip here is usvg's own, round a nested <svg> or <symbol>'s
    // viewport, and a filter a function in a `filter` property (`blur(2)`).
    if g.clip_path().is_some() {
        return Err(problem(
            "svg.unsupported",
            "This SVG nests an <svg> or <symbol>, which clips what's inside it; Klipolo can't import that yet. In your \
             design tool, ungroup or flatten it, then import again.",
        ));
    }
    if g.mask().is_some() {
        return Err(unsupported("a mask", "flatten it (or release the mask)"));
    }
    if !g.filters().is_empty() {
        return Err(unsupported(
            "a filter (a blur, shadow or similar effect)",
            "remove the effect or flatten it",
        ));
    }
    let opacity = opacity * g.opacity().get();
    for child in g.children() {
        match child {
            usvg::Node::Group(g) => group(g, opacity, out)?,
            usvg::Node::Path(p) => {
                if let Some(node) = path(p, opacity, out)? {
                    if out.nodes.len() >= MAX_SVG_PATHS {
                        return Err(problem(
                            "svg.tooManyPaths",
                            format!(
                                "This SVG becomes more than {MAX_SVG_PATHS} paths, more than Klipolo imports. In your \
                                 design tool, merge or combine shapes, then import again."
                            ),
                        ));
                    }
                    out.nodes.push(node);
                }
            }
            usvg::Node::Image(_) => return Err(picture()),
            usvg::Node::Text(_) => {
                return Err(problem(
                    "svg.text",
                    "This SVG has text, which Klipolo can't import as part of a graphic. In your design tool, convert \
                     the text to outlines, then import again (or add the words as a text layer).",
                ));
            }
        }
    }
    Ok(())
}

/// `t` applied to `(x, y)`.
fn map(t: &Transform, x: f32, y: f32) -> (f32, f32) {
    (t.sx * x + t.kx * y + t.tx, t.ky * x + t.sy * y + t.ty)
}

/// A transform's mean scale (a length's, whichever way it runs).
fn mean_scale(t: &Transform) -> f32 {
    (t.sx * t.sy - t.kx * t.ky).abs().sqrt()
}

fn path(p: &usvg::Path, opacity: f32, out: &mut Out) -> Result<Option<Node>, Vec<SceneProblem>> {
    let view = out.view;
    if !p.is_visible() || (p.fill().is_none() && p.stroke().is_none()) {
        return Ok(None);
    }
    let t = p.abs_transform();
    let mut d = String::new();
    let mut commands = 0usize;
    for seg in p.data().segments() {
        commands += 1;
        if commands > SceneLimits::MAX_PATH_COMMANDS {
            return Err(problem(
                "svg.pathTooLong",
                format!(
                    "A path in this SVG becomes more than {} curves and lines, more than Klipolo imports. In your \
                     design tool, simplify it, then import again.",
                    SceneLimits::MAX_PATH_COMMANDS
                ),
            ));
        }
        let none = usvg::tiny_skia_path::Point { x: 0.0, y: 0.0 };
        let (verb, points, n) = match seg {
            PathSegment::MoveTo(a) => ("M", [a, none, none], 1),
            PathSegment::LineTo(a) => ("L", [a, none, none], 1),
            PathSegment::QuadTo(a, b) => ("Q", [a, b, none], 2),
            PathSegment::CubicTo(a, b, c) => ("C", [a, b, c], 3),
            PathSegment::Close => ("Z", [none; 3], 0),
        };
        d.push(' ');
        d.push_str(verb);
        for p in &points[..n] {
            let (x, y) = map(&t, p.x, p.y);
            if !(f64::from(x).abs() <= MAX_COORD && f64::from(y).abs() <= MAX_COORD) {
                return Err(too_large("path data"));
            }
            number(&mut d, x, out.decimals);
            number(&mut d, y, out.decimals);
        }
    }
    out.bytes += d.len();
    if out.bytes > MAX_PATH_TEXT {
        return Err(too_large("path data"));
    }
    if commands < 2 {
        return Ok(None);
    }
    let mut props = Props {
        d: Some(d.trim_start().to_owned()),
        view_box: Some(view),
        fill: Some(Paint::Color("none".into())),
        ..Props::default()
    };
    if let Some(f) = p.fill() {
        props.fill = paint(f.paint(), opacity * f.opacity().get(), &t, view)?;
        if f.rule() == usvg::FillRule::EvenOdd {
            props.fill_rule = Some(FillRule::EvenOdd);
        }
    }
    if let Some(s) = p.stroke() {
        let scale = mean_scale(&t);
        props.stroke = paint(s.paint(), opacity * s.opacity().get(), &t, view)?;
        props.stroke_width = Some(Val::Num(f64::from(s.width().get() * scale)));
        props.cap = Some(match s.linecap() {
            usvg::LineCap::Butt => LineCap::Butt,
            usvg::LineCap::Round => LineCap::Round,
            usvg::LineCap::Square => LineCap::Square,
        });
        props.join = Some(match s.linejoin() {
            usvg::LineJoin::Round => LineJoin::Round,
            usvg::LineJoin::Bevel => LineJoin::Bevel,
            _ => LineJoin::Miter,
        });
        if let Some(dash) = s.dasharray().filter(|a| !a.is_empty()) {
            // usvg has already written an odd list twice; the scene repeats it itself (and takes up to four lengths).
            let half = dash.len() / 2;
            let odd_twice = dash.len() % 4 == 2 && dash[..half] == dash[half..];
            let written = if odd_twice { &dash[..half] } else { dash };
            let kept: Vec<f64> = written
                .iter()
                .take(4)
                .map(|v| f64::from(v.max(0.0) * scale))
                .collect();
            if kept.iter().sum::<f64>() > 0.0 {
                props.dash = Some(kept);
                props.dash_offset = Some(Val::Num(f64::from(s.dashoffset() * scale)));
            }
        }
    }
    Ok(Some(Node {
        kind: NodeType::Path,
        id: None,
        class: None,
        bounds: None,
        props,
        animate: Default::default(),
        children: Vec::new(),
    }))
}
