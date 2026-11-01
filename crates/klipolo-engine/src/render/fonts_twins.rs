//! Two more kinds of record (`managed` registers them with the rest), each a face record:
//! - **width twins.** An installed face that isn't normal width (Impact, Futura Condensed) is registered at normal
//!   width as well, under its own alias (`width_alias`), and `family_for` asks for that: cosmic-text's script and
//!   common fallbacks and its last stage only see faces of the asked width, and the system's script faces are normal
//!   width, so a condensed title's Thai, Arabic or symbols would otherwise draw as boxes. Its glyphs are the face's own.
//!   A twin of an installed CJK family is on the forbidden-fallback list with the family (`cjk_forbidden`);
//! - **the emoji face at every weight.** cosmic-text matches the emoji face at any width and style (its PostScript name
//!   says "Emoji") but only at the asked weight: with the face at 400 only, an emoji in a 900 title would make it load
//!   the computer's other fonts one by one (its last stage) until it reached the emoji face. With a record at every
//!   weight a fallback has, its common fallback finds it at once; each record maps the same file, which fontdb
//!   memory-maps once.

use klipolo_fonts::index::FontIndex;
use klipolo_fonts::installed::NORMAL_WIDTH;

use super::managed::ManagedFace;

/// The alias a face of width class `stretch` of the installed family registered as `alias` is asked by at normal
/// width (its width twin).
pub(crate) fn width_alias(alias: &str, stretch: u16) -> String {
    format!("kl:w:{alias}:{stretch}")
}

/// Normal-width copies of the installed faces that aren't normal width, under `width_alias`; upright too for a family
/// whose faces are all italic (as `upright_twins`).
pub(super) fn width_twins(index: &FontIndex) -> Vec<ManagedFace> {
    let mut twins = Vec::new();
    for family in index.installed() {
        let italic_only = family.faces.iter().all(|f| f.italic);
        for f in family.faces.iter().filter(|f| f.stretch != NORMAL_WIDTH) {
            let twin = ManagedFace {
                alias: width_alias(&family.alias, f.stretch),
                weight: f.weight,
                italic: f.italic,
                stretch: NORMAL_WIDTH,
                path: f.path.clone(),
                index: f.index,
            };
            if italic_only {
                twins.push(ManagedFace {
                    italic: false,
                    ..twin.clone()
                });
            }
            twins.push(twin);
        }
    }
    twins
}

/// The computer's emoji face (`klipolo_platform::fonts::emoji_family`) at each of `weights` it doesn't have itself,
/// under its own name (so the fallback chain's name finds it), upright and normal width.
pub(super) fn emoji_weights(index: &FontIndex, weights: &[u16]) -> Vec<ManagedFace> {
    let Some(family) = index.installed_family(klipolo_platform::fonts::emoji_family()) else {
        return Vec::new();
    };
    let Some(face) = family
        .faces
        .iter()
        .find(|f| !f.italic)
        .or(family.faces.first())
    else {
        return Vec::new();
    };
    weights
        .iter()
        .filter(|&&w| !family.faces.iter().any(|f| f.weight == w))
        .map(|&weight| ManagedFace {
            alias: family.alias.clone(),
            weight,
            italic: false,
            stretch: NORMAL_WIDTH,
            path: face.path.clone(),
            index: face.index,
        })
        .collect()
}
