//! A project's caption settings, checked with the rest of the project (`validate`): the style a well-formed caption
//! style (kept when this build doesn't have it), a look the style has, a colour, a font, places on the video and a
//! language tag.

use super::style::hex_ok;
use super::styles::{default_style, CaptionStyles};
use crate::contrib::{ContribId, ContribKind};
use crate::error::{invalid, Result};
use crate::font::font_ok;
use crate::messages::code;
use crate::project::Project;

fn check(ok: bool, code: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(invalid(code))
    }
}

/// A BCP 47 tag as the captions store it: 2–35 ASCII letters, digits and hyphens, starting with a letter.
fn language_ok(tag: &str) -> bool {
    (2..=35).contains(&tag.len())
        && tag.starts_with(|c: char| c.is_ascii_alphabetic())
        && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub(crate) fn validate_settings(p: &Project) -> Result<()> {
    let c = &p.captions;
    if let Some(id) = &c.style {
        crate::validate::check_contrib(id, ContribKind::CaptionStyle)?;
    }
    if let Some(variant) = &c.variant {
        // A pack's look on a style this build doesn't have is kept, as the style is.
        let id = c
            .style
            .clone()
            .unwrap_or_else(|| ContribId::from(default_style(&p.settings.formats)));
        if let Some(style) = CaptionStyles::builtin().get(&id) {
            check(
                style.variants.iter().any(|v| &v.key == variant),
                code!("model.captions.look"),
            )?;
        }
    }
    check(
        c.accent.as_deref().is_none_or(hex_ok),
        code!("model.captions.color"),
    )?;
    check(
        c.font.as_ref().is_none_or(font_ok),
        code!("model.captions.font"),
    )?;
    check(
        c.place
            .values()
            .all(|y| y.is_finite() && (0.05..=0.95).contains(y)),
        code!("model.captions.place"),
    )?;
    check(
        c.language.as_deref().is_none_or(language_ok),
        code!("model.captions.language"),
    )
}
