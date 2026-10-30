//! A submission's `svg` nodes flattened (`make::svg::flatten`): each, at any depth, becomes in
//! its place a `group` of the SVG's `path` nodes, keeping the node's own id, class, box, transforms, opacity and keys,
//! so a tween naming it moves the drawing and every JSON path after it stays the same. A problem names the node's
//! `src` (`/nodes/2/src`): an SVG the request didn't bring, or one the import refused, with the import's own words.
//!
//! The scene's caps come before the work: the nodes are counted first (more than
//! [`SceneLimits::MAX_NODES`] is refused before any SVG is read), each SVG the request brought is flattened at most
//! once whatever number of nodes name it, and the nodes its paths add are counted as they go in, so the first `svg`
//! node that would take the scene past the cap is refused at its `src` and nothing more is flattened. An `svg` node at
//! the deepest level is refused at its `src` too: its paths would sit one level deeper than a scene may nest.
//! The path data the uses copy is counted the same way: past [`SceneLimits::MAX_BYTES`] in all
//! (what a whole stored scene holds, so no scene that could be stored is refused) the use is refused at its `src`
//! before its paths are copied, so one request never makes the worker copy and write tens of MiB.

use std::collections::HashMap;

use klipolo_model::scene::{Node, NodeType, Scene, SceneLimits, SceneProblem};

use crate::make::svg::flatten;

/// The SVGs a request brought, at most this many (`svgs`).
pub const MAX_SVGS: usize = 4;

/// One SVG's paths and the bytes of path data they hold, or why it was refused.
type Flattened = Result<(Vec<Node>, usize), Vec<SceneProblem>>;

/// What one splice has done: each SVG flattened once (by name), how many times `flatten` ran, the scene's nodes
/// counted so far with what the SVGs added, and the path data the uses copied.
#[derive(Default)]
struct Splice {
    flattened: HashMap<String, Flattened>,
    flattens: usize,
    nodes: usize,
    path_bytes: usize,
    problems: Vec<SceneProblem>,
}

/// `scene` with every `svg` node replaced by a group of the paths of the SVG its `src` names in `svgs` (name, text).
pub fn splice(scene: &mut Scene, svgs: &[(String, Vec<u8>)]) -> Result<(), Vec<SceneProblem>> {
    splice_counted(scene, svgs).map(drop)
}

/// [`splice`], answering how many times an SVG was flattened (the tests read it).
pub fn splice_counted(
    scene: &mut Scene,
    svgs: &[(String, Vec<u8>)],
) -> Result<usize, Vec<SceneProblem>> {
    let written: usize = scene.nodes.iter().map(count).sum();
    if written > SceneLimits::MAX_NODES {
        return Err(vec![too_many("/nodes".into(), written)]);
    }
    let mut s = Splice {
        nodes: written,
        ..Splice::default()
    };
    for (i, node) in scene.nodes.iter_mut().enumerate() {
        if !s.problems.is_empty() {
            break;
        }
        splice_node(node, &format!("/nodes/{i}"), 1, svgs, &mut s);
    }
    if s.problems.is_empty() {
        Ok(s.flattens)
    } else {
        Err(s.problems)
    }
}

/// `node`'s nodes as the check counts them: itself, its children and an emitter's particle shape.
fn count(node: &Node) -> usize {
    1 + node.children.iter().map(count).sum::<usize>()
        + node.props.child.as_deref().map_or(0, count)
}

fn too_many(path: String, nodes: usize) -> SceneProblem {
    SceneProblem {
        path,
        code: "scene.tooManyNodes",
        message: format!(
            "A scene has at most {} nodes, counting children and the paths its SVGs become; this one would have \
             {nodes}. Use fewer SVG nodes, or simpler SVGs.",
            SceneLimits::MAX_NODES
        ),
    }
}

/// The path data `node` (one of an SVG's paths) holds.
fn path_bytes(node: &Node) -> usize {
    node.props.d.as_ref().map_or(0, String::len)
}

fn too_much_path(path: String, bytes: usize) -> SceneProblem {
    SceneProblem {
        path,
        code: "scene.tooLarge",
        message: format!(
            "A motion graphic holds at most {} KB in all; with this svg node its SVGs' paths alone would be {} KB. \
             Use fewer svg nodes, or a simpler SVG.",
            SceneLimits::MAX_BYTES / 1024,
            bytes.div_ceil(1024)
        ),
    }
}

/// `node` (at nesting `depth`, top-level nodes at 1) and its children (a `group`'s or `repeat`'s, an emitter's
/// `child`), at JSON pointer `path`. The loader bounds the nesting (8 levels), so the recursion does too.
fn splice_node(
    node: &mut Node,
    path: &str,
    depth: usize,
    svgs: &[(String, Vec<u8>)],
    s: &mut Splice,
) {
    if !s.problems.is_empty() {
        return;
    }
    if node.kind == NodeType::Svg {
        splice_svg(node, path, depth, svgs, s);
        return;
    }
    for (j, child) in node.children.iter_mut().enumerate() {
        splice_node(child, &format!("{path}/children/{j}"), depth + 1, svgs, s);
    }
    if let Some(child) = node.props.child.as_deref_mut() {
        splice_node(child, &format!("{path}/child"), depth + 1, svgs, s);
    }
}

fn splice_svg(
    node: &mut Node,
    path: &str,
    depth: usize,
    svgs: &[(String, Vec<u8>)],
    s: &mut Splice,
) {
    let at = format!("{path}/src");
    if depth >= SceneLimits::MAX_DEPTH {
        s.problems.push(SceneProblem {
            path: at,
            code: "scene.tooDeep",
            message: format!(
                "An svg node adds one level: its paths go inside it. Nodes nest at most {} deep, and this one is at \
                 depth {depth}: move it up a level.",
                SceneLimits::MAX_DEPTH
            ),
        });
        return;
    }
    let src = node.props.src.take().unwrap_or_default();
    let Some((_, text)) = svgs.iter().find(|(name, _)| *name == src) else {
        let brought: Vec<String> = svgs
            .iter()
            .map(|(n, _)| format!("`{}`", short(n)))
            .collect();
        s.problems.push(SceneProblem {
            path: at,
            code: "scene.svgMissing",
            message: if brought.is_empty() {
                format!(
                    "This svg node names `{}`, but the request brought no SVGs: give its text in `svgs`.",
                    short(&src)
                )
            } else {
                format!(
                    "This svg node names `{}`, but the request's svgs are {}.",
                    short(&src),
                    brought.join(", ")
                )
            },
        });
        return;
    };
    if !s.flattened.contains_key(&src) {
        s.flattens += 1;
        let flattened = flatten(text).map(|paths| {
            let bytes = paths.iter().map(path_bytes).sum();
            (paths, bytes)
        });
        s.flattened.insert(src.clone(), flattened);
    }
    match &s.flattened[&src] {
        Ok((paths, bytes)) => {
            // The group takes the node's own place: only its paths are new.
            let nodes = s.nodes + paths.len();
            if nodes > SceneLimits::MAX_NODES {
                s.problems.push(too_many(at, nodes));
                return;
            }
            let written = s.path_bytes + bytes;
            if written > SceneLimits::MAX_BYTES {
                s.problems.push(too_much_path(at, written));
                return;
            }
            (s.nodes, s.path_bytes) = (nodes, written);
            node.kind = NodeType::Group;
            node.children = paths.clone();
        }
        Err(found) => {
            let found = found.clone();
            s.problems.extend(found.into_iter().map(|p| SceneProblem {
                path: at.clone(),
                ..p
            }));
        }
    }
}

/// An SVG's name as a problem quotes it: at most 64 characters, shown as written only when nothing in it hides.
fn short(name: &str) -> String {
    crate::registry::guard::shown(&name.chars().take(64).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One small SVG: two shapes, so each `svg` node becomes a group of two paths.
    const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
      <rect x="5" y="5" width="50" height="50" fill="#2D6CDF"/><circle cx="70" cy="70" r="25" fill="#FF7A00"/></svg>"##;

    fn scene_of(nodes: &str) -> Scene {
        let json = format!(r#"{{"scene":1,"duration":2,"nodes":[{nodes}]}}"#);
        Scene::from_submission(json.as_bytes()).unwrap_or_else(|p| panic!("{p:#?}"))
    }

    fn svgs() -> Vec<(String, Vec<u8>)> {
        vec![("logo".into(), LOGO.as_bytes().to_vec())]
    }

    /// 300 svg nodes naming one SVG are refused before anything is flattened, and nodes up to the cap flatten it
    /// once; the first one past the cap is refused at its `src`.
    #[test]
    fn caps_come_before_svg_work_and_an_svg_is_flattened_once() {
        let many = vec![r#"{"type":"svg","src":"logo"}"#; 300].join(",");
        let refused = splice_counted(&mut scene_of(&many), &svgs()).unwrap_err();
        assert_eq!(
            (refused[0].code, refused[0].path.as_str()),
            ("scene.tooManyNodes", "/nodes")
        );
        // 70 nodes would become 70 groups of two paths (210 nodes): node 65 is the first past 200.
        let some = vec![r#"{"type":"svg","src":"logo"}"#; 70].join(",");
        let refused = splice_counted(&mut scene_of(&some), &svgs()).unwrap_err();
        assert_eq!(refused[0].path, "/nodes/65/src", "{refused:?}");
        let few = vec![r#"{"type":"svg","src":"logo"}"#; 20].join(",");
        assert_eq!(
            splice_counted(&mut scene_of(&few), &svgs()),
            Ok(1),
            "flattened once"
        );
    }

    /// An SVG of about 30 KB of path data named by many nodes is refused at the first use that takes the paths
    /// copied past what a stored scene holds (256 KB), before they are copied; fewer uses of it pass.
    #[test]
    fn path_data_past_a_scenes_size_is_refused_at_the_use_that_passes_it() {
        // 4 zigzags of 1,000 points each (about 8 KB of path data apiece).
        let d: String = (0..1000)
            .map(|i| format!("L{} {} ", i % 400, (i % 2) * 50 + i / 10))
            .collect();
        let rects: String = (0..4)
            .map(|_| format!(r##"<path d="M0 0 {d}Z" fill="#2D6CDF"/>"##))
            .collect();
        let heavy = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 400">{rects}</svg>"#
        );
        let svgs = vec![("heavy".to_string(), heavy.into_bytes())];
        let mut one = scene_of(r#"{"type":"svg","src":"heavy"}"#);
        splice_counted(&mut one, &svgs).unwrap_or_else(|p| panic!("{p:#?}"));
        let per_use: usize = one.nodes[0].children.iter().map(path_bytes).sum();
        assert!(per_use > 20_000, "{per_use}");
        let uses = SceneLimits::MAX_BYTES / per_use + 1;
        let many = vec![r#"{"type":"svg","src":"heavy"}"#; uses].join(",");
        let refused = splice_counted(&mut scene_of(&many), &svgs).unwrap_err();
        assert_eq!(refused[0].code, "scene.tooLarge", "{refused:?}");
        assert_eq!(refused[0].path, format!("/nodes/{}/src", uses - 1));
        let fewer = vec![r#"{"type":"svg","src":"heavy"}"#; uses - 1].join(",");
        assert_eq!(splice_counted(&mut scene_of(&fewer), &svgs), Ok(1));
    }

    /// An svg node at the deepest level is refused at its `src`, not later at a path the agent never wrote.
    #[test]
    fn an_svg_at_the_deepest_level_is_refused_at_its_src() {
        let mut nested = r#"{"type":"svg","src":"logo"}"#.to_string();
        for _ in 1..SceneLimits::MAX_DEPTH {
            nested = format!(r#"{{"type":"group","children":[{nested}]}}"#);
        }
        let refused = splice_counted(&mut scene_of(&nested), &svgs()).unwrap_err();
        assert_eq!(refused[0].code, "scene.tooDeep");
        assert!(refused[0].path.ends_with("/src"), "{refused:?}");
    }
}
