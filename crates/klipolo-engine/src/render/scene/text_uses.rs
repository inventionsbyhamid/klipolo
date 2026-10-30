//! The fonts a scene layer's text draws in, for the missing-font rules (the app's
//! `make::fonts`): one use per text node of the layer's instance, its font as the drawer resolves it (`text_font`:
//! the `FontRef` as written, a shorthand through the index, a `"$font"` setting's value), the weight and italic it is
//! drawn at, the setting it comes from (if any), and its text as the layer shows it (so CJK without its Noto face is
//! caught as for a title). Built once per scene layer when asked (the banner, the export's preflight), never per frame.

use std::collections::BTreeMap;
use std::sync::Arc;

use klipolo_fonts::index::FontIndex;
use klipolo_model::font::FontRef;
use klipolo_model::params::ParamValue;
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::timeline::CompiledScene;
use klipolo_model::scene::{NodeType, SceneFont};

use super::text::DEFAULT_FAMILY;
use super::text_font::{NodeBindings, font_ref, own_look};

/// One text of a scene layer: its font, weight and italic as drawn, the `Font` setting it comes from, and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextFontUse {
    pub font: FontRef,
    pub weight: u16,
    pub italic: bool,
    pub param: Option<String>,
    pub text: String,
}

/// Every text of `scene` with the layer's settings `params`, as the drawer resolves its font against `index`.
pub fn text_font_uses(
    scene: &Arc<CompiledScene>,
    params: &BTreeMap<String, ParamValue>,
    index: &FontIndex,
) -> Vec<TextFontUse> {
    let inst = SceneInstance::new(scene, params);
    let mut out = Vec::new();
    for i in 0..inst.len() {
        let Some(node) = inst
            .cnode(i)
            .and_then(|c| scene.nodes.get(c))
            .map(|c| &c.node)
        else {
            continue;
        };
        if node.kind != NodeType::Text {
            continue;
        }
        let bindings = NodeBindings {
            inst: &inst,
            node: i,
        };
        let default = SceneFont::Name(DEFAULT_FAMILY.into());
        let written = node.props.font.as_ref().unwrap_or(&default);
        let (font, weight, italic) = font_ref(written, node, index, &bindings);
        let param = match written {
            SceneFont::Name(name) => klipolo_model::scene::param_binding(name).map(str::to_owned),
            SceneFont::Ref { .. } => None,
        };
        out.push(TextFontUse {
            font,
            weight,
            italic,
            param,
            text: inst.own_text(i).to_owned(),
        });
    }
    out
}

/// The faces (weight, italic) the texts of `scene` whose font is its `Font` setting `key` are drawn at, with the layer's
/// settings `params`, each once (what `replaceFonts` asks the new family at). A setting
/// holds the family only: each text keeps its own weight and italic (`own_look`).
pub fn setting_faces(
    scene: &Arc<CompiledScene>,
    params: &BTreeMap<String, ParamValue>,
    key: &str,
) -> Vec<(u16, bool)> {
    let inst = SceneInstance::new(scene, params);
    let mut out = Vec::new();
    for i in 0..inst.len() {
        let Some(node) = inst
            .cnode(i)
            .and_then(|c| scene.nodes.get(c))
            .map(|c| &c.node)
        else {
            continue;
        };
        let bound = matches!(&node.props.font, Some(SceneFont::Name(name))
            if klipolo_model::scene::param_binding(name) == Some(key));
        if node.kind != NodeType::Text || !bound {
            continue;
        }
        let face = own_look(
            node,
            &NodeBindings {
                inst: &inst,
                node: i,
            },
        );
        if !out.contains(&face) {
            out.push(face);
        }
    }
    out
}
