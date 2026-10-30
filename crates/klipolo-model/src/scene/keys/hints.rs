//! What a key's value should look like, and a close name for a key that isn't one: what CSS or GSAP calls it, or the
//! nearest spelling.

use super::super::NodeType;
use super::{accepts, own, COMMON};

/// What a key's value looks like, for a message about a value of the wrong shape.
pub fn expects(key: &str) -> &'static str {
    match key {
        "x" | "y" | "wrap" => "a fraction of the box, \"50%\", \"22px\" or a \"$key\" number setting",
        "box" => "[x, y, w, h]: fractions of the parent's box, \"50%\" or \"22px\"",
        "origin" => "\"50% 0%\", a word such as \"center\", or [x, y]",
        "visible" | "italic" => "true, false or a \"$key\" on/off setting",
        "fill" | "stroke" => {
            "a colour (\"#FFB000\", \"rgba(255, 176, 0, 0.5)\", \"none\"), a \"$key\" colour setting, a gradient \
             ({\"linear\": [x0, y0, x1, y1], \"stops\": [[0, \"#FFFFFF\"], [1, \"#000000\"]]}) or a generator \
             ({\"generator\": \"<id>\", \"params\": {…}})"
        }
        "font" => {
            "a family name (\"Inter\"), {\"family\": …, \"source\": {\"type\": \"bundled\" | \"installed\" | \
             \"google\", \"version\"?: \"v31\"}, \"weight\"?: 700, \"italic\"?: true} or a \"$key\" font setting"
        }
        "radius" => "a number of 1080p pixels or [top left, top right, bottom right, bottom left]",
        "points" => "a \"$key\" table setting or [[x, y], …]",
        "dash" | "sizeOverLife" | "opacityOverLife" => "a list of numbers",
        "speed" | "spin" | "life" => "a number or [min, max]",
        "gap" => "[dx, dy]",
        "cap" => "\"butt\", \"round\" or \"square\"",
        "join" => "\"miter\", \"round\" or \"bevel\"",
        "align" => "\"left\", \"center\" or \"right\"",
        "case" => "\"upper\" or \"lower\"",
        "split" => "\"chars\", \"words\" or \"lines\"",
        "fit" => "\"shrink\" or \"clip\"",
        "blend" => "\"normal\", \"multiply\", \"screen\", \"add\" or \"overlay\"",
        "stagger" => "seconds, or {\"each\" | \"amount\": seconds, \"from\"?: \"start\" | \"end\" | \"center\" | \
                      \"edges\" | \"random\", \"seed\"?: n}",
        "trim" => "{\"start\"?, \"end\"?, \"offset\"?}: fractions of the stroke",
        "shadow" => "{\"x\"?, \"y\"?, \"blur\"?, \"spread\"?: 1080p pixels, \"color\"?: a colour}",
        "mask" => "{\"node\": \"#id\", \"mode\"?: \"alpha\" | \"luma\", \"invert\"?: true}",
        "motionPath" => {
            "{\"path\": SVG path data or \"#id\" of a path node, \"progress\"?: 0–1, \"autoRotate\"?: true, \
             \"viewBox\"?: [x, y, w, h], \"preserveAspectRatio\"?: \"none\"}"
        }
        "viewBox" => "[x, y, width, height]: finite, the width and height above 0",
        "preserveAspectRatio" => "\"xMidYMid meet\" or \"none\"",
        "fillRule" => "\"nonzero\" or \"evenodd\"",
        "modifiers" => "{\"<property>\": {\"type\": \"wiggle\", \"freq\", \"amp\", \"seed\"?} | {\"type\": \
                        \"oscillate\", \"freq\", \"amp\", \"phase\"?, \"decay\"?} | {\"type\": \"marquee\", \"speed\"}}",
        "spans" => "[{\"text\", \"fill\"?, \"size\"?, \"weight\"?, \"italic\"?, \"background\"?}]",
        "outline" => "{\"width\": 1080p pixels, \"color\": a colour}",
        "count" => "{\"from\", \"to\", \"decimals\"?, \"separator\"?, \"prefix\"?, \"suffix\"?, \"at\"?, \"dur\"?, \"ease\"?}",
        "reveal" => "{\"by\"?: \"chars\" | \"words\", \"at\"?, \"dur\"?, \"ease\"?}",
        "scramble" => "{\"chars\"?, \"seed\"?, \"at\"?, \"dur\"?}",
        "holds" => "[{\"t\": seconds, \"text\": …}]",
        "cone" => "{\"angle\"?: degrees, \"spread\"?: degrees}",
        "child" => "one node",
        "children" => "a list of nodes",
        "animate" => "{\"<property>\": [{\"t\": seconds, \"v\": value, \"ease\"?, \"unit\"?: \"px\"}]}",
        "text" | "emoji" | "asset" | "rows" | "src" | "clip" | "d" | "tint" | "id" | "class" | "file" | "ease"
        | "label" => "a string",
        "smooth" | "yoyo" | "strokeScale" | "autoRotate" => "true or false",
        "at" => "seconds or a position such as \"<\", \">\", \"+=0.2\", a label or \"out\"",
        "to" | "from" | "fromTo" | "set" => "\"#id\", \".class\", a node type, or a list of them",
        "start" => "{\"<property>\": value}",
        "repeat" => "a whole number, or -1 to repeat until the scene ends",
        "seed" | "decimals" => "a whole number",
        _ => "a number or a \"$key\" number setting",
    }
}

/// Names agents carry over from CSS, GSAP and other tools, and what Klipolo calls them.
const ALIASES: &[(&str, &str)] = &[
    ("color", "fill"),
    ("colour", "fill"),
    ("background", "fill"),
    ("backgroundColor", "fill"),
    ("fillColor", "fill"),
    ("borderColor", "stroke"),
    ("strokeColor", "stroke"),
    ("borderWidth", "strokeWidth"),
    ("lineWidth", "strokeWidth"),
    ("borderRadius", "radius"),
    ("cornerRadius", "radius"),
    ("rotation", "rotate"),
    ("angle", "rotate"),
    ("transformOrigin", "origin"),
    ("anchor", "origin"),
    ("pivot", "origin"),
    ("fontSize", "size"),
    ("fontWeight", "weight"),
    ("fontFamily", "font"),
    ("fontStyle", "italic"),
    ("textAlign", "align"),
    ("textTransform", "case"),
    ("clipPath", "clip"),
    ("mixBlendMode", "blend"),
    ("blendMode", "blend"),
    ("filter", "blur"),
    ("alpha", "opacity"),
    ("autoAlpha", "opacity"),
    ("duration", "dur"),
    ("delay", "at"),
    ("position", "at"),
    ("time", "t"),
    ("value", "v"),
    ("width", "box"),
    ("height", "box"),
    ("left", "box"),
    ("top", "box"),
    ("content", "text"),
    ("src", "asset"),
    ("url", "asset"),
    ("image", "asset"),
    ("path", "d"),
    ("count", "times"),
    ("repeat", "times"),
    ("keyframes", "animate"),
    ("keys", "animate"),
    ("animation", "animate"),
    ("drawSVG", "trim.end"),
];

/// A key from `candidates` close to `name`: what CSS or GSAP calls it, else the nearest spelling (ignoring ASCII
/// case, at most a third of its letters changed).
pub fn suggest<'a>(name: &str, candidates: &[&'a str]) -> Option<&'a str> {
    if let Some(target) = ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .and_then(|(_, target)| candidates.iter().find(|c| *c == target))
    {
        return Some(*target);
    }
    if name.chars().count() > 64 {
        return None;
    }
    let most = (name.chars().count() / 3).clamp(1, 3);
    candidates
        .iter()
        .map(|c| (distance(name, c), *c))
        .filter(|(d, _)| *d <= most)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Levenshtein distance, ignoring ASCII case.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().map(|c| c.to_ascii_lowercase()).collect();
    let b: Vec<char> = b.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diag = i;
        let mut left = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let up = row.get(j + 1).copied().unwrap_or(usize::MAX);
            let cost = diag + usize::from(ca != cb);
            let next = cost.min(up.saturating_add(1)).min(left + 1);
            diag = up;
            if let Some(slot) = row.get_mut(j + 1) {
                *slot = next;
            }
            left = next;
        }
        if let Some(first) = row.first_mut() {
            *first = i + 1;
        }
    }
    row.last().copied().unwrap_or(0)
}

/// Why a node of this type doesn't take `key`, with what it may have meant: "did you mean `fill`?", or which node
/// types do take it.
pub fn why_not(kind: NodeType, key: &str, node_path: &str) -> String {
    let shown = super::super::problems::quote(key);
    let mut accepted: Vec<&str> = COMMON.to_vec();
    accepted.extend_from_slice(own(kind));
    if let Some(close) = suggest(key, &accepted) {
        return format!("unknown property `{shown}` at {node_path}; did you mean `{close}`?");
    }
    let others: Vec<&str> = NodeType::ALL
        .iter()
        .filter(|t| **t != NodeType::Svg && accepts(**t, key))
        .map(|t| t.name())
        .collect();
    if !others.is_empty() {
        return format!(
            "a {} node has no `{shown}` (at {node_path}); {} nodes do",
            kind.name(),
            others.join(", ")
        );
    }
    format!(
        "unknown property `{shown}` at {node_path}; a {} node takes {}",
        kind.name(),
        accepted.join(", ")
    )
}
