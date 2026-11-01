//! Which face each text draws in: the one resolver's answer for its font, weight and
//! style, its CJK stretches in their language's face, and what the frame says about them.
//!
//! Resolved at layout time only (a text is laid out again only when it changes), cached per font until the fonts change
//! (`clear`). Each laid-out text keeps its `Notes`, which `Said` turns into the frame's codes once per frame: a steady
//! frame looks them up and allocates nothing.

use std::collections::HashMap;

use klipolo_fonts::cjk::has_cjk;
use klipolo_model::font::FontRef;
use klipolo_model::text_language::CjkScript;
use klipolo_model::timeline::TextStyle;

use crate::api::{FRAME_CJK_FONT_MISSING, FRAME_FONT_MISSING};
use crate::render::fonts::{self, FaceChoice, family_for};
use crate::render::script_runs::{Runs, runs};

/// A face asked for at (weight, italic), and the face that answers.
type Asked = ((u16, bool), FaceChoice);

/// The faces resolved so far, per font, then per (asked weight, italic).
#[derive(Default)]
pub(super) struct Faces {
    cache: HashMap<FontRef, Vec<Asked>>,
}

impl Faces {
    /// Forgets every answer (the fonts changed: a registration, or the scan finished).
    pub(super) fn clear(&mut self) {
        self.cache.clear();
    }

    /// The face `style` draws in: exactly what `klipolo_fonts::resolve` picks in the engine's index.
    pub(super) fn choice(&mut self, style: &TextStyle) -> FaceChoice {
        let asked = (style.weight, style.italic);
        if let Some(known) = self.cache.get(&style.font)
            && let Some((_, choice)) = known.iter().find(|(key, _)| *key == asked)
        {
            return choice.clone();
        }
        let choice = family_for(&fonts::index(), &style.font, style.weight, style.italic);
        self.cache
            .entry(style.font.clone())
            .or_default()
            .push((asked, choice.clone()));
        choice
    }
}

/// What a laid-out text tells its frame: its font is a stand-in, and which CJK scripts it needs a Noto Sans for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notes {
    stand_in: bool,
    /// A bit per `CjkScript::index`: a stretch of that script whose `kl:fallback:*` isn't registered.
    cjk_missing: u8,
}

impl Notes {
    /// The text draws a stand-in face (said once the scan has run).
    pub(crate) fn stand_in(&self) -> bool {
        self.stand_in
    }

    /// A bit per `CjkScript::index` whose Noto Sans isn't downloaded (scene text).
    pub(crate) fn cjk_missing(&self) -> u8 {
        self.cjk_missing
    }
}

/// The stretches `text` asks for, each with its family (empty: all of it asks for `choice`'s), and its notes. Text
/// without CJK takes the quick path: one family, nothing allocated for runs.
///
/// `scanning`: an installed font drawn while the scan of this computer's fonts still runs (the provisional font system
/// has none of them yet). It draws the stand-in for that moment, but says nothing: the scan, a moment later, has it.
/// A font still missing after the scan says so then.
pub(crate) fn runs_and_notes(
    text: &str,
    choice: &FaceChoice,
    language: &str,
    scanning: bool,
) -> (Runs, Notes) {
    let mut notes = Notes {
        stand_in: choice.stand_in && !scanning,
        cjk_missing: 0,
    };
    if !has_cjk(text) {
        return (Vec::new(), notes);
    }
    let runs = runs(text, &choice.family, language, choice.cjk);
    for script in CjkScript::ALL {
        let alias = klipolo_fonts::fallback::cjk_alias(script);
        if runs.iter().any(|(_, family)| &**family == alias) && !fonts::cjk_registered(script) {
            notes.cjk_missing |= 1 << script.index();
        }
    }
    (runs, notes)
}

/// What this frame has said already: each code once a frame.
#[derive(Default)]
pub(super) struct Said {
    font_missing: bool,
    cjk: u8,
}

impl Said {
    /// Says `notes` in `warnings` (`frame.fontMissing`, `frame.cjkFontMissing`), each code once a frame.
    pub(super) fn say(&mut self, notes: Notes, warnings: &mut Vec<String>) {
        if notes.stand_in && !self.font_missing {
            self.font_missing = true;
            warnings.push(FRAME_FONT_MISSING.into());
        }
        if notes.cjk_missing & !self.cjk != 0 {
            if self.cjk == 0 {
                warnings.push(FRAME_CJK_FONT_MISSING.into());
            }
            self.cjk |= notes.cjk_missing;
        }
    }
}
