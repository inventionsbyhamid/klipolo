//! Motion graphics in the missing-font rules: each text of a scene layer, as the app lists it
//! from the layer's instance (`make::fonts::scene_fonts`), runs the same per-font check: the one resolver's stand-in
//! rule (a Google family whose version isn't downloaded, an installed family this computer lacks; a family present at
//! another weight isn't missing: its nearest face draws), and CJK text without its Noto face. A use from a `Font`
//! setting is listed under `scene_params` (what Replace writes), never under `layers`; one written inside a saved scene
//! version is listed with neither, so only "Export with Inter instead" covers it.

use std::collections::HashMap;

use klipolo_model::font::FontRef;
use klipolo_model::project::Project;

use super::{MissingFont, User, add, cjk_needed, layers_and_captions, noto, offers};
use crate::cjk::CjkCover;
use crate::index::FontIndex;
use crate::resolve::resolve;

/// One text of a motion graphic layer: its font as drawn, the weight and italic, the layer, the `Font` setting it comes
/// from (if any), its text as the layer shows it, and whether the export draws the layer.
#[derive(Clone, Copy, Debug)]
pub struct SceneUse<'a> {
    pub font: &'a FontRef,
    pub weight: u16,
    pub italic: bool,
    pub layer: &'a str,
    pub param: Option<&'a str>,
    pub text: &'a str,
    pub drawn: bool,
}

/// Every font `p` uses that isn't here, its text layers' and captions' (`missing_in`) and the motion graphics' texts
/// `scenes`, with each one's offers.
pub fn missing_with(
    index: &FontIndex,
    p: &Project,
    text_language: &str,
    scenes: &[SceneUse<'_>],
) -> Vec<MissingFont> {
    let mut out = layers_and_captions(index, p, text_language);
    let mut resolved: HashMap<(&FontRef, u16, bool), (bool, CjkCover)> = HashMap::new();
    for s in scenes {
        let face = (s.weight, s.italic);
        let user = User::Scene(s.layer, s.param, s.drawn);
        let (stand_in, cover) =
            *resolved
                .entry((s.font, s.weight, s.italic))
                .or_insert_with(|| {
                    let r = resolve(index, s.font, s.weight, s.italic);
                    (r.stand_in, index.cjk_cover(&r.face))
                });
        if stand_in {
            add(&mut out, s.font.clone(), None, face, user);
        }
        for script in cjk_needed(s.text, text_language, cover) {
            if index.cjk(script).is_none() {
                add(&mut out, noto(index, script), Some(script), face, user);
            }
        }
    }
    offers(index, &mut out);
    out
}
