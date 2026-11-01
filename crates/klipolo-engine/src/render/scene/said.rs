//! What a scene says about itself, once per scene until the project closes: what it left out, paths too detailed to
//! draw, its text's notes, and text that can't be built.

use std::sync::Arc;

use klipolo_model::scene::eval::CapsOver;

use super::text::TextNotes;
use crate::api::{
    FRAME_CJK_FONT_MISSING, FRAME_FONT_MISSING, FRAME_SCENE_REDUCED, FRAME_SCENE_STILL,
    FRAME_SCENE_TEXT_CUT, FRAME_SCENE_TEXT_MISSING, FRAME_SCENE_TOO_DETAILED,
    FRAME_SCENE_TOO_MANY_GROUPS, FRAME_TEXTS_SOFTER,
};

/// What a frame of a scene drew short of: the runtime caps it is past (`over`), a path too detailed to draw
/// (`detailed`), and what its text tells (a stand-in face, CJK without its Noto Sans, text cut by
/// the caps, and an atlas drawn softer for its budget).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Short {
    pub over: CapsOver,
    pub detailed: bool,
    pub text: TextNotes,
    pub softer: bool,
    /// Its text can't be built (the build thread is gone): drawn without it, or with an earlier text.
    pub text_missing: bool,
    /// A node past the frame's offscreen caps drew in place, without its effect.
    pub groups: bool,
    /// Something past the other runtime caps was left out: generator fills, backdrops, emitters or blurring units.
    pub reduced: bool,
}

/// Says each thing `short` names for scene `contrib`, once until the project closes: instances or particles left out,
/// the still a scene past the track cap shows, a path too detailed to draw, and its text's notes: the text codes for a
/// stand-in face or CJK without its download (an export stops on those, as on a title's), text cut (`node_chars` or
/// `scene_chars` past the caps, or the renderer's own caps) and an atlas drawn softer; and nodes past the
/// offscreen caps drawn in place (`frame.sceneTooManyGroups`), and fills, backdrops, emitters or blurring units past
/// theirs left out (`frame.sceneReduced`).
pub(super) fn say(
    said: &mut Vec<(Arc<str>, &'static str)>,
    contrib: &Arc<str>,
    short: Short,
    warnings: &mut Vec<String>,
) {
    let Short {
        over,
        detailed,
        text,
        softer,
        text_missing,
        groups,
        reduced,
    } = short;
    let codes = [
        (
            over.instances || over.particles || reduced,
            FRAME_SCENE_REDUCED,
        ),
        (groups || over.isolated_groups, FRAME_SCENE_TOO_MANY_GROUPS),
        (over.tracks, FRAME_SCENE_STILL),
        (detailed, FRAME_SCENE_TOO_DETAILED),
        (text.stand_in, FRAME_FONT_MISSING),
        (text.cjk_missing != 0, FRAME_CJK_FONT_MISSING),
        (
            text.cut || over.node_chars || over.scene_chars,
            FRAME_SCENE_TEXT_CUT,
        ),
        (softer, FRAME_TEXTS_SOFTER),
        (text_missing, FRAME_SCENE_TEXT_MISSING),
    ];
    for (on, code) in codes {
        if on && !said.iter().any(|(c, k)| *k == code && **c == **contrib) {
            said.push((contrib.clone(), code));
            warnings.push(code.to_owned());
        }
    }
}
