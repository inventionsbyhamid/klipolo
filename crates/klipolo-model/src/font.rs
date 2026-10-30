//! Which face a text uses: the family and where it comes from. A font installed on this Mac never
//! stands in for Klipolo's copy of the same family, nor the other way round; a Google family names its version, the
//! folder its files are kept in. The weight and style are the text's own (`TextStyle`); `klipolo_fonts::resolve` picks
//! the face that exists.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FontRef {
    pub family: String,
    pub source: FontSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum FontSource {
    /// Ships with Klipolo (`assets/fonts`).
    Bundled,
    /// Google Fonts, downloaded on the person's pick: `version` is the catalogue's (`v31`).
    Google { version: String },
    /// On this computer only.
    Installed,
}

impl FontRef {
    pub fn bundled(family: &str) -> Self {
        Self {
            family: family.into(),
            source: FontSource::Bundled,
        }
    }

    /// The model's default text font.
    pub fn inter() -> Self {
        Self::bundled("Inter")
    }
}

/// Whether the family is one line of 1 to 100 characters (not only spaces) without control characters.
pub fn family_ok(family: &str) -> bool {
    let n = family.chars().count();
    (1..=100).contains(&n) && !family.chars().any(char::is_control) && !family.trim().is_empty()
}

/// One line of 1 to 100 characters without control characters; a Google version is `v` and digits.
pub fn font_ok(f: &FontRef) -> bool {
    let version = match &f.source {
        FontSource::Google { version } => {
            version.len() >= 2
                && version.starts_with('v')
                && version[1..].bytes().all(|b| b.is_ascii_digit())
        }
        _ => true,
    };
    family_ok(&f.family) && version
}
