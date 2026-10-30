//! What usvg would do with an SVG, counted before it does it. usvg copies a `<use>`'s
//! target into its tree once per use and reads every copy again: its path data and its arcs, its attributes and those
//! it inherits, every style rule against it. A file under 1 MiB can ask for gigabytes, or for ever. This pass over
//! roxmltree's document fails closed, each refusal plain and saying what to do:
//! - a `<use>` resolves as usvg resolves it (`links.rs`): one that names nothing is ignored, as usvg ignores it; one
//!   that leads back to itself, or a chain more than [`MAX_CHAIN`] deep, is refused; so are gradient loops and chains;
//! - counted once per copy: elements (at most [`MAX_ELEMENTS`]), path data bytes ([`MAX_PATH_DATA`]), the segments usvg
//!   makes, an arc by its radius ([`MAX_SEGMENTS`]; one path at most 4,096; one arc at most 64 cubics: `segments.rs`),
//!   and the attribute bytes each element reads, its own and every ancestor's it may inherit ([`MAX_ATTRIBUTE_WORK`]);
//! - a percentage is of the largest viewport side in the file, while no viewport is sized past 100 %; once one is,
//!   every percentage length in the file is refused, for such sizes compound;
//! - `<style>`: at most [`MAX_STYLE_BYTES`] and [`MAX_STYLE_RULES`] selectors, and testing and writing its rules on
//!   every element (copies included) at most [`MAX_STYLE_WORK`] (`style.rs`);
//! - an `<image>` is refused, `svg.external` when it names anything but embedded data (no file is opened, ever).
//!
//! The counts are bounds from above, paired with each other and with the byte caps: none is usvg's exact work.

use std::collections::HashMap;

use klipolo_model::scene::{SceneLimits, SceneProblem};
use usvg::roxmltree::{Document, Node};

use super::links::{gradient_chains, href_id, href_value, ids};
use super::segments::{length, of_path, percent, quarter, viewport};
use super::style::Sheet;
use super::{MAX_ELEMENTS, external, picture, problem};

/// Path data bytes once every copy is counted.
pub const MAX_PATH_DATA: usize = 2 << 20;
/// Segments (lines and curves) once every copy is counted: well past what a stored scene's 192 KiB of path data holds.
pub const MAX_SEGMENTS: usize = 1 << 17;
/// Attribute bytes read once every copy and every inheriting element is counted.
pub const MAX_ATTRIBUTE_WORK: usize = 32 << 20;
/// `<use>` of a `<use>` of … (and a gradient naming a gradient …) at most this deep.
pub const MAX_CHAIN: usize = 32;
pub const MAX_STYLE_BYTES: usize = 64 << 10;
/// Selectors, a rule each.
pub const MAX_STYLE_RULES: usize = 512;
/// Selector tests and declaration bytes written, over every element.
pub const MAX_STYLE_WORK: u64 = 1 << 25;

/// What an element costs once copied: elements, path data bytes, segments, its attribute bytes (its elements' own,
/// and those each inherits from its ancestors within it), and the longest chain of `<use>` at or under it.
#[derive(Clone, Copy, Default)]
struct Cost {
    elements: usize,
    data: usize,
    segments: usize,
    own: usize,
    inherited: usize,
    chain: usize,
}

impl Cost {
    fn attributes(&self) -> usize {
        self.own.saturating_add(self.inherited)
    }

    fn over(&self) -> bool {
        self.elements > MAX_ELEMENTS
            || self.data > MAX_PATH_DATA
            || self.segments > MAX_SEGMENTS
            || self.attributes() > MAX_ATTRIBUTE_WORK
    }
}

fn too_large(what: String) -> Vec<SceneProblem> {
    problem(
        "svg.tooLarge",
        format!(
            "This SVG {what}, more than Klipolo can import. In your design tool, simplify it, then import again."
        ),
    )
}

fn radius() -> Vec<SceneProblem> {
    problem(
        "svg.tooLarge",
        "This SVG has an arc, circle, ellipse or rounded corner whose radius Klipolo can't bound (billions of units, or \
         in em or ex). In your design tool, scale it down or size it in pixels, then import again.",
    )
}

/// `named` (as the file writes it) is a percentage in a file that sizes a viewport past 100 %.
fn percentage(named: &str) -> Vec<SceneProblem> {
    problem(
        "svg.tooLarge",
        format!(
            "This SVG has a length in percent ({named}) and sizes an <svg>, <symbol> or <use> over 100 %, so Klipolo \
             can't tell how large it gets. In your design tool, size it in pixels or keep those sizes at 100 % or \
             less, then import again."
        ),
    )
}

/// `s` as a refusal shows it: printable ASCII, at most 48 characters of it.
fn shown(s: &str) -> String {
    let mut out: String = s
        .chars()
        .take(48)
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect();
    if s.chars().nth(48).is_some() {
        out.push('…');
    }
    out
}

/// Properties and attributes whose percentages aren't lengths: fractions (a stop's offset, the opacities) and names.
fn not_a_length(name: &str) -> bool {
    matches!(
        name,
        "offset"
            | "opacity"
            | "fill-opacity"
            | "stroke-opacity"
            | "stop-opacity"
            | "flood-opacity"
            | "id"
            | "class"
            | "href"
    )
}

/// The first declaration in CSS `text` (a style attribute or sheet) whose value is a percentage, from above: any
/// `name: value` between `;`, `{` and `}`.
fn percent_in_css(text: &str) -> Option<String> {
    text.split([';', '{', '}'])
        .filter_map(|d| d.split_once(':'))
        .map(|(name, value)| (name.trim(), value.trim()))
        .find(|(name, value)| !not_a_length(name) && percent(value))
        .map(|(name, value)| format!("{name}:{value}"))
}

/// The first percentage length in `doc` as it's written: an attribute, or a declaration in a `style` attribute or a
/// `<style>`. The viewport sizes themselves (an `<svg>`'s, a `<symbol>`'s or a `<use>`'s `width` and `height`) are
/// what [`viewport`] weighed, so they aren't counted again.
fn first_percentage(doc: &Document) -> Option<String> {
    for n in doc.descendants().filter(|n| n.is_element()) {
        let tag = n.tag_name().name();
        let sizes_a_viewport = matches!(tag, "svg" | "symbol" | "use");
        for a in n.attributes() {
            let found = match a.name() {
                "style" => percent_in_css(a.value()),
                "width" | "height" if sizes_a_viewport => None,
                name => (!not_a_length(name) && percent(a.value()))
                    .then(|| format!("{name}=\"{}\"", a.value())),
            };
            if found.is_some() {
                return found;
            }
        }
        if tag == "style" {
            let sheet = n.descendants().filter(|t| t.is_text());
            if let Some(found) = sheet.filter_map(|t| percent_in_css(t.text()?)).next() {
                return Some(found);
            }
        }
    }
    None
}

fn style_too_large() -> Vec<SceneProblem> {
    problem(
        "svg.tooLarge",
        "This SVG's style sheet is larger than Klipolo can import. In your design tool, export styles as presentation \
         attributes instead of CSS, then import again.",
    )
}

fn deep() -> Vec<SceneProblem> {
    problem(
        "svg.tooDeep",
        format!(
            "This SVG chains <use> more than {MAX_CHAIN} deep, more than Klipolo can import. In your design tool, \
             expand the <use> elements, then import again."
        ),
    )
}

/// The path data bytes and the segments usvg makes of shape `n` (a percentage of `view`, the largest viewport side).
fn shape(n: Node, view: f64) -> Result<(usize, usize), Vec<SceneProblem>> {
    let len = |a: &str| {
        n.attribute(a)
            .map_or(Ok(0.0), |v| length(v, view).ok_or_else(radius))
    };
    let round = |r: f64| quarter(r).map(|q| 4 * q as usize).ok_or_else(radius);
    Ok(match n.tag_name().name() {
        "path" => {
            let d = n.attribute("d").unwrap_or("");
            (d.len(), of_path(d).ok_or_else(radius)? as usize)
        }
        "polyline" | "polygon" => {
            let points = n.attribute("points").unwrap_or("");
            (points.len(), points.split([' ', ',']).count() / 2 + 2)
        }
        "circle" => (0, round(len("r")?)?),
        "ellipse" | "rect" => (0, 6 + round(len("rx")?.max(len("ry")?))?),
        "line" => (0, 2),
        _ => (0, 0),
    })
}

struct Walk<'a, 'input> {
    ids: HashMap<&'a str, Node<'a, 'input>>,
    /// The largest viewport side in the file, `<use>` sizes included: what a percentage is of, from above, in a file
    /// that sizes no viewport past 100 %.
    view: f64,
    memo: HashMap<usize, Cost>,
    /// The elements being counted (a cycle comes back to one), and the `<use>` hops on the way down to the current
    /// one (bounding the walk's own depth, whatever order the chain is written in).
    stack: Vec<usize>,
    hops: usize,
}

impl<'a, 'input> Walk<'a, 'input> {
    fn cost(&mut self, n: Node<'a, 'input>) -> Result<Cost, Vec<SceneProblem>> {
        let key = n.range().start;
        if let Some(&c) = self.memo.get(&key) {
            return Ok(c);
        }
        if self.stack.contains(&key) {
            return Err(problem(
                "svg.reference",
                "A <use> in this SVG leads back to itself, so it would never end. In your design tool, remove the \
                 loop (or expand the <use>), then import again.",
            ));
        }
        self.stack.push(key);
        let (data, segments) = shape(n, self.view)?;
        if segments > SceneLimits::MAX_PATH_COMMANDS {
            return Err(problem(
                "svg.pathTooLong",
                format!(
                    "A path in this SVG becomes more than {} curves and lines, more than Klipolo imports. In your \
                     design tool, simplify it, then import again.",
                    SceneLimits::MAX_PATH_COMMANDS
                ),
            ));
        }
        // Path data is read once, by its own element; every other attribute may be inherited by each descendant.
        let own: usize = n
            .attributes()
            .filter(|a| !matches!(a.name(), "d" | "points"))
            .map(|a| a.value().len())
            .sum();
        let mut total = Cost {
            elements: 1,
            data,
            segments,
            own,
            inherited: 0,
            chain: 0,
        };
        let mut add = |c: Cost, hop: usize| {
            total.elements = total.elements.saturating_add(c.elements);
            total.data = total.data.saturating_add(c.data);
            total.segments = total.segments.saturating_add(c.segments);
            total.own = total.own.saturating_add(c.own);
            let below = c.inherited.saturating_add(own.saturating_mul(c.elements));
            total.inherited = total.inherited.saturating_add(below);
            total.chain = total.chain.max(c.chain + hop);
            total.over()
        };
        for child in n.children().filter(|c| c.is_element()) {
            if add(self.cost(child)?, 0) {
                break;
            }
        }
        // A <use> naming nothing (or not as an IRI) is skipped by usvg: it costs only itself.
        let target = (n.tag_name().name() == "use")
            .then(|| href_id(n).and_then(|id| self.ids.get(id).copied()))
            .flatten();
        if let Some(target) = target {
            self.hops += 1;
            if self.hops > MAX_CHAIN {
                return Err(deep());
            }
            let c = self.cost(target)?;
            self.hops -= 1;
            add(c, 1);
        }
        if total.chain > MAX_CHAIN {
            return Err(deep());
        }
        self.stack.pop();
        self.memo.insert(key, total);
        Ok(total)
    }
}

/// `doc` within what usvg may be asked to do, or the plain reason it isn't.
pub(super) fn check(doc: &Document) -> Result<(), Vec<SceneProblem>> {
    let mut sheet = Sheet::default();
    let (mut deepest, mut view, mut grow) = (0usize, 100.0f64, 1.0f64);
    for n in doc.descendants().filter(|n| n.is_element()) {
        // Its ancestors: at most 64 (the scan), so at most 64 × 80,000 steps (roxmltree's node limit).
        deepest = deepest.max(n.ancestors().skip(1).filter(|a| a.is_element()).count());
        let (side, factor) = viewport(n);
        (view, grow) = (view.max(side), grow.max(factor));
        match n.tag_name().name() {
            "style" => {
                for t in n.descendants().filter(|t| t.is_text()) {
                    sheet.read(t.text().unwrap_or(""));
                }
            }
            "image" => {
                let embedded = href_value(n).is_some_and(|v| v.trim_start().starts_with("data:"));
                return Err(if embedded { picture() } else { external() });
            }
            _ => {}
        }
    }
    if sheet.bytes > MAX_STYLE_BYTES || sheet.selectors > MAX_STYLE_RULES {
        return Err(style_too_large());
    }
    gradient_chains(doc)?;
    // A viewport sized in percent is a share of the one around it, and usvg nests them (an <svg> in an <svg>, a <use>
    // sizing the <symbol> it shows) through every hop of a <use> chain: past 100 % they compound until its f32
    // viewport saturates (a 7 KB file's one rx="50%" made usvg 1.6 million segments). So once any viewport is sized
    // past 100 %, no percentage length in the file is bounded (fails closed).
    if grow > 1.0
        && let Some(named) = first_percentage(doc)
    {
        return Err(percentage(&shown(&named)));
    }
    let mut walk = Walk {
        ids: ids(doc),
        view,
        memo: HashMap::new(),
        stack: Vec::new(),
        hops: 0,
    };
    let total = walk.cost(doc.root_element())?;
    let copies = "once each <use> is counted as what it copies";
    if total.elements > MAX_ELEMENTS {
        return Err(too_large(format!(
            "becomes more than {MAX_ELEMENTS} elements {copies}"
        )));
    }
    if total.data > MAX_PATH_DATA {
        return Err(too_large(format!(
            "holds more than 2 MB of path data {copies}"
        )));
    }
    if total.segments > MAX_SEGMENTS {
        return Err(too_large(format!(
            "becomes more than {MAX_SEGMENTS} curves and lines {copies}"
        )));
    }
    if total.attributes() > MAX_ATTRIBUTE_WORK {
        return Err(too_large(format!(
            "reads more than 32 MB of attributes (styles, transforms, dashes and the like) {copies} and each \
             element that inherits them"
        )));
    }
    if sheet
        .per_element(deepest)
        .saturating_mul(total.elements as u64)
        > MAX_STYLE_WORK
    {
        return Err(style_too_large());
    }
    Ok(())
}
