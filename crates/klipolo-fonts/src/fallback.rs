//! The one fallback chain: the engine's cosmic-text `Fallback` and every CSS stack read it (exported
//! to `src/lib/font-fallbacks.json`). Fallback families are registered at all nine weights in both styles (each the
//! nearest real file), so cosmic-text's exact-weight lookups always find them.

use klipolo_model::text_language::CjkScript;
use klipolo_platform::caps::OsFamily;
use klipolo_platform::fonts::emoji_family_of;
use serde::Serialize;

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
}

impl Fallback {
    pub fn alias(self) -> &'static str {
        match self {
            Fallback::Latin => LATIN,
            Fallback::Cjk(s) => cjk_alias(s),
            Fallback::Notdef => NOTDEF,
        }
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
