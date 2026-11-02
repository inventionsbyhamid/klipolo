//! The one fallback chain: the engine's cosmic-text `Fallback` and every CSS stack read it (exported
//! to `apps/desktop/src/lib/font-fallbacks.json`). Fallback families are registered at all nine weights in both styles
//! (each the nearest real file), so cosmic-text's exact-weight lookups always find them.
//!
//! Indic, Arabic, Hebrew and Thai letters a text's face lacks draw in the bundled Noto Sans of their script
//! (`NotoScript`), the same on every computer, in the preview and in every export: never a system face, which the
//! export computer may not have.

use klipolo_model::text_language::CjkScript;
use klipolo_platform::caps::OsFamily;
use klipolo_platform::fonts::emoji_family_of;
use serde::Serialize;
use unicode_script::Script;

pub const LATIN: &str = crate::alias::LATIN_FALLBACK;
/// Adobe NotDef's alias: the boxes the engine and the webview both draw for CJK before its Noto Sans is downloaded.
pub const NOTDEF: &str = "kl:fallback:notdef";

/// A fallback alias, as the resolver and the webview's fallback key name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Fallback {
    Latin,
    Cjk(CjkScript),
    /// Adobe NotDef, the last CJK fallback in the engine and the webview.
    Notdef,
    /// A script's bundled Noto Sans.
    Script(NotoScript),
}

impl Fallback {
    pub fn alias(self) -> &'static str {
        match self {
            Fallback::Latin => LATIN,
            Fallback::Cjk(s) => cjk_alias(s),
            Fallback::Notdef => NOTDEF,
            Fallback::Script(s) => s.alias(),
        }
    }
}

/// A script whose letters draw in a bundled Noto Sans when the text's face lacks them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NotoScript {
    /// Hindi, Marathi, Nepali, Sanskrit.
    Devanagari,
    /// Bengali, Assamese.
    Bengali,
    /// Punjabi.
    Gurmukhi,
    Gujarati,
    Tamil,
    Telugu,
    Kannada,
    Malayalam,
    /// Arabic, Persian, Urdu: drawn right to left.
    Arabic,
    /// Drawn right to left.
    Hebrew,
    Thai,
}

impl NotoScript {
    pub const ALL: [NotoScript; 11] = [
        NotoScript::Devanagari,
        NotoScript::Bengali,
        NotoScript::Gurmukhi,
        NotoScript::Gujarati,
        NotoScript::Tamil,
        NotoScript::Telugu,
        NotoScript::Kannada,
        NotoScript::Malayalam,
        NotoScript::Arabic,
        NotoScript::Hebrew,
        NotoScript::Thai,
    ];

    /// The bundled family (`bundled::BUNDLED`'s, a `Fallback` one: no menu lists it).
    pub fn family(self) -> &'static str {
        match self {
            NotoScript::Devanagari => "Noto Sans Devanagari",
            NotoScript::Bengali => "Noto Sans Bengali",
            NotoScript::Gurmukhi => "Noto Sans Gurmukhi",
            NotoScript::Gujarati => "Noto Sans Gujarati",
            NotoScript::Tamil => "Noto Sans Tamil",
            NotoScript::Telugu => "Noto Sans Telugu",
            NotoScript::Kannada => "Noto Sans Kannada",
            NotoScript::Malayalam => "Noto Sans Malayalam",
            NotoScript::Arabic => "Noto Sans Arabic",
            NotoScript::Hebrew => "Noto Sans Hebrew",
            NotoScript::Thai => "Noto Sans Thai",
        }
    }

    /// The alias its letters fall back to, registered at every weight and style.
    pub fn alias(self) -> &'static str {
        match self {
            NotoScript::Devanagari => "kl:fallback:devanagari",
            NotoScript::Bengali => "kl:fallback:bengali",
            NotoScript::Gurmukhi => "kl:fallback:gurmukhi",
            NotoScript::Gujarati => "kl:fallback:gujarati",
            NotoScript::Tamil => "kl:fallback:tamil",
            NotoScript::Telugu => "kl:fallback:telugu",
            NotoScript::Kannada => "kl:fallback:kannada",
            NotoScript::Malayalam => "kl:fallback:malayalam",
            NotoScript::Arabic => "kl:fallback:arabic",
            NotoScript::Hebrew => "kl:fallback:hebrew",
            NotoScript::Thai => "kl:fallback:thai",
        }
    }

    /// The bundled Noto Sans for the letters of `script`, if there is one.
    pub fn of(script: Script) -> Option<NotoScript> {
        Some(match script {
            Script::Devanagari => NotoScript::Devanagari,
            Script::Bengali => NotoScript::Bengali,
            Script::Gurmukhi => NotoScript::Gurmukhi,
            Script::Gujarati => NotoScript::Gujarati,
            Script::Tamil => NotoScript::Tamil,
            Script::Telugu => NotoScript::Telugu,
            Script::Kannada => NotoScript::Kannada,
            Script::Malayalam => NotoScript::Malayalam,
            Script::Arabic => NotoScript::Arabic,
            Script::Hebrew => NotoScript::Hebrew,
            Script::Thai => NotoScript::Thai,
            _ => return None,
        })
    }
}

/// The Google family a script's CJK fallback is downloaded as: the only CJK fallbacks there are.
pub fn cjk_family(s: CjkScript) -> &'static str {
    match s {
        CjkScript::Sc => "Noto Sans SC",
        CjkScript::Tc => "Noto Sans TC",
        CjkScript::Jp => "Noto Sans JP",
        CjkScript::Kr => "Noto Sans KR",
    }
}

/// The script whose CJK fallback `family` is (`cjk_family`'s inverse).
pub fn cjk_script_of(family: &str) -> Option<CjkScript> {
    CjkScript::ALL
        .into_iter()
        .find(|&s| cjk_family(s) == family)
}

/// The alias of a script's downloaded Noto Sans (registered only once it is downloaded).
pub fn cjk_alias(s: CjkScript) -> &'static str {
    match s {
        CjkScript::Sc => "kl:fallback:sc",
        CjkScript::Tc => "kl:fallback:tc",
        CjkScript::Jp => "kl:fallback:jp",
        CjkScript::Kr => "kl:fallback:kr",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chain {
    pub common: [&'static str; 1],
    pub cjk: [(CjkScript, &'static str); 4],
    pub notdef: &'static str,
    /// The webview registers NotDef with this `unicode-range` (`cjk::cjk_ranges_css()`), so it boxes only CJK; other
    /// scripts keep the system's fallback in both the engine and the webview.
    pub cjk_ranges: String,
    /// Each OS's colour emoji face, last in the chain. The file is the same for every build (generated on any
    /// machine); each takes its own OS's (`klipolo_platform::fonts::emoji_family` in the engine).
    pub emoji: Emoji,
}

#[derive(Serialize)]
pub struct Emoji {
    pub macos: &'static str,
    pub windows: &'static str,
    pub linux: &'static str,
}

pub fn chain() -> Chain {
    use CjkScript::*;
    Chain {
        common: [LATIN],
        cjk: [
            (Sc, cjk_alias(Sc)),
            (Tc, cjk_alias(Tc)),
            (Jp, cjk_alias(Jp)),
            (Kr, cjk_alias(Kr)),
        ],
        notdef: NOTDEF,
        cjk_ranges: crate::cjk::cjk_ranges_css(),
        emoji: Emoji {
            macos: emoji_family_of(OsFamily::Macos),
            windows: emoji_family_of(OsFamily::Windows),
            linux: emoji_family_of(OsFamily::Linux),
        },
    }
}
