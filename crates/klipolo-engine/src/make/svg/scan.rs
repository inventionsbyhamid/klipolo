//! The byte scan an SVG passes before any parser sees it: it walks the markup (skipping comments,
//! CDATA and processing instructions, honouring quotes inside tags) and refuses text elements (flowed text too),
//! entity declarations or a DTD's internal subset, scripts and foreign content, patterns, clip paths, masks, filters,
//! markers (usvg converts a marker's children at every vertex, markers within markers too), and nesting past
//! [`MAX_DEPTH`]. Each refusal names the first such element and says what to do.

use klipolo_model::scene::SceneProblem;

use super::{MAX_DEPTH, problem};

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| from + i)
}

fn starts(b: &[u8], at: usize, prefix: &[u8]) -> bool {
    b.get(at..at + prefix.len())
        .is_some_and(|s| s.eq_ignore_ascii_case(prefix))
}

/// The end of the tag opened at `at` (its `>`, quotes honoured), or the end of the bytes.
fn tag_end(b: &[u8], at: usize) -> usize {
    let mut quote = None;
    for (i, &c) in b.iter().enumerate().skip(at) {
        match (quote, c) {
            (None, b'"' | b'\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, b'>') => return i,
            _ => {}
        }
    }
    b.len()
}

/// Text elements, Inkscape's flowed text among them (usvg without fonts drops every one silently).
const TEXT: [&[u8]; 8] = [
    b"text",
    b"tspan",
    b"textPath",
    b"flowRoot",
    b"flowPara",
    b"flowSpan",
    b"flowRegion",
    b"flowDiv",
];

/// What the import refuses by name, before any parser (usvg copies or converts these per use, per element or per
/// vertex before they could be refused): each with what it is, in the designer's words, and what to do about it.
const REFUSED: [(&[u8], &str, &str); 7] = [
    (b"pattern", "a pattern fill", "expand it into shapes"),
    (b"clipPath", "a clip path", "flatten or outline it"),
    (b"mask", "a mask", "flatten it (or release the mask)"),
    (
        b"filter",
        "a filter (a blur, shadow or similar effect)",
        "remove the effect or flatten it",
    ),
    (
        b"marker",
        "markers (arrowheads or dots on a line)",
        "expand them into shapes",
    ),
    (b"script", "a script, which Klipolo never runs", "remove it"),
    (
        b"foreignObject",
        "foreign content (<foreignObject>, such as HTML)",
        "remove it or convert it to shapes",
    ),
];

/// What the import refuses by name: `what`, and what to do.
pub(super) fn unsupported(what: &str, todo: &str) -> Vec<SceneProblem> {
    let can_t = if what.contains("never") {
        String::new()
    } else {
        ", which Klipolo can't import yet".to_owned()
    };
    problem(
        "svg.unsupported",
        format!("This SVG uses {what}{can_t}. In your design tool, {todo}, then import again."),
    )
}

pub(super) fn scan(b: &[u8]) -> Result<(), Vec<SceneProblem>> {
    if b.starts_with(&[0x1f, 0x8b]) {
        return Err(problem(
            "svg.parse",
            "This SVG is compressed (.svgz), which Klipolo doesn't read. Save it as a plain .svg in your design tool, \
             then import again.",
        ));
    }
    let mut i = 0;
    let mut depth = 0usize;
    while let Some(lt) = find(b, i, b"<") {
        let at = lt + 1;
        if starts(b, at, b"!--") {
            i = find(b, at + 3, b"-->").map_or(b.len(), |e| e + 3);
            continue;
        }
        if starts(b, at, b"![CDATA[") {
            i = find(b, at + 8, b"]]>").map_or(b.len(), |e| e + 3);
            continue;
        }
        if starts(b, at, b"?") {
            i = find(b, at, b"?>").map_or(b.len(), |e| e + 2);
            continue;
        }
        if starts(b, at, b"!DOCTYPE") {
            let end = tag_end(b, at);
            if b[at..end].contains(&b'[') {
                return Err(entity());
            }
            i = end + 1;
            continue;
        }
        if starts(b, at, b"!") {
            return Err(entity());
        }
        let end = tag_end(b, at);
        if starts(b, at, b"/") {
            depth = depth.saturating_sub(1);
            i = end + 1;
            continue;
        }
        let name_end = b[at..end]
            .iter()
            .position(|c| c.is_ascii_whitespace() || *c == b'/' || *c == b'>')
            .map_or(end, |p| at + p);
        let name = &b[at..name_end];
        let local = name.rsplit(|&c| c == b':').next().unwrap_or(name);
        let is = |n: &[u8]| local.eq_ignore_ascii_case(n);
        if let Some(name) = TEXT.iter().find(|n| is(n)) {
            return Err(problem(
                "svg.text",
                format!(
                    "This SVG has text (a <{}> element), which Klipolo can't import as part of a graphic. In your \
                     design tool, convert the text to outlines, then import again (or add the words as a text layer).",
                    String::from_utf8_lossy(name)
                ),
            ));
        }
        if let Some((_, what, todo)) = REFUSED.iter().find(|(n, ..)| is(n)) {
            return Err(unsupported(what, todo));
        }
        let closed = end > at && b.get(end - 1) == Some(&b'/');
        if !closed {
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(problem(
                    "svg.tooDeep",
                    format!(
                        "This SVG nests its elements more than {MAX_DEPTH} deep, more than Klipolo can import. In your \
                         design tool, ungroup some groups, then import again."
                    ),
                ));
            }
        }
        i = end + 1;
    }
    Ok(())
}

fn entity() -> Vec<SceneProblem> {
    problem(
        "svg.entity",
        "This SVG declares entities or a DTD subset (<!ENTITY …>), which Klipolo never expands. Save it from your \
         design tool without a DOCTYPE, then import again.",
    )
}
