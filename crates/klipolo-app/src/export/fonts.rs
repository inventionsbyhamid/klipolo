//! The export's font check: the fonts the video uses that aren't on this Mac (`klipolo_fonts::missing`, the one
//! resolver's stand-in rule, so this, the engine and the webview agree). An export never draws a stand-in silently: it
//! is refused (`exports.fontsMissing`, naming them) unless stand-ins were allowed, which never covers Chinese, Japanese
//! or Korean text without its Noto Sans. Only fonts the export draws count: a title on a hidden track never stops one.

use klipolo_fonts::index::FontIndex;
use klipolo_fonts::missing::{MissingFont, missing_in, names};
use klipolo_model::coded::{CodedError, Said};
use klipolo_model::project::Project;

/// The fonts `project` uses that aren't here and the export draws, its text in `text_language`.
pub fn missing_fonts(
    index: &FontIndex,
    project: &Project,
    text_language: &str,
) -> Vec<MissingFont> {
    let mut all = missing_in(index, project, text_language);
    all.retain(MissingFont::drawn);
    all
}

/// Refuses an export of `missing` fonts unless stand-ins are allowed and none of them is CJK text, naming them.
pub fn refuse_missing(missing: &[MissingFont], allow_stand_ins: bool) -> Result<(), Said> {
    let covered = allow_stand_ins && missing.iter().all(|m| m.cjk.is_none());
    if missing.is_empty() || covered {
        return Ok(());
    }
    let names = names(missing.iter().map(|m| m.font.family.as_str()));
    Err(Said::Coded(
        CodedError::new("exports.fontsMissing").with("names", names),
    ))
}

#[cfg(test)]
mod tests {
    use klipolo_model::font::{FontRef, FontSource};
    use klipolo_model::layout::{Anchor, BoxN, Layout};
    use klipolo_model::text_language::CjkScript;
    use klipolo_model::timeline::{Item, ItemContent, TextStyle, Track, TrackKind};

    use super::*;

    /// A font on this Mac only (installed), by its family.
    fn installed(family: &str) -> FontRef {
        FontRef {
            family: family.into(),
            source: FontSource::Installed,
        }
    }

    /// `project` with a title in `font`, on a track that is `hidden` or not.
    fn titled(font: FontRef, hidden: bool) -> Project {
        let (mut p, _) = klipolo_engine::test_support::captions_project("caption.bold", &[]);
        p.captions.enabled = false;
        let mut titles = Track::new(TrackKind::Overlay, "Titles");
        titles.hidden = hidden;
        titles.items = vec![Item {
            id: "title".into(),
            start_us: 0,
            duration_us: 1_000_000,
            attach: None,
            layout: Layout::new(
                Anchor::TopLeft,
                BoxN {
                    x: 0.1,
                    y: 0.3,
                    w: 0.8,
                    h: 0.4,
                },
            ),
            content: ItemContent::Text {
                text: "A day at the studio".into(),
                style: TextStyle {
                    font,
                    ..TextStyle::default()
                },
            },
            effects: Vec::new(),
        }];
        p.timeline.tracks = vec![titles];
        p
    }

    fn shipped() -> FontIndex {
        FontIndex::shipped(klipolo_fonts::bundled::bundled_dir())
    }

    #[test]
    fn a_font_that_isnt_here_refuses_the_export_by_name() {
        let missing = missing_fonts(&shipped(), &titled(installed("Nowhere"), false), "en");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].font.family, "Nowhere");
        assert_eq!(
            refuse_missing(&missing, false),
            Err(Said::Coded(
                CodedError::new("exports.fontsMissing").with("names", "Nowhere")
            ))
        );
        assert_eq!(
            refuse_missing(&missing, true),
            Ok(()),
            "a stand-in when asked"
        );
        // Bundled fonts are always here, and a hidden track's title is never drawn.
        assert!(missing_fonts(&shipped(), &titled(FontRef::inter(), false), "en").is_empty());
        assert!(missing_fonts(&shipped(), &titled(installed("Nowhere"), true), "en").is_empty());
        assert_eq!(refuse_missing(&[], false), Ok(()));
    }

    #[test]
    fn cjk_text_without_its_font_is_never_drawn_with_a_stand_in() {
        let mut missing = missing_fonts(&shipped(), &titled(installed("Nowhere"), false), "en");
        missing[0].cjk = Some(CjkScript::Jp);
        assert!(refuse_missing(&missing, true).is_err());
    }
}
