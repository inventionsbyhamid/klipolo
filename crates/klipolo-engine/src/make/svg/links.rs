//! The links in an SVG as usvg 0.48.1 follows them: a `<use>`'s target through an id
//! map that keeps the FIRST element of a duplicated id, its `href` before its `xlink:href` (whichever comes first in
//! the file), read as svgtypes reads an IRI; and a gradient's `href` chain, which usvg follows until it finds stops,
//! stopping only back at the gradient it started from or at one naming itself.

use std::collections::HashMap;

use klipolo_model::scene::SceneProblem;
use usvg::roxmltree::{Document, Node};

use super::budget::MAX_CHAIN;
use super::problem;

const XLINK: &str = "http://www.w3.org/1999/xlink";

/// Any `href` on `n`, the unprefixed one first.
pub(super) fn href_value<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    n.attributes()
        .find(|a| a.name() == "href" && a.namespace().is_none())
        .or_else(|| {
            n.attributes()
                .find(|a| a.name() == "href" && a.namespace() == Some(XLINK))
        })
        .map(|a| a.value())
}

/// The id `n`'s href names, as svgtypes reads an IRI (spaces, `#`, the id up to a space, then nothing but spaces):
/// None when it has none or names none.
pub(super) fn href_id<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    let rest = href_value(n)?
        .trim_start_matches([' ', '\t', '\n', '\r'])
        .strip_prefix('#')?;
    let (id, tail) = rest.split_at(rest.find(' ').unwrap_or(rest.len()));
    (!id.is_empty() && tail.trim_matches([' ', '\t', '\n', '\r']).is_empty()).then_some(id)
}

/// Every element by id, the first of a duplicated id kept (as usvg's `<use>` resolves them).
pub(super) fn ids<'a, 'input>(doc: &'a Document<'input>) -> HashMap<&'a str, Node<'a, 'input>> {
    let mut ids = HashMap::new();
    for n in doc.descendants() {
        if let Some(id) = n.attribute("id") {
            ids.entry(id).or_insert(n);
        }
    }
    ids
}

fn is_gradient(n: Node) -> bool {
    matches!(n.tag_name().name(), "linearGradient" | "radialGradient")
}

/// Gradients' `href` chains: one that comes back on itself anywhere (usvg would follow it forever when no gradient on
/// the loop has stops) or runs past [`MAX_CHAIN`] is refused. usvg resolves these through its own id map (the last of a
/// duplicated id among the elements it parses), so every gradient of an id is taken as where its name leads: a loop
/// either map could make is refused.
pub(super) fn gradient_chains(doc: &Document) -> Result<(), Vec<SceneProblem>> {
    let mut next: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut starts = Vec::new();
    for n in doc.descendants().filter(|n| is_gradient(*n)) {
        let Some(to) = href_id(n) else { continue };
        starts.push(to);
        if let Some(id) = n.attribute("id") {
            next.entry(id).or_default().push(to);
        }
    }
    // The longest chain from each id (None while it is being walked: met again, it is a loop).
    let mut depth: HashMap<&str, Option<usize>> = HashMap::new();
    for start in starts {
        chain(start, &next, &mut depth, 1)?;
    }
    Ok(())
}

fn chain<'a>(
    id: &'a str,
    next: &HashMap<&'a str, Vec<&'a str>>,
    depth: &mut HashMap<&'a str, Option<usize>>,
    hops: usize,
) -> Result<usize, Vec<SceneProblem>> {
    if hops > MAX_CHAIN {
        return Err(gradients_too_deep());
    }
    match depth.get(id) {
        Some(Some(d)) => return Ok(*d),
        Some(None) => {
            return Err(problem(
                "svg.reference",
                "A gradient in this SVG refers back to itself through other gradients, so it would never end. In \
                 your design tool, remove the loop (or expand the gradients), then import again.",
            ));
        }
        None => {}
    }
    depth.insert(id, None);
    let mut longest = 0;
    for to in next.get(id).into_iter().flatten() {
        longest = longest.max(1 + chain(to, next, depth, hops + 1)?);
    }
    if hops + longest > MAX_CHAIN {
        return Err(gradients_too_deep());
    }
    depth.insert(id, Some(longest));
    Ok(longest)
}

fn gradients_too_deep() -> Vec<SceneProblem> {
    problem(
        "svg.tooDeep",
        format!(
            "This SVG chains gradients more than {MAX_CHAIN} deep (each naming the next), more than Klipolo can \
             import. In your design tool, expand the gradients, then import again."
        ),
    )
}
