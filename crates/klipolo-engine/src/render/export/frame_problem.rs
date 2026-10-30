//! Why an export stops at a frame it couldn't draw whole, as a code with the time (split from `render/export.rs`): too
//! many effects, a protection that couldn't run, too many large texts, a font that isn't here or CJK without its Noto
//! Sans, captions whose style isn't installed, or another note (in English).

/// What stops an export when a frame at `at_us` couldn't be drawn whole (`warning`, the frame's note), as a code with
/// the time: too many effects, a protection that couldn't run, too many large texts, or another note (in English).
pub(super) fn frame_problem(
    warning: &str,
    at_us: klipolo_model::time::Us,
) -> crate::api::CodedError {
    use crate::api::{
        CodedError, EXPORT_CAPTION_STYLE_MISSING, EXPORT_CJK_FONT_MISSING, EXPORT_FONT_MISSING,
        EXPORT_FRAME_NOT_DRAWN, EXPORT_PROTECTION_FAILED, EXPORT_SCENE_TEXT_MISSING,
        EXPORT_TOO_MANY_EFFECTS, EXPORT_TOO_MANY_TEXTS, FRAME_CAPTION_STYLE_MISSING,
        FRAME_CJK_FONT_MISSING, FRAME_EFFECTS_LEFT_OUT, FRAME_FONT_MISSING,
        FRAME_PROTECTION_HIDDEN, FRAME_SCENE_TEXT_MISSING, FRAME_TEXTS_SOFTER,
    };
    // The caption-style note names its style after the code (`api::caption_style_missing`).
    if let Some((FRAME_CAPTION_STYLE_MISSING, style)) = warning.split_once(' ') {
        return CodedError::new(EXPORT_CAPTION_STYLE_MISSING)
            .with("style", style)
            .with("atUs", at_us);
    }
    let code = match warning {
        FRAME_EFFECTS_LEFT_OUT => EXPORT_TOO_MANY_EFFECTS,
        FRAME_PROTECTION_HIDDEN => EXPORT_PROTECTION_FAILED,
        FRAME_TEXTS_SOFTER => EXPORT_TOO_MANY_TEXTS,
        FRAME_FONT_MISSING => EXPORT_FONT_MISSING,
        FRAME_CJK_FONT_MISSING => EXPORT_CJK_FONT_MISSING,
        FRAME_SCENE_TEXT_MISSING => EXPORT_SCENE_TEXT_MISSING,
        other => {
            return CodedError::new(EXPORT_FRAME_NOT_DRAWN)
                .with("reason", other)
                .with("atUs", at_us);
        }
    };
    CodedError::new(code).with("atUs", at_us)
}

/// Why an export stops at `plan`'s frame for a text in a font that isn't here (`FRAME_FONT_MISSING`), naming the
/// fonts the frame draws in a stand-in: its titles' and its captions', resolved in `index` as the drawing did.
/// Only when an export stops, so once per export at most.
pub(super) fn font_problem(
    plan: &crate::plan::FramePlan,
    index: &klipolo_fonts::index::FontIndex,
) -> crate::api::CodedError {
    use crate::plan::Layer;
    use klipolo_fonts::resolve::resolve;
    let mut families: Vec<String> = Vec::new();
    for layer in &plan.layers {
        let (font, weight, italic) = match layer {
            Layer::Text(t) => (&t.style.font, t.style.weight, t.style.italic),
            Layer::Caption(c) => (&*c.font, c.style.font.weight, c.style.font.italic),
            // A motion graphic's texts, as its drawer resolves them: its missing
            // font is named too, never the generic stop.
            Layer::Scene(s) => {
                for u in crate::render::scene::text_font_uses(&s.scene, &s.params, index) {
                    if resolve(index, &u.font, u.weight, u.italic).stand_in {
                        families.push(u.font.family);
                    }
                }
                continue;
            }
            _ => continue,
        };
        if resolve(index, font, weight, italic).stand_in {
            families.push(font.family.clone());
        }
    }
    if families.is_empty() {
        return frame_problem(crate::api::FRAME_FONT_MISSING, plan.at_us);
    }
    crate::api::CodedError::new(crate::api::EXPORT_FONT_MISSING_NAMED)
        .with(
            "names",
            klipolo_fonts::missing::names(families.iter().map(String::as_str)),
        )
        .with("atUs", plan.at_us)
}

#[cfg(test)]
mod tests {
    use crate::api::{
        CodedError, EXPORT_FRAME_NOT_DRAWN, EXPORT_PROTECTION_FAILED, EXPORT_TOO_MANY_EFFECTS,
        EXPORT_TOO_MANY_TEXTS, FRAME_EFFECTS_LEFT_OUT, FRAME_PROTECTION_HIDDEN, FRAME_TEXTS_SOFTER,
    };

    /// An export stopped by a frame it couldn't draw whole says, as a code, why and when.
    #[test]
    fn a_frame_that_couldnt_be_drawn_whole_names_its_cause_and_time() {
        let at = |code: &str, at_us: i64| CodedError::new(code).with("atUs", at_us);
        assert_eq!(
            super::frame_problem(FRAME_EFFECTS_LEFT_OUT, 83_000_000),
            at(EXPORT_TOO_MANY_EFFECTS, 83_000_000)
        );
        assert_eq!(
            super::frame_problem(FRAME_PROTECTION_HIDDEN, 5_000_000),
            at(EXPORT_PROTECTION_FAILED, 5_000_000)
        );
        assert_eq!(
            super::frame_problem(FRAME_TEXTS_SOFTER, 61_000_000),
            at(EXPORT_TOO_MANY_TEXTS, 61_000_000)
        );
        use crate::api::{
            EXPORT_CJK_FONT_MISSING, EXPORT_FONT_MISSING, FRAME_CJK_FONT_MISSING,
            FRAME_FONT_MISSING,
        };
        assert_eq!(
            super::frame_problem(FRAME_FONT_MISSING, 2_000_000),
            at(EXPORT_FONT_MISSING, 2_000_000)
        );
        assert_eq!(
            super::frame_problem(FRAME_CJK_FONT_MISSING, 3_000_000),
            at(EXPORT_CJK_FONT_MISSING, 3_000_000)
        );
        use crate::api::{EXPORT_CAPTION_STYLE_MISSING, caption_style_missing};
        assert_eq!(
            super::frame_problem(
                &caption_style_missing("com.acme.pack/caption.neon"),
                4_000_000
            ),
            CodedError::new(EXPORT_CAPTION_STYLE_MISSING)
                .with("style", "com.acme.pack/caption.neon")
                .with("atUs", 4_000_000i64)
        );
        let other = super::frame_problem("A LUT couldn't be read.", 1_000_000);
        assert_eq!(other.code, EXPORT_FRAME_NOT_DRAWN);
    }

    /// A frame stopped for a font that isn't here names the font, read from the frame itself.
    #[test]
    fn a_frame_stopped_for_a_missing_font_names_it() {
        use std::sync::Arc;

        use klipolo_model::font::{FontRef, FontSource};
        use klipolo_model::layout::{Anchor, BoxN, Layout};
        use klipolo_model::project::{FormatKind, Project};
        use klipolo_model::timeline::{Item, ItemContent, TextStyle, Track, TrackKind};

        let mut p = Project::new("Fonts");
        let mut titles = Track::new(TrackKind::Overlay, "Titles");
        let title = |id: &str, family: &str, source: FontSource| Item {
            id: id.into(),
            start_us: 0,
            duration_us: 1_000_000,
            attach: None,
            layout: Layout::new(
                Anchor::TopLeft,
                BoxN {
                    x: 0.1,
                    y: 0.3,
                    w: 0.8,
                    h: 0.2,
                },
            ),
            content: ItemContent::Text {
                text: "Hello".into(),
                style: TextStyle {
                    font: FontRef {
                        family: family.into(),
                        source,
                    },
                    ..TextStyle::default()
                },
            },
            effects: Vec::new(),
        };
        titles.items = vec![title("a", "Nowhere Sans", FontSource::Installed)];
        let mut more = Track::new(TrackKind::Overlay, "More");
        more.items = vec![title("b", "Inter", FontSource::Bundled)];
        p.timeline.tracks = vec![titles, more];
        let ctx = crate::plan::PlanContext::new(Arc::new(p));
        let plan = crate::plan::frame_plan(&ctx, 0, FormatKind::Landscape);
        let index = klipolo_fonts::index::FontIndex::shipped(klipolo_fonts::bundled::bundled_dir());
        assert_eq!(
            super::font_problem(&plan, &index),
            CodedError::new(crate::api::EXPORT_FONT_MISSING_NAMED)
                .with("names", "Nowhere Sans")
                .with("atUs", 0i64)
        );
    }
}
