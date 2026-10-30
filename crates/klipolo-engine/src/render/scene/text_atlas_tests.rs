//! `text_atlas.rs`'s draw, on its own: an atlas that would pass the budget is never
//! allocated; the draw says by how much to shrink instead.

use std::sync::Arc;

use klipolo_model::font::FontRef;

use super::text::{CTextNode, TextLook, TextUnits};
use super::text_atlas::draw;
use crate::render::fonts::{configure_for_tests, index};
use crate::render::shared_fonts::SharedFonts;

fn title(fonts: &mut SharedFonts) -> TextUnits {
    let node = CTextNode {
        looks: vec![TextLook {
            font: FontRef::bundled("Inter"),
            weight: 700,
            italic: false,
            size: 200.0,
        }],
        letter_spacing: 0.0,
        line_height: 1.2,
        align: None,
        wrap: None,
        fit: None,
        split: None,
        reveal: None,
        language: Arc::from("en"),
        emoji: false,
    };
    TextUnits::lay_out(fonts, &index(), &node, "Big title", (2000.0, 400.0), 1.0)
}

#[test]
fn an_atlas_past_the_budget_is_never_allocated() {
    configure_for_tests();
    let mut fonts = SharedFonts::new(false);
    let t = title(&mut fonts);
    let texts = [(&t, 0.0f32)];
    let whole =
        draw(fonts.system(), &texts, 1.0, u64::MAX).expect("drawn within an unbounded budget");
    let bytes = whole.bytes();
    assert!(bytes > 4096, "{bytes}");
    // A quarter of its bytes: refused before its pixels are made, with the factor that fits them.
    match draw(fonts.system(), &texts, 1.0, bytes / 4) {
        Ok(image) => panic!("drawn past the budget: {} bytes", image.bytes()),
        Err(shrink) => assert!((shrink - 0.5).abs() < 0.05, "{shrink}"),
    }
}
