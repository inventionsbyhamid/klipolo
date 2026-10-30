//! A caption page fits its frame: a long word is never cut at the frame's edge, the shadow
//! and the glow keep their size to the text at every resolution (the preview draws what the export does), a hidden word
//! keeps the person's new line, and Arcade's seam is where a portrait split's halves meet.

mod support;

use std::sync::Arc;

use klipolo_engine::plan::clip::split_seam;
use klipolo_engine::render::captions::layout::{PageLayout, lay_out_page};
use klipolo_engine::render::captions::{blur_sigmas, test_font_system};
use klipolo_engine::render::fonts::{configure_for_tests, family_for, index};
use klipolo_model::captions::Keyword;
use klipolo_model::captions::looks::{Look, WordLook};
use klipolo_model::captions::pages::Pages;
use klipolo_model::captions::style::CaptionStyleDef;
use klipolo_model::captions::styles::CaptionStyles;
use klipolo_model::contrib::ContribId;
use klipolo_model::font::FontRef;
use klipolo_model::project::{CaptionSettings, FormatKind};
use klipolo_model::timeline::{CameraLayout, CameraMode};
use support::{ctx, project, secs};

/// One page of `words`, shown as given.
fn page(words: &[&str]) -> Pages {
    let timed: Vec<(&str, i64, i64)> = (0i64..)
        .zip(words)
        .map(|(i, w)| (*w, i * 300_000, i * 300_000 + 250_000))
        .collect();
    Pages::one_page("v", 0, timed.last().unwrap().2, &timed)
}

fn style(key: &str) -> Arc<CaptionStyleDef> {
    let settings = CaptionSettings {
        enabled: true,
        style: Some(ContribId::builtin(key)),
        ..CaptionSettings::default()
    };
    CaptionStyles::builtin()
        .for_settings(&settings, &[FormatKind::Portrait])
        .0
}

/// `pages`' first page in `style` for a 1080 × 1920 frame, its lines at most 0.70 of the width.
fn lay_out(style: &CaptionStyleDef, pages: &Pages) -> PageLayout {
    configure_for_tests();
    let font = &style.font;
    let face = family_for(
        &index(),
        &FontRef::bundled(&font.family),
        font.weight,
        font.italic,
    );
    let mut fonts = test_font_system();
    let out = (1080, 1920);
    lay_out_page(
        fonts.system(),
        style,
        &face,
        "en",
        pages,
        0,
        FormatKind::Portrait,
        out,
        0.70,
    )
}

/// The block's width with its outline on both sides, px.
fn drawn_width(style: &CaptionStyleDef, layout: &PageLayout) -> f32 {
    let biggest = layout.words.iter().map(|w| w.scale).fold(1.0, f32::max);
    let em = layout.font_px * layout.autofit * biggest;
    layout.block[0] + 2.0 * style.outline.as_ref().map_or(0.0, |o| o.width_em as f32) * em
}

#[test]
fn a_long_word_shrinks_past_autofit_min_only_to_stay_inside_the_frame() {
    let bold = style("caption.bold");
    let least = bold.autofit_min as f32;
    let inside = |layout: &PageLayout, what: &str| {
        let w = drawn_width(&bold, layout);
        assert!(
            w <= 1080.0 * 0.96 + 0.5,
            "{what}: {w} px in a 1080-px frame"
        );
        assert!(
            layout.autofit < least,
            "{what}: below the minimum only when the frame needs it"
        );
    };
    // A creator's handle of 24 capitals.
    inside(
        &lay_out(&bold, &page(&["@THEFITNESSCOACHDAILYVLOG"])),
        "the handle",
    );
    // A keyword (×1.12).
    let mut keyword = page(&["UNBELIEVABLEFITNESS"]);
    keyword.keywords[0] = Some(Keyword::Plain);
    inside(&lay_out(&bold, &keyword), "the keyword");
    // The person's biggest size step (+3, ×1.52).
    let mut big = page(&["TRANSFORMATION"]);
    big.looks = Arc::new(vec![WordLook {
        asset_id: "v".into(),
        start_us: 0,
        end_us: 250_000,
        look: Look {
            size: Some(3),
            ..Look::default()
        },
    }]);
    Arc::make_mut(&mut big.words)[0].look = 0;
    inside(&lay_out(&bold, &big), "the +3 word");
    // A page that fits keeps its size.
    assert_eq!(lay_out(&bold, &page(&["MAKE", "IT", "POP"])).autofit, 1.0);
}

/// A frame-bounded word in Arcade, with the ad-safe switch on (its centre at 504 of
/// 1080 px), stays inside the frame while it is the word being said (×1.08), its outline included.
#[test]
fn an_ad_safe_word_at_its_active_scale_stays_inside_the_frame() {
    use klipolo_engine::plan::captions::place::{Anchor, active_scale};

    let arcade = style("caption.arcade");
    let layout = lay_out(&arcade, &page(&["@THEFITNESSCOACHDAILYVLOGGER"]));
    let w = f64::from(drawn_width(&arcade, &layout));
    let anchor = Anchor {
        seam: None,
        place: None,
        ad_safe: true,
    };
    let (x, _) = anchor.centre(
        &arcade,
        FormatKind::Portrait,
        (w / 1080.0, f64::from(layout.block[1]) / 1920.0),
    );
    let half = w / 2.0 * active_scale(&arcade);
    let (left, right) = (x * 1080.0 - half, x * 1080.0 + half);
    assert!(
        left >= -0.5 && right <= 1080.5,
        "{left:.1}..{right:.1} px in a 1080-px frame"
    );
    assert!(
        half * 2.0 <= 1080.0 * 0.96 + 0.5,
        "{:.1} px at ×1.08",
        half * 2.0
    );
    // A short page keeps the ad-safe centre.
    let (x, _) = anchor.centre(&arcade, FormatKind::Portrait, (0.3, 0.1));
    assert!((x - 504.0 / 1080.0).abs() < 1e-9);
}

#[test]
fn shadows_and_glows_keep_their_size_to_the_text_at_every_resolution() {
    for key in [
        "caption.arcade",
        "caption.loud",
        "caption.clean",
        "caption.bold",
    ] {
        let s = style(key);
        let size = s.size.size(FormatKind::Portrait) as f32;
        // The text's size in the 720 × 1280 preview, the 1080 × 1920 export and a 4K export.
        let ratios = [size * 720.0 / 1080.0, size, size * 2.0].map(|em| {
            let (shadow, glow) = blur_sigmas(&s, em);
            (shadow / em, glow.max(0.0) / em)
        });
        for (shadow, glow) in &ratios[1..] {
            assert!(
                (shadow - ratios[0].0).abs() < 1e-5,
                "{key}: shadow {ratios:?}"
            );
            assert!((glow - ratios[0].1).abs() < 1e-5, "{key}: glow {ratios:?}");
        }
    }
}

#[test]
fn a_hidden_word_hands_its_new_line_to_the_next_word() {
    let clean = style("caption.clean");
    let mut pages = page(&["one", "two", "three", "four"]);
    let words = Arc::make_mut(&mut pages.words);
    words[2].hidden = true;
    words[2].line_break = true;
    let layout = lay_out(&clean, &pages);
    assert_eq!(layout.words.len(), 3);
    assert_eq!(layout.lines.len(), 2, "the person's new line stays");
    assert_eq!(layout.words[2].line, 1, "\"four\" starts it");
}

#[test]
fn the_seam_is_where_a_portrait_splits_halves_meet() {
    let mut p = project();
    p.background.padding = 0.0;
    p.timeline.main[0].camera = Some(CameraLayout {
        mode: CameraMode::SplitBottom,
        ..CameraLayout::default()
    });
    let split = ctx(p.clone());
    assert_eq!(
        split_seam(&split, secs(1.0), FormatKind::Portrait),
        Some(0.5)
    );
    assert_eq!(split_seam(&split, secs(1.0), FormatKind::Landscape), None);
    assert_eq!(
        split_seam(&split, secs(25.0), FormatKind::Portrait),
        None,
        "a clip of a file"
    );
    // The default layout splits in 9:16; a bubble there doesn't.
    p.timeline.main[0].camera = Some(CameraLayout {
        mode: CameraMode::Bubble,
        mode_overrides: Default::default(),
        ..CameraLayout::default()
    });
    let bubble = ctx(p);
    assert_eq!(split_seam(&bubble, secs(1.0), FormatKind::Portrait), None);
}
