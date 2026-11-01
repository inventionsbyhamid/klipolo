//! A caption page's layout: its lines (the model's pure breaker, shared with the caption files), where its block sits,
//! a word wider than the line, and the faces the renderer shapes with.

use klipolo_engine::plan::captions::place::{block_centre, max_width_share};
use klipolo_model::captions::lines::{LineInput, break_lines};
use klipolo_model::captions::styles::CaptionStyles;
use klipolo_model::contrib::ContribId;
use klipolo_model::project::{FormatKind, Project};

fn lines(widths: &[f32], max: f32, max_lines: usize, short: &[bool], pyramid: bool) -> Vec<u8> {
    let forced = vec![false; widths.len()];
    break_lines(&LineInput {
        widths,
        space: 10.0,
        max_width: max,
        max_lines,
        forced: &forced,
        short,
        pyramid,
    })
    .starts
    .to_vec()
}

#[test]
fn the_fewest_lines_that_fit_then_the_most_balanced() {
    // Fits on one line: one line.
    assert_eq!(
        lines(&[100.0, 100.0], 300.0, 2, &[false, false], false),
        [0]
    );
    // Needs two: the split that keeps the widest line narrowest.
    assert_eq!(
        lines(&[200.0, 100.0, 100.0, 200.0], 400.0, 2, &[false; 4], false),
        [0, 2]
    );
}

#[test]
fn a_short_word_is_never_left_alone_on_a_line() {
    // Balance alone would put "WOW" alone on line 2 (widest 350 against 370), but a lone word of three letters or fewer
    // is avoided; without the short flag, balance wins.
    let short = [false, false, true];
    assert_eq!(lines(&[40.0, 300.0, 60.0], 400.0, 2, &short, false), [0, 1]);
    assert_eq!(
        lines(&[40.0, 300.0, 60.0], 400.0, 2, &[false; 3], false),
        [0, 2]
    );
}

#[test]
fn clean_prefers_a_bottom_heavy_pyramid() {
    // Balance alone puts the wider line on top (210 over 112); Clean's pyramid puts it below (100 over 222).
    let widths = [100.0, 100.0, 112.0];
    assert_eq!(lines(&widths, 230.0, 2, &[false; 3], true), [0, 1]);
    assert_eq!(lines(&widths, 230.0, 2, &[false; 3], false), [0, 2]);
}

#[test]
fn a_page_with_fewer_words_than_lines_still_breaks_them() {
    // Two words too wide for one line, in a style allowing three lines: two lines, never one 810-px line.
    assert_eq!(lines(&[400.0, 400.0], 300.0, 3, &[false; 2], false), [0, 1]);
}

#[test]
fn forced_breaks_are_kept_and_count_toward_the_lines() {
    let widths = [50.0, 50.0, 50.0];
    let forced = [false, false, true];
    let b = break_lines(&LineInput {
        widths: &widths,
        space: 10.0,
        max_width: 500.0,
        max_lines: 2,
        forced: &forced,
        short: &[false; 3],
        pyramid: false,
    });
    assert_eq!(b.starts.to_vec(), [0, 2]);
}

#[test]
fn nothing_fits_gives_the_narrowest_at_the_most_lines_for_autofit() {
    let b = break_lines(&LineInput {
        widths: &[400.0, 400.0],
        space: 10.0,
        max_width: 300.0,
        max_lines: 2,
        forced: &[false; 2],
        short: &[false; 2],
        pyramid: false,
    });
    assert_eq!((b.starts.to_vec(), b.widest), (vec![0, 1], 400.0));
}

fn project(style: &str) -> Project {
    let mut p = Project::new("p");
    p.captions.enabled = true;
    p.captions.style = Some(ContribId::builtin(style));
    p
}

#[test]
fn a_placed_block_keeps_its_place_and_a_default_one_stays_organic_safe() {
    let mut p = project("caption.bold");
    let style = CaptionStyles::builtin()
        .for_settings(&p.captions, &p.settings.formats)
        .0;
    // Bold portrait default 0.62 with a block 0.2 tall: inside 0.15–0.72.
    let (x, y) = block_centre(&p, &style, FormatKind::Portrait, 0.2, None);
    assert_eq!(x, 0.5);
    assert!(y + 0.1 <= 0.72 + 1e-9 && y - 0.1 >= 0.15 - 1e-9, "{y}");
    p.captions.place.insert(FormatKind::Portrait, 0.9);
    assert_eq!(
        block_centre(&p, &style, FormatKind::Portrait, 0.2, None).1,
        0.9,
        "the person's own place is never moved"
    );
    p.captions.place.clear();
    p.captions.ad_safe = true;
    let (x, y) = block_centre(&p, &style, FormatKind::Portrait, 0.2, None);
    assert!(y + 0.1 <= 0.65 + 1e-9);
    assert!(
        (x - 504.0 / 1080.0).abs() < 1e-9,
        "ad-safe centres between 120 and 888 px"
    );
    assert!(max_width_share(&p, &style, FormatKind::Portrait) <= 768.0 / 1080.0 + 1e-9);
}

#[test]
fn arcade_sits_at_the_split_seam_in_portrait() {
    let p = project("caption.arcade");
    let style = CaptionStyles::builtin()
        .for_settings(&p.captions, &p.settings.formats)
        .0;
    assert_eq!(
        block_centre(&p, &style, FormatKind::Portrait, 0.1, Some(0.4)).1,
        0.4
    );
    assert_eq!(
        block_centre(&p, &style, FormatKind::Portrait, 0.1, None).1,
        0.58
    );
}

#[test]
fn a_word_wider_than_the_line_autofits_and_is_never_cut() {
    klipolo_engine::render::fonts::configure_for_tests();
    let (p, sp) = klipolo_engine::test_support::captions_project(
        "caption.loud",
        &[("Supercalifragilistic", 0.0, 0.8)],
    );
    let caps = klipolo_model::captions::captions(&p, &sp);
    let pages = &caps.pages[&FormatKind::Portrait];
    let mut fonts = klipolo_engine::render::captions::test_font_system();
    let style = CaptionStyles::builtin()
        .for_settings(&p.captions, &p.settings.formats)
        .0;
    let face = klipolo_engine::render::fonts::family_for(
        &klipolo_engine::render::fonts::index(),
        &klipolo_model::font::FontRef::bundled(&style.font.family),
        style.font.weight,
        style.font.italic,
    );
    assert_eq!(face.family, "kl:b:bangers");
    let layout = klipolo_engine::render::captions::layout::lay_out_page(
        fonts.system(),
        &style,
        &face,
        "en",
        pages,
        0,
        FormatKind::Portrait,
        (1080, 1920),
        0.70,
    );
    assert!(
        layout.autofit < 1.0 && layout.autofit >= style.autofit_min as f32 - 1e-6,
        "{}",
        layout.autofit
    );
    let w = &layout.words[0];
    assert!(w.size[0] > 0.0, "drawn, at whatever width it takes");
    // Never broken: one word, one line, its cell holding all of it.
    assert_eq!(layout.lines.len(), 1);
    assert!(w.cell[2] as f32 >= w.size[0] + 2.0 * layout.pad - 1.0);
}

/// An emoji in a heavy caption finds the emoji face at the caption's own weight at once (cosmic-text no longer loads
/// the computer's fonts one by one to reach it), and a condensed face keeps the system's script faces, so its Thai
/// never draws as boxes.
#[test]
fn an_emoji_at_900_finds_its_face_and_a_condensed_face_keeps_the_script_fallbacks() {
    use klipolo_engine::render::fonts::{configure_for_tests, test_index, test_installed_shape};
    use klipolo_model::font::{FontRef, FontSource};
    configure_for_tests();
    let index = test_index();
    let dir = index.bundled_dir().unwrap();
    // Adobe NotDef's file has a glyph for every character: it stands in for this computer's emoji and Thai faces.
    let any = dir.join("AdobeNotDef/AND-Regular.ttf");
    let emoji = klipolo_platform::fonts::emoji_family();
    let (_, drawn, weight) = test_installed_shape(
        &[(emoji, any.clone(), 400, 5)],
        &FontRef::bundled("Montserrat"),
        900,
        "😀",
    );
    assert_eq!(
        (drawn.as_str(), weight),
        (emoji, 900),
        "the emoji face at the asked weight"
    );
    let condensed = FontRef {
        family: "Test Condensed".into(),
        source: FontSource::Installed,
    };
    let faces = [
        (
            "Test Condensed",
            dir.join("BebasNeue/BebasNeue-Regular.ttf"),
            400,
            3,
        ),
        ("Test Thai", any, 400, 5),
    ];
    let (choice, drawn, _) = test_installed_shape(&faces, &condensed, 400, "ไทย");
    // A condensed CJK family's twin is as forbidden as the family itself (never a fallback).
    let cjk = klipolo_fonts::index::InstalledFamily {
        family: "Test CJK Condensed".into(),
        alias: "Test CJK".into(),
        faces: vec![klipolo_fonts::installed::InstalledFace {
            weight: 400,
            italic: false,
            stretch: 3,
            path: index
                .bundled_dir()
                .unwrap()
                .join("AdobeNotDef/AND-Regular.ttf"),
            index: 0,
            variable: false,
            bytes: 0,
        }],
        coverage: klipolo_fonts::coverage::HAN,
    };
    let forbidden = klipolo_engine::render::font_fallback::cjk_forbidden(&[cjk]);
    assert!(
        forbidden.iter().any(|f| f == "kl:w:Test CJK:3"),
        "{forbidden:?}"
    );
    assert_eq!(choice.family, "kl:w:Test Condensed:3");
    assert_eq!(
        drawn, "Test Thai",
        "the system's Thai face, not boxes in the condensed one"
    );
}
