//! An SVG flattened into scene `path` nodes, in the render worker only: its
//! `CheckScene` replaces each submitted `svg` node by these before the scene is stored, so no stored scene
//! holds SVG and the main process never reads one.
//!
//! The SVG is untrusted. In order, each refusal named for the agent and saying, in plain words, what to do (it names
//! the first thing it found):
//! - at most [`MAX_SVG_BYTES`] (`svg.tooLarge`), as text (no svgz);
//! - a byte scan before any parser: no text, flowed text included (`svg.text`: without fonts usvg would drop it
//!   silently), no entity declarations or DTD subset (`svg.entity`: nothing to expand), no `<script>`,
//!   `<foreignObject>`, patterns, clip paths, masks, filters or markers (`svg.unsupported`), at most [`MAX_DEPTH`]
//!   elements deep (`svg.tooDeep`);
//! - parsed with roxmltree's node limit, then usvg's work counted before it is done (`svg/budget.rs`): every `<use>`
//!   and gradient link followed as usvg follows it (one naming nothing is ignored, as usvg ignores it), counted as what
//!   it copies (elements, path data, segments with arcs by their radius, attribute bytes read), loops and long chains
//!   refused, `<style>` weighed by its selectors and combinators, images refused;
//! - usvg with no resources folder and an image resolver that refuses every `href` (defence in depth: nothing is
//!   opened, nothing fetched);
//! - only filled or stroked paths (shapes are paths to usvg), with colour or gradient paint; at most
//!   [`MAX_SVG_PATHS`] paths of at most 4,096 commands each, written to the view's precision, every coordinate and the
//!   view within the scene's reach, and all their path data within what a stored scene holds.

use std::sync::atomic::{AtomicBool, Ordering};

use klipolo_model::scene::{Node, SceneLimits, SceneProblem};

mod budget;
mod convert;
mod links;
mod paint;
mod scan;
mod segments;
mod style;

/// The largest SVG read.
pub const MAX_SVG_BYTES: usize = 1 << 20;
/// The most paths one SVG becomes.
pub const MAX_SVG_PATHS: usize = 200;
/// How deep its elements nest at most.
pub const MAX_DEPTH: usize = 64;
/// Its elements at most, every `<use>` counted as what it copies.
pub const MAX_ELEMENTS: usize = 20_000;

/// One problem, in the agent's English.
fn problem(code: &'static str, message: impl Into<String>) -> Vec<SceneProblem> {
    vec![SceneProblem {
        path: String::new(),
        code,
        message: message.into(),
    }]
}

/// `bytes` (an SVG's text) as `path` nodes filling the box their `viewBox` names (the SVG's size), or why not. The
/// problems' paths are empty: the caller names the `svg` node.
pub fn flatten(bytes: &[u8]) -> Result<Vec<Node>, Vec<SceneProblem>> {
    if bytes.len() > MAX_SVG_BYTES {
        return Err(problem(
            "svg.tooLarge",
            format!(
                "This SVG is {} KB; Klipolo imports SVGs of up to {} KB. In your design tool, simplify it or export it \
                 without hidden layers, then import again.",
                bytes.len().div_ceil(1024),
                MAX_SVG_BYTES / 1024
            ),
        ));
    }
    scan::scan(bytes)?;
    let text = std::str::from_utf8(bytes).map_err(|_| {
        problem(
            "svg.parse",
            "This SVG isn't plain UTF-8 text, so Klipolo can't read it. Save it again as a plain .svg from your design \
             tool, then import again.",
        )
    })?;
    let options = usvg::roxmltree::ParsingOptions {
        // The scan let no entity or internal subset through; a plain DOCTYPE line is allowed.
        allow_dtd: true,
        nodes_limit: 4 * MAX_ELEMENTS as u32,
        ..Default::default()
    };
    let doc = usvg::roxmltree::Document::parse_with_options(text, options).map_err(unreadable)?;
    budget::check(&doc)?;
    let (linked, embedded) = (AtomicBool::new(false), AtomicBool::new(false));
    let opt = usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| {
                embedded.store(true, Ordering::Relaxed);
                None
            }),
            resolve_string: Box::new(|_, _| {
                linked.store(true, Ordering::Relaxed);
                None
            }),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_xmltree(&doc, &opt).map_err(unreadable)?;
    if linked.load(Ordering::Relaxed) {
        return Err(external());
    }
    if embedded.load(Ordering::Relaxed) {
        return Err(picture());
    }
    let nodes = convert::paths(&tree)?;
    if nodes.is_empty() {
        return Err(problem(
            "svg.empty",
            "This SVG draws nothing Klipolo can import: no shape in it has a fill or a stroke. In your design tool, give \
             its shapes a fill or stroke, then import again.",
        ));
    }
    debug_assert!(nodes.len() <= MAX_SVG_PATHS && SceneLimits::MAX_PATH_COMMANDS == 4096);
    Ok(nodes)
}

/// Why the SVG doesn't parse.
fn unreadable(e: impl std::fmt::Display) -> Vec<SceneProblem> {
    problem(
        "svg.parse",
        format!(
            "This SVG doesn't read as SVG ({e}). Save it again as a plain .svg from your design tool, then import \
             again."
        ),
    )
}

/// An image embedded in the SVG.
fn picture() -> Vec<SceneProblem> {
    problem(
        "svg.image",
        "This SVG contains a picture, which Klipolo can't import as part of a graphic. Remove it in your design tool \
         (or add the picture as its own image layer), then import again.",
    )
}

/// Anything the SVG names outside itself.
fn external() -> Vec<SceneProblem> {
    problem(
        "svg.external",
        "This SVG links to a file or web address, which Klipolo never opens. Remove or embed it in your design tool, \
         then import again.",
    )
}
