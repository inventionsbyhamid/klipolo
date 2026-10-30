//! Klipolo's fallback chain for cosmic-text (`klipolo_fonts::fallback`). Letters the
//! text's face lacks come from the Latin fallback (Inter) at the same weight and style, then the system's emoji face.
//!
//! - **CJK** (Han, kana, Hangul, Bopomofo) falls back only to Adobe NotDef's boxes. The script runs
//!   (`script_runs::runs`) already ask for the language's own Noto Sans; NotDef is all that's left after it, in the
//!   engine as in the webview (never another language's Noto, never a system face).
//! - **Other scripts** (Thai, Arabic…) keep the system's script fallbacks and never reach NotDef: it is on the
//!   forbidden list, which cosmic-text's last stage (any face not on it) reads and its script stage doesn't.
//! - **Installed CJK families** (PingFang, Hiragino, Apple SD Gothic Neo, hidden ones too) join the forbidden list once
//!   the scan has found them (`forbid_installed_cjk`), so the last stage never borrows them for CJK the
//!   text didn't pick them for. Such a family still draws when it is the text's own font: the list only bars fallback.
//!   cosmic-text reads the list when it builds a font system (`Fallbacks::new`), and every font system with installed
//!   faces in it is built after the scan.

use std::sync::OnceLock;

use glyphon::cosmic_text::{Fallback, PlatformFallback};
use klipolo_fonts::coverage::CJK;
use klipolo_fonts::fallback::{LATIN, NOTDEF};
use klipolo_fonts::index::InstalledFamily;
use klipolo_fonts::installed::NORMAL_WIDTH;
use klipolo_fonts::unicode_script::Script;

use crate::render::fonts::width_alias;

/// The system's lists for scripts Klipolo leaves to it.
static PLATFORM: PlatformFallback = PlatformFallback;
/// The forbidden list once the scan has found this computer's CJK families (their names leaked once: one scan a
/// process).
static WITH_INSTALLED: OnceLock<Vec<&'static str>> = OnceLock::new();

/// The platform's forbidden families, plus Adobe NotDef.
fn before_the_scan() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut list = PLATFORM.forbidden_fallback().to_vec();
        list.push(NOTDEF);
        list
    })
}

/// The installed families covering Han, kana or Hangul: never a fallback. Their names in the font database
/// (`alias`), each once, and their faces' width twins' (`fonts::width_alias`: a condensed CJK face is asked at normal
/// width under its own alias).
pub fn cjk_forbidden(families: &[InstalledFamily]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for f in families.iter().filter(|f| f.coverage & CJK != 0) {
        let twins = f
            .faces
            .iter()
            .filter(|face| face.stretch != NORMAL_WIDTH)
            .map(|face| width_alias(&f.alias, face.stretch));
        for name in std::iter::once(f.alias.clone()).chain(twins) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Puts the installed CJK families named `names` (`cjk_forbidden`'s) on the forbidden list of every font system built
/// from now on. The scan calls it once (or an export worker, with the app's scan); a later call changes
/// nothing.
pub(crate) fn forbid_installed_cjk(names: &[String]) {
    WITH_INSTALLED.get_or_init(|| {
        let mut list = before_the_scan().to_vec();
        list.extend(
            names
                .iter()
                .map(|name| &*Box::leak(name.clone().into_boxed_str())),
        );
        list
    });
}

pub(crate) struct KlipoloFallback;

impl Fallback for KlipoloFallback {
    fn common_fallback(&self) -> &[&'static str] {
        static COMMON: OnceLock<[&'static str; 2]> = OnceLock::new();
        COMMON.get_or_init(|| [LATIN, klipolo_platform::fonts::emoji_family()])
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        WITH_INSTALLED
            .get()
            .map_or_else(before_the_scan, Vec::as_slice)
    }

    fn script_fallback(&self, script: Script, locale: &str) -> &[&'static str] {
        match script {
            Script::Han
            | Script::Hiragana
            | Script::Katakana
            | Script::Hangul
            | Script::Bopomofo => &[NOTDEF],
            _ => PLATFORM.script_fallback(script, locale),
        }
    }
}
