//! The captions' look of one project revision: the resolved style, its colours, the font it asks for and the
//! hash its page atlases are keyed by. Made once per change of the caption settings or the formats, never per frame.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use klipolo_model::captions::style::CaptionStyleDef;
use klipolo_model::captions::styles::CaptionStyles;
use klipolo_model::font::FontRef;
use klipolo_model::project::{CaptionSettings, FormatKind, Project};

use super::colors::StyleColors;

/// The captions of one project revision as drawn: the resolved style (its look and the person's colour applied), its
/// colours, and the font. A context made from a previous one keeps it while they stay (`keep_or_make`).
#[derive(Debug)]
pub struct CaptionLook {
    /// What it was resolved from.
    settings: CaptionSettings,
    formats: Vec<FormatKind>,
    pub style: Arc<CaptionStyleDef>,
    /// The style's id when this build doesn't have it and Clean stands in.
    pub missing: Option<Arc<str>>,
    pub colors: StyleColors,
    /// What the style's word masks are drawn from: its id and look and the person's size and case, never its colours (an
    /// atlas is white masks, so a colour change or a swatch hover keeps the atlases). Part of the drawer's atlas key.
    pub key: u64,
    /// The font the captions ask for: the person's caption font, else the style's own family (at the style's weight
    /// and style; the drawer resolves the face).
    pub font: Arc<FontRef>,
}

impl CaptionLook {
    pub fn of(p: &Project) -> Self {
        let (style, missing) =
            CaptionStyles::builtin().for_settings(&p.captions, &p.settings.formats);
        let variant = p.captions.variant.as_deref().filter(|_| !missing);
        let key = {
            let mut h = DefaultHasher::new();
            (style.id.as_str(), variant).hash(&mut h);
            // The masks are drawn at the words' size and in their case.
            (p.captions.size.map(f64::to_bits), p.captions.upper).hash(&mut h);
            h.finish()
        };
        let missing = missing
            .then(|| p.captions.style.as_ref().map(|id| Arc::from(id.as_str())))
            .flatten();
        let font = Arc::new(
            p.captions
                .font
                .clone()
                .unwrap_or_else(|| FontRef::bundled(&style.font.family)),
        );
        Self {
            settings: p.captions.clone(),
            formats: p.settings.formats.clone(),
            colors: StyleColors::of(&style),
            style,
            missing,
            key,
            font,
        }
    }

    /// `previous` while `p`'s caption settings and formats are the ones it was made for, else `p`'s own.
    pub(crate) fn keep_or_make(
        previous: Option<&Arc<CaptionLook>>,
        p: &Project,
    ) -> Arc<CaptionLook> {
        match previous {
            Some(look) if look.settings == p.captions && look.formats == p.settings.formats => {
                look.clone()
            }
            _ => Arc::new(Self::of(p)),
        }
    }
}

#[cfg(test)]
mod tests {
    use klipolo_model::project::{CaptionSettings, Project};

    use super::CaptionLook;

    /// The word masks are drawn at the words' size and in their case, so a size or a case is part of the atlas key; the
    /// words' colour isn't (the masks are white).
    #[test]
    fn a_size_or_case_changes_the_atlas_key_and_a_colour_doesnt() {
        let p = Project::new("Look");
        let key = |captions: CaptionSettings| {
            CaptionLook::of(&Project {
                captions,
                ..p.clone()
            })
            .key
        };
        let mut c = CaptionSettings {
            enabled: true,
            ..CaptionSettings::default()
        };
        let own = key(c.clone());
        c.color = Some("#FFD02E".into());
        assert_eq!(key(c.clone()), own);
        c.size = Some(1.3);
        let bigger = key(c.clone());
        assert_ne!(bigger, own);
        c.upper = Some(true);
        assert_ne!(key(c), bigger);
    }
}
