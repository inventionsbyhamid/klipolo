//! `check_scene`: one pass over a scene's settings, nodes and timeline that names every mistake with its JSON path,
//! and says what drawing it costs ([`SceneFacts`]). Total like the loader: bounded recursion (nodes deeper than
//! [`SceneLimits::MAX_DEPTH`] are refused, not visited), saturating counts, no indexing without a bound. Runs where a
//! scene comes in, never per frame.

use std::collections::BTreeMap;

use super::problems::{at, join, Problems};
use super::{Node, NodeType, Scene, SceneFacts, SceneLimits, SceneProblem, SCENE_VERSION};
use crate::contrib::Catalog;
use crate::params::ParamSpec;

mod animators;
mod bind;
mod blur;
mod common;
mod cost;
mod keyed;
mod kinds;
mod moving;
mod nodes;
mod paint;
mod params;
mod plan;
mod ranges;
mod shown;
mod text;
mod timeline;
mod timing;
mod tracks;
mod values;

pub(super) use bind::{binding, Binding};
use plan::analyze;
pub(super) use plan::{plan, PlanTween};
pub(super) use ranges::range_of;

/// Checks `scene` against its own params and `catalog` (the generators a fill may name, with their settings).
pub fn check_scene(scene: &Scene, catalog: &Catalog) -> Result<SceneFacts, Vec<SceneProblem>> {
    let mut out = Problems::default();
    match analyze(scene, catalog, &mut out) {
        Some(a) if out.is_empty() => Ok(a.facts),
        _ => Err(out.finish()),
    }
}

/// The setting a `"$key"` string is bound to, if it is bound to one (inside a `repeat`, `$i` and `$row…` aren't).
pub fn param_binding(s: &str) -> Option<&str> {
    match binding(s)? {
        Binding::Param(key) => Some(key),
        _ => None,
    }
}

fn scene_fields(scene: &Scene, out: &mut Problems) {
    if scene.scene != SCENE_VERSION {
        out.push(
            "/scene",
            "scene.version",
            format!(
                "This scene is scene@{}; this Klipolo reads scene@{SCENE_VERSION}.",
                scene.scene
            ),
        );
    }
    let d = scene.duration;
    if !(SceneLimits::MIN_DURATION..=SceneLimits::MAX_DURATION).contains(&d) {
        out.push(
            "/duration",
            "scene.duration",
            format!(
                "A scene lasts {} to {} seconds; this one says {d}.",
                SceneLimits::MIN_DURATION,
                SceneLimits::MAX_DURATION
            ),
        );
    }
    if let Some(seg) = scene.segments {
        let fits = [seg.enter, seg.out]
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
            && seg.enter + seg.out <= d;
        if !fits {
            out.push(
                "/segments",
                "scene.segments",
                format!(
                    "The entrance and exit (in {} s, out {} s) are each 0 or more and fit inside the scene's {d} s.",
                    seg.enter, seg.out
                ),
            );
        }
    }
}

/// The nearest enclosing `repeat`: what `$i` and `$row.…` may bind to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Repeat {
    Times,
    Rows { columns: usize },
}

/// A node with where it is: its path, instances once enclosing repeats are expanded, the repeat it is in (what it
/// may bind, and which node a cap names when its instances go over), and whether it is an emitter's particle shape.
pub(super) struct Flat<'a> {
    pub node: &'a Node,
    pub path: String,
    pub mult: usize,
    pub repeat: Option<Repeat>,
    /// The nearest enclosing repeat's index in `Scope::flat`.
    pub owner: Option<usize>,
    /// The enclosing node's index in `Scope::flat`.
    pub parent: Option<usize>,
    pub particle: bool,
}

impl Flat<'_> {
    /// The node a cap names for this one's cost: its repeat, which multiplies it, else itself.
    pub fn blame<'s>(&'s self, s: &'s Scope) -> &'s str {
        self.owner
            .and_then(|i| s.flat.get(i))
            .map_or(self.path.as_str(), |r| r.path.as_str())
    }
}

/// Where `Scope::add` is in the tree.
#[derive(Clone, Copy)]
struct Place {
    depth: usize,
    mult: usize,
    repeat: Option<Repeat>,
    owner: Option<usize>,
    parent: Option<usize>,
    particle: bool,
}

/// What every check reads: the scene, the catalog, the params by key, every node in document order, and ids.
pub(super) struct Scope<'a> {
    pub scene: &'a Scene,
    pub catalog: &'a Catalog,
    pub params: BTreeMap<&'a str, &'a ParamSpec>,
    pub flat: Vec<Flat<'a>>,
    pub ids: BTreeMap<&'a str, usize>,
}

impl<'a> Scope<'a> {
    fn add(&mut self, node: &'a Node, path: String, here: Place, out: &mut Problems) {
        if here.depth > SceneLimits::MAX_DEPTH {
            out.push(
                path,
                "scene.tooDeep",
                format!("Nodes nest at most {} deep.", SceneLimits::MAX_DEPTH),
            );
            return;
        }
        let index = self.flat.len();
        let inner = match node.kind {
            NodeType::Repeat => {
                let (count, repeat) = nodes::repeat_count(self, node);
                Place {
                    mult: here.mult.saturating_mul(count),
                    repeat: Some(repeat),
                    owner: Some(index),
                    ..here
                }
            }
            _ => here,
        };
        let children = join(&path, "children");
        let child_path = join(&path, "child");
        self.flat.push(Flat {
            node,
            path,
            mult: here.mult,
            repeat: here.repeat,
            owner: here.owner,
            parent: here.parent,
            particle: here.particle,
        });
        for (i, child) in node.children.iter().enumerate() {
            let place = Place {
                depth: here.depth + 1,
                parent: Some(index),
                ..inner
            };
            self.add(child, at(&children, i), place, out);
        }
        if let Some(child) = &node.props.child {
            let place = Place {
                depth: here.depth + 1,
                parent: Some(index),
                particle: true,
                ..here
            };
            self.add(child, child_path, place, out);
        }
    }

    /// Ids are unique, and ids and classes are names: a letter or `_`, then letters, digits, `_` and `-`.
    fn ids(&mut self, out: &mut Problems) {
        for (i, f) in self.flat.iter().enumerate() {
            let node: &'a Node = f.node;
            if let Some(id) = &node.id {
                let path = join(&f.path, "id");
                if !is_name(id) {
                    out.push(
                        path,
                        "scene.id",
                        "An id is a letter or _, then up to 63 letters, digits, _ or -.",
                    );
                } else if self.ids.insert(id.as_str(), i).is_some() {
                    out.push(
                        path,
                        "scene.idDuplicate",
                        format!("Another node already has the id `{id}`."),
                    );
                }
            }
            if let Some(class) = &node.class {
                // The length first, so a long class is never split.
                let fine = class.len() <= 128 && {
                    let names: Vec<&str> = class.split_whitespace().collect();
                    names.len() <= 8 && names.iter().all(|n| is_name(n))
                };
                if !fine {
                    out.push(
                        join(&f.path, "class"),
                        "scene.id",
                        "A class is up to 8 names separated by spaces, each a letter or _, then letters, digits, _ \
                         or -.",
                    );
                }
            }
        }
    }

    /// The node an id names.
    pub fn by_id(&self, id: &str) -> Option<&Flat<'a>> {
        self.ids.get(id).and_then(|&i| self.flat.get(i))
    }
}

/// A letter or `_`, then up to 63 letters, digits, `_` or `-`.
pub(super) fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
