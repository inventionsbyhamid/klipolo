//! A scene checked and measured in the render worker (the costly
//! settings, R2-m4, R3-k8). What an agent submits (`CheckScene`) and what a library or project folder holds (`Verify`)
//! take one path (`checked`): read with the total loader, a submission's `svg` nodes flattened into `path` nodes
//! (`svgs`), written as the canonical bytes a library stores and a key covers (`normalise`, m1) and read again
//! as stored (so what is saved is exactly what the main process will accept), checked against the built-ins and the
//! generators its fills name (`inputs`: the app's `contribs.json`), and compiled. Its cost (`cost`) is the
//! measurement with the scene as the draft layer and its costly settings ([`costly`]) at their costly end, then its
//! CPU time a frame and its memory as the scene drawer holds it.

mod checked;
mod cost;
mod inputs;
mod svgs;

use klipolo_model::params::ParamKind;
use klipolo_model::scene::timeline::CompiledScene;
use klipolo_model::scene::{Scene, SceneProblem, Val, param_binding};

use crate::registry::guard::{CostlyParam, End};
pub use checked::{CheckedScene, Fills, SceneInput, check};
pub use cost::{CPU_FRAMES, scene_cost};
pub use inputs::{read_fills, read_svgs};

/// The canonical bytes of a stored scene: what the library stores and a key covers. Read as stored
/// (`Scene::from_stored`: no `svg` node), then written again compactly in the format's own field order, so normalising
/// what this made gives the same bytes. The built-in scene presets are stored this way.
pub fn normalise(bytes: &[u8]) -> Result<Vec<u8>, Vec<SceneProblem>> {
    written(&Scene::from_stored(bytes)?)
}

/// `scene` written compactly in the format's field order.
pub(crate) fn written(scene: &Scene) -> Result<Vec<u8>, Vec<SceneProblem>> {
    serde_json::to_vec(scene).map_err(|e| {
        vec![SceneProblem {
            path: String::new(),
            code: "scene.json",
            message: format!("The scene couldn't be written down: {e}."),
        }]
    })
}

/// The settings of `scene` its cost grows with, each costly at its maximum (R2-m4): a number setting bound to a
/// `repeat`'s `times`, an emitter's `burst` or `rate`, a `blur` or a text's `size`, and a table setting a `repeat` draws
/// a row of (`rows`: costly at its most rows). An emitter's `life` takes numbers only, so no setting reaches it. In the
/// scene's param order, each once.
pub fn costly(scene: &CompiledScene) -> Vec<CostlyParam> {
    let mut bound: Vec<&str> = Vec::new();
    for n in &scene.nodes {
        let p = &n.node.props;
        for v in [&p.times, &p.burst, &p.rate, &p.blur, &p.size]
            .into_iter()
            .flatten()
        {
            if let Val::Str(s) = v
                && let Some(key) = param_binding(s)
            {
                bound.push(key);
            }
        }
        if let Some(key) = p.rows.as_deref().and_then(param_binding) {
            bound.push(key);
        }
    }
    scene
        .params
        .iter()
        .filter(|spec| bound.contains(&spec.key.as_str()))
        .filter(|spec| {
            matches!(
                spec.kind,
                ParamKind::Float { .. } | ParamKind::Int { .. } | ParamKind::Table { .. }
            )
        })
        .map(|spec| CostlyParam {
            key: spec.key.clone(),
            costly_end: End::Max,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use klipolo_model::contrib::Catalog;

    use super::*;

    /// The canonical bytes are compact, in the format's order, and normalise to themselves; a stored scene
    /// can't hold an `svg` node.
    #[test]
    fn normalised_bytes_normalise_to_themselves() {
        let json = r##"{ "scene": 1, "duration": 2.5,
          "params": [{"key": "title", "label": "Title", "type": "text", "default": "Hi", "maxLen": 40},
            {"key": "accent", "label": "Accent", "type": "color", "default": "#FF5500"}],
          "nodes": [ {"type": "text", "id": "t", "text": "$title", "box": [0, 0.5, 1, 0.3], "size": 64.5,
            "fill": "$accent"} ],
          "timeline": [ {"from": "#t", "dur": 0.5, "opacity": 0, "ease": "power3.out", "stagger": 0.1} ] }"##;
        let once = normalise(json.as_bytes()).unwrap_or_else(|p| panic!("{p:#?}"));
        assert_eq!(normalise(&once).unwrap(), once);
        assert!(!once.contains(&b'\n') && !once.starts_with(b"{ "));
        let svg = r#"{"scene":1,"duration":2,"nodes":[{"type":"svg","src":"logo"}]}"#;
        let refused = normalise(svg.as_bytes()).unwrap_err();
        assert_eq!(refused[0].code, "scene.svgStored");
    }

    #[test]
    fn a_scenes_settings_that_grow_its_work_are_costly_at_their_most() {
        let json = r##"{"scene":1,"duration":2,
          "params":[{"key":"amount","label":"Amount","type":"int","min":0,"max":500,"default":40},
            {"key":"copies","label":"Copies","type":"int","min":1,"max":200,"default":3},
            {"key":"soft","label":"Soft","type":"float","min":0,"max":40,"default":0},
            {"key":"big","label":"Size","type":"float","min":10,"max":300,"default":60},
            {"key":"tint","label":"Tint","type":"color","default":"#FFFFFF"},
            {"key":"spin","label":"Spin","type":"float","min":0,"max":360,"default":0},
            {"key":"title","label":"Title","type":"text","default":"Hi","maxLen":40}],
          "nodes":[
            {"type":"emitter","id":"e","box":[0,0,1,1],"burst":"$amount","child":
              {"type":"ellipse","box":[0,0,0.01,0.01],"fill":"$tint"}},
            {"type":"repeat","times":"$copies","gap":[0.1,0],"children":[
              {"type":"rect","box":[0,0,0.05,0.05],"fill":"$tint","blur":"$soft","rotate":"$spin"}]},
            {"type":"text","id":"t","text":"$title","box":[0,0.5,1,0.3],"size":"$big"}]}"##;
        let scene = crate::registry::compile_scene(json.as_bytes(), Catalog::builtin())
            .unwrap_or_else(|p| panic!("{p:#?}"));
        let keys: Vec<(String, End)> = costly(&scene)
            .into_iter()
            .map(|c| (c.key, c.costly_end))
            .collect();
        let max = |k: &str| (k.to_string(), End::Max);
        assert_eq!(
            keys,
            [max("amount"), max("copies"), max("soft"), max("big")],
            "a colour, a turn and a text aren't costly"
        );
    }
}
