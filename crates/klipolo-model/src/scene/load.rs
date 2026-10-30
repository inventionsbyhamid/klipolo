//! The two ways a scene comes in: an agent's submission, read only in the render worker, and a stored
//! scene, read everywhere else only after the worker's verdict. Both are total: size, then nesting (scanned before any
//! parsing), then the JSON, the version, every key against [`super::keys`], and the typed fields, each mistake a
//! [`SceneProblem`] with its path. Nothing here panics, and time and memory stay linear in the input (at most 256
//! KiB): serde's errors and the messages built for problems copy agent text, which `Problems` then cleans and cuts.

use serde::Deserialize;
use serde_json::{Map, Value};

use super::problems::Problems;
use super::walk::{walk, Entry};
use super::{Scene, SceneLimits, SceneProblem, SCENE_VERSION};

mod locate;

impl Scene {
    /// An agent's scene, in the render worker only: it may hold `svg` nodes, which the worker turns into `path`
    /// nodes before storing it.
    pub fn from_submission(bytes: &[u8]) -> Result<Scene, Vec<SceneProblem>> {
        load(bytes, Entry::Submission)
    }

    /// A stored scene (the library's, a project's embedded copy), only after the worker's verdict for that exact
    /// contribution: it never holds `svg` nodes.
    pub fn from_stored(bytes: &[u8]) -> Result<Scene, Vec<SceneProblem>> {
        load(bytes, Entry::Stored)
    }
}

/// A refusal with one problem.
fn refuse(path: &str, code: &'static str, message: String) -> Result<Scene, Vec<SceneProblem>> {
    let mut out = Problems::default();
    out.push(path, code, message);
    Err(out.finish())
}

fn load(bytes: &[u8], entry: Entry) -> Result<Scene, Vec<SceneProblem>> {
    if bytes.len() > SceneLimits::MAX_BYTES {
        return refuse(
            "",
            "scene.tooLarge",
            format!(
                "A scene is at most {} KiB of JSON; this one is {} KiB.",
                SceneLimits::MAX_BYTES / 1024,
                bytes.len().div_ceil(1024)
            ),
        );
    }
    if let Some(depth) = too_deep(bytes) {
        return refuse(
            "",
            "scene.tooDeep",
            format!(
                "The scene's JSON nests more than {} levels deep (at least {depth}).",
                SceneLimits::MAX_JSON_DEPTH
            ),
        );
    }
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            return refuse(
                "",
                "scene.json",
                format!("The scene isn't valid JSON: {e}."),
            )
        }
    };
    let Some(root) = value.as_object() else {
        return refuse(
            "",
            "scene.type",
            "A scene is a JSON object: {\"scene\": 1, …}.".into(),
        );
    };
    if let Some(problem) = version(root) {
        return Err(vec![problem]);
    }
    let mut out = Problems::default();
    walk(root, entry, &mut out);
    // A refused walk stops here: the typed parse would only rebuild, and recurse through, what it refused.
    if !out.is_empty() {
        return Err(out.finish());
    }
    Scene::deserialize(&value).map_err(|e| {
        locate::locate(root, &e, &mut out);
        out.finish()
    })
}

/// How deep the raw JSON nests past [`SceneLimits::MAX_JSON_DEPTH`], counting `[` and `{` outside strings.
fn too_deep(bytes: &[u8]) -> Option<usize> {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for &b in bytes {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > SceneLimits::MAX_JSON_DEPTH {
                    return Some(depth);
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// `"scene"` must be 1; a larger number was made for a newer Klipolo.
fn version(root: &Map<String, Value>) -> Option<SceneProblem> {
    let problem = |message: String| SceneProblem {
        path: "/scene".into(),
        code: "scene.version",
        message,
    };
    match root.get("scene") {
        Some(Value::Number(n)) if n.as_u64() == Some(u64::from(SCENE_VERSION)) => None,
        Some(Value::Number(n)) if n.as_f64().is_some_and(|v| v > f64::from(SCENE_VERSION)) => Some(problem(format!(
            "This scene was made for a newer Klipolo (scene@{n}); this one reads scene@{SCENE_VERSION}."
        ))),
        Some(_) => Some(problem(format!("\"scene\" is {SCENE_VERSION} for this format."))),
        None => Some(problem(format!(
            "A scene starts with \"scene\": {SCENE_VERSION}, the format's version."
        ))),
    }
}
