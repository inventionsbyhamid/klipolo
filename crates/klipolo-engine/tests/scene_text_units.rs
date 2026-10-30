//! Scene text laid out (no GPU): a 60-character name,
//! an empty title, emoji (ZWJ sequences and VS16), CJK before its Noto download, a newline and right-to-left letters
//! each lay out inside their box as their units; a 1 MB string and 10,000 combining marks on one base are held to the
//! caps, said, quickly; an em is never laid out past 2,048 px, and right-to-left text moves about its right edge.
//! Shaped with Klipolo's own faces (the managed font system), as the preview starts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_engine::render::fonts::{configure_for_tests, index};
use klipolo_engine::render::scene::text::{
    CTextNode, MAX_CLUSTER_CHARS, MAX_PIECES, NO_UNIT, TextLook, TextUnits, piece_text,
};
use klipolo_engine::render::shared_fonts::SharedFonts;
use klipolo_model::font::FontRef;
use klipolo_model::scene::{Fit, Split};

fn node(size: f32, fit: Option<Fit>, split: Option<Split>) -> CTextNode {
    CTextNode {
        looks: vec![TextLook {
            font: FontRef::bundled("Inter"),
            weight: 700,
            italic: false,
            size,
        }],
        letter_spacing: 0.0,
        line_height: 1.2,
        align: None,
        wrap: None,
        fit,
        split,
        reveal: None,
        language: Arc::from("en"),
        emoji: false,
    }
}

fn lay_out(node: &CTextNode, text: &str, box_px: (f32, f32)) -> TextUnits {
    configure_for_tests();
    let mut fonts = SharedFonts::new(false);
    TextUnits::lay_out(&mut fonts, &index(), node, text, box_px, 1.0)
}

/// Every piece's box lies across the box (a pixel of slack), and down it unless the text clips there.
fn inside(t: &TextUnits, (w, h): (f32, f32)) -> bool {
    t.pieces.iter().all(|p| {
        p.rect[0] >= -1.0
            && p.rect[2] <= w + 1.0
            && (t.clip || (p.rect[1] >= -1.0 && p.rect[3] <= h + 1.0))
    })
}

#[test]
fn a_sixty_character_name_shrinks_to_fit_its_box() {
    let name = "Maximiliana Wolfeschlegelsteinhausen-Bergerdorff Ana Lee Jr.";
    assert_eq!(name.chars().count(), 60);
    let n = node(48.0, Some(Fit::Shrink), None);
    let t = lay_out(&n, name, (1200.0, 80.0));
    assert!(
        t.scale < 1.0 && t.scale >= 0.5,
        "shrunk, not past half: {}",
        t.scale
    );
    assert_eq!(
        t.lines.len(),
        1,
        "on its own line while it fits at half or more"
    );
    assert!(t.clip && inside(&t, (1200.0, 80.0)));
    // Twice as long: past half it wraps at half, and clips to the box.
    let long = format!("{name} {name}");
    let t = lay_out(&n, &long, (600.0, 80.0));
    assert_eq!(t.scale, 0.5);
    assert!(t.lines.len() >= 2 && t.clip && inside(&t, (600.0, 80.0)));
}

#[test]
fn an_empty_title_lays_out_nothing() {
    let t = lay_out(
        &node(48.0, Some(Fit::Shrink), Some(Split::Chars)),
        "",
        (400.0, 80.0),
    );
    assert!(t.pieces.is_empty() && t.units == 0 && !t.notes.cut && !t.notes.stand_in);
}

#[test]
fn an_emoji_cluster_is_one_unit() {
    // A family (three people joined by ZWJ), a red heart with VS16, and two letters.
    let text = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{2764}\u{FE0F}ok";
    let t = lay_out(&node(48.0, None, Some(Split::Chars)), text, (600.0, 100.0));
    assert_eq!(t.units, 4, "the family, the heart, o and k");
    for p in &t.pieces {
        let shown = piece_text(&t, p);
        assert!(shown == "ok" || shown.chars().count() <= 5, "{shown:?}");
    }
    assert!(
        t.pieces
            .iter()
            .all(|p| p.unit != NO_UNIT && usize::from(p.unit) < 4)
    );
    assert!(inside(&t, (600.0, 100.0)));
}

#[test]
fn cjk_before_its_download_says_so() {
    let t = lay_out(&node(48.0, None, None), "李安", (400.0, 100.0));
    assert_ne!(t.notes.cjk_missing, 0, "the missing-CJK-font code");
    assert!(
        !t.pieces.is_empty(),
        "drawn in Adobe NotDef's boxes meanwhile"
    );
}

#[test]
fn a_newline_starts_a_line() {
    let t = lay_out(
        &node(48.0, None, Some(Split::Lines)),
        "Ana\nLee",
        (400.0, 200.0),
    );
    assert_eq!((t.lines.len(), t.units), (2, 2));
    let top = |unit: u16| {
        t.pieces
            .iter()
            .find(|p| p.unit == unit)
            .map(|p| p.rect[1])
            .unwrap()
    };
    assert!(top(1) > top(0));
}

#[test]
fn right_to_left_letters_read_in_order_from_the_right() {
    let t = lay_out(
        &node(48.0, None, Some(Split::Words)),
        "שלום עולם",
        (600.0, 100.0),
    );
    assert_eq!(t.units, 2);
    let left = |unit: u16| {
        t.pieces
            .iter()
            .find(|p| p.unit == unit)
            .map(|p| p.rect[0])
            .unwrap()
    };
    assert!(left(0) > left(1), "the first word is on the right");
    let right = t.pieces.iter().map(|p| p.rect[2]).fold(0.0, f32::max);
    assert!(
        (right - 600.0).abs() < 2.0,
        "a right-to-left paragraph aligns right: {right}"
    );
    assert!(inside(&t, (600.0, 100.0)));
}

#[test]
fn hostile_text_is_held_to_the_caps_quickly() {
    let n = node(48.0, Some(Fit::Shrink), Some(Split::Chars));
    let started = Instant::now();
    let megabyte = "W".repeat(1 << 20);
    let t = lay_out(&n, &megabyte, (800.0, 200.0));
    assert!(t.notes.cut, "said");
    assert!(t.text.chars().count() <= 500 && t.units <= 500 && t.pieces.len() <= MAX_PIECES);
    let pile = format!("a{}b", "\u{0301}".repeat(10_000));
    let t = lay_out(&n, &pile, (800.0, 200.0));
    assert!(t.notes.cut);
    assert!(
        t.text.chars().count() <= MAX_CLUSTER_CHARS + 1,
        "{}",
        t.text.chars().count()
    );
    assert!(inside(&t, (800.0, 200.0)) || t.clip);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// The units a split text laid out reach the instance, which places its tweens on them (`set_units_many`): a title split
/// by lines that wraps into three lines has three units, though it has no newline.
#[test]
fn the_layouts_units_reach_the_instance() {
    const WRAPPED: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"id":"t","type":"text","text":"one two three four five six","split":"lines","box":[0,0,0.12,1],"size":60}],
      "timeline":[{"from":"#t","dur":0.5,"opacity":0,"stagger":0.1}]}"##;
    let probe = klipolo_engine::render::scene::TextProbe::new(
        WRAPPED,
        &Default::default(),
        (1280, 720),
        720.0 / 1080.0,
    )
    .unwrap();
    assert!(probe.units("t") >= 3, "{}", probe.units("t"));
}

/// The typewriter shows the units its text laid out once, one more at each step, and lays nothing out to do it.
#[test]
fn the_typewriter_shows_units_laid_out_once() {
    const TYPED: &str = r##"{"scene":1,"duration":3,"nodes":[
      {"id":"w","type":"text","text":"Typed one letter at a time","box":[0,0,1,1],"size":60,
       "reveal":{"by":"chars","at":0,"dur":2}}]}"##;
    let mut probe = klipolo_engine::render::scene::TextProbe::new(
        TYPED,
        &Default::default(),
        (1280, 720),
        720.0 / 1080.0,
    )
    .unwrap();
    let layouts = klipolo_engine::render::scene::text_layout_calls_on_this_thread();
    let (start, middle, end) = (probe.frame(0.0), probe.frame(1.0), probe.frame(2.5));
    assert_eq!(
        (start, end),
        (0, 21),
        "none at first, every letter at the end"
    );
    assert!(middle > 5 && middle < 16, "about half halfway: {middle}");
    assert_eq!(
        klipolo_engine::render::scene::text_layout_calls_on_this_thread(),
        layouts
    );
}

/// A text is never laid out (or rasterised) past 2,048 px an em, however many target pixels a 1080p
/// pixel is (a layer scaled up from almost nothing asked for 10⁶).
#[test]
fn a_huge_scale_lays_out_at_a_bounded_size() {
    configure_for_tests();
    let mut fonts = SharedFonts::new(false);
    let n = node(48.0, Some(Fit::Shrink), Some(Split::Chars));
    let started = Instant::now();
    let t = TextUnits::lay_out(&mut fonts, &index(), &n, "Ana Lee", (1200.0, 80.0), 1e6);
    assert!(t.font_px <= 2048.0, "{}", t.font_px);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// Right-to-left text without an alignment spreads and grows about its right edge, where it stands.
#[test]
fn right_to_left_text_moves_about_its_right_edge() {
    let t = lay_out(
        &node(48.0, None, Some(Split::Words)),
        "שלום עולם",
        (600.0, 100.0),
    );
    assert_eq!(t.align_share, 1.0);
    assert!((t.anchor[0] - 600.0).abs() < 1.0, "{:?}", t.anchor);
}
