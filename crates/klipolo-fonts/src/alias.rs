//! The family names faces are registered under, in the engine's font database and the webview's `FontFace`s alike.
//! Klipolo's own copies get `kl:` aliases, so they never collide with an installed family of the same name (an installed
//! Inter of another version draws differently), nor one Google version with another.

use klipolo_model::font::{FontRef, FontSource};

/// What a missing font draws in (at its nearest weight), said as `frame.fontMissing`.
pub const STAND_IN_FAMILY: &str = "Inter";
/// The Latin fallback alias: Inter's nearest face at every weight and style, for letters the text's own face lacks.
pub const LATIN_FALLBACK: &str = "kl:fallback:latin";

/// `Playfair Display` → `playfair-display`: lower case, words joined by dashes.
pub fn slug(family: &str) -> String {
    family
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

/// `kl:b:<slug>` for a bundled family, `kl:g:<slug>:<version>` for a Google one, an installed family's own name.
pub fn alias(font: &FontRef) -> String {
    match &font.source {
        FontSource::Bundled => format!("kl:b:{}", slug(&font.family)),
        FontSource::Google { version } => format!("kl:g:{}:{version}", slug(&font.family)),
        FontSource::Installed => font.family.clone(),
    }
}
