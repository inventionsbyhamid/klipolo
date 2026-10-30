//! Stickers: Muse Image draws on plain white with no transparency, so the white around the subject is cut away here,
//! and a cut-out that would erase most of the picture keeps it whole with a note.

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Instant;

use image::{ImageFormat, Rgb, RgbImage, RgbaImage};
use klipolo_app::media::sticker::{
    CutOutError, ERASES_MOST_NOTE, MAX_KEPT, MIN_KEPT, NOTHING_TO_CUT_NOTE, cut_out_white,
};

/// The real Muse Image answer from the 2026-10-30 check (`README.md` beside it).
fn paper_plane() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/paper-plane.png");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn png_of(image: &RgbImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

fn rgba(png: &[u8]) -> RgbaImage {
    image::load_from_memory_with_format(png, ImageFormat::Png)
        .unwrap()
        .to_rgba8()
}

const WHITE: Rgb<u8> = Rgb([255, 255, 255]);
const BLACK: Rgb<u8> = Rgb([0, 0, 0]);

/// A white canvas with a black frame of `thickness` from (`x0`, `y0`) to (`x1`, `y1`) inclusive; the frame's edges may
/// lie off the canvas.
fn framed(size: u32, (x0, y0): (i64, i64), (x1, y1): (i64, i64), thickness: i64) -> RgbImage {
    RgbImage::from_fn(size, size, |x, y| {
        let (x, y) = (i64::from(x), i64::from(y));
        let inside = x >= x0 && x <= x1 && y >= y0 && y <= y1;
        let edge =
            x < x0 + thickness || x > x1 - thickness || y < y0 + thickness || y > y1 - thickness;
        if inside && edge { BLACK } else { WHITE }
    })
}

#[test]
fn cut_out_removes_outer_white_and_keeps_subject() {
    let keyed = cut_out_white(&paper_plane()).unwrap();
    let out = rgba(&keyed.png);

    assert_eq!(out.dimensions(), (1600, 1600));
    // The plane and its outline are about a third of the square.
    assert!(
        (0.28..0.36).contains(&keyed.kept_fraction),
        "{}",
        keyed.kept_fraction
    );
    // The white around it is gone, corners and the gap under the wings included.
    for (x, y) in [(5, 5), (1595, 1595), (100, 100), (1500, 1500), (800, 1300)] {
        assert_eq!(out.get_pixel(x, y)[3], 0, "({x}, {y}) is still there");
    }
    // The paper and the outline stay as they were.
    for (x, y) in [(900, 800), (600, 800), (1200, 700), (800, 700)] {
        let original = image::load_from_memory(&paper_plane()).unwrap().to_rgb8();
        let kept = out.get_pixel(x, y);
        assert_eq!(kept[3], 255, "({x}, {y})");
        assert_eq!(kept.0[..3], original.get_pixel(x, y).0, "({x}, {y})");
    }
    // The white highlights inside the outline are part of the plane.
    for (x, y) in [(775, 540), (1095, 600), (851, 496)] {
        let kept = out.get_pixel(x, y);
        assert_eq!(kept[3], 255, "highlight ({x}, {y}) was cut");
        assert!(kept.0[..3].iter().all(|&c| c > 240), "({x}, {y}): {kept:?}");
    }
}

#[test]
fn white_inside_the_outline_is_kept() {
    let ring = framed(200, (50, 50), (149, 149), 6);

    let keyed = cut_out_white(&png_of(&ring)).unwrap();
    let out = rgba(&keyed.png);

    assert!(
        (keyed.kept_fraction - 0.25).abs() < 1e-9,
        "{}",
        keyed.kept_fraction
    );
    assert_eq!(out.get_pixel(100, 100).0, [255, 255, 255, 255]);
    assert_eq!(out.get_pixel(57, 57).0, [255, 255, 255, 255]);
    assert_eq!(out.get_pixel(10, 10)[3], 0);
    assert_eq!(out.get_pixel(49, 100)[3], 0);
}

/// A subject cut off by the frame lets the flood into its white inside: the cut-out would leave only its outline, so
/// the picture stays whole, with the note.
#[test]
fn subject_touching_the_border_falls_back_to_opaque() {
    // The frame's left side is off the canvas, so its inside opens onto the left border.
    let open = framed(200, (-20, 50), (149, 149), 3);

    let error = cut_out_white(&png_of(&open)).unwrap_err();

    match error {
        CutOutError::ErasesMost { kept_fraction } => {
            assert!(kept_fraction < MIN_KEPT, "{kept_fraction}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(error.note(), Some(ERASES_MOST_NOTE));
    assert_eq!(
        ERASES_MOST_NOTE,
        "Kept the white background. The cut-out would have erased most of the picture."
    );
}

#[test]
fn a_white_subject_falls_back_to_opaque() {
    // A white cup drawn in a hairline: only the line would be left.
    let cup = framed(300, (40, 40), (259, 259), 1);
    let mut open = cup.clone();
    for y in 40..=259 {
        open.put_pixel(40, y, WHITE);
    }
    assert!(matches!(
        cut_out_white(&png_of(&open)),
        Err(CutOutError::ErasesMost { .. })
    ));
}

#[test]
fn a_picture_with_no_white_around_it_falls_back_to_opaque() {
    let photo = RgbImage::from_pixel(120, 80, Rgb([40, 90, 160]));

    let error = cut_out_white(&png_of(&photo)).unwrap_err();

    assert!(
        matches!(error, CutOutError::NothingToCut { kept_fraction } if kept_fraction > MAX_KEPT),
        "{error:?}"
    );
    assert_eq!(error.note(), Some(NOTHING_TO_CUT_NOTE));
    for note in [ERASES_MOST_NOTE, NOTHING_TO_CUT_NOTE] {
        assert!(!note.contains(['\u{2014}', '\u{2013}']), "{note}");
    }
}

#[test]
fn edges_are_feathered_and_keep_no_white_fringe() {
    // A black square with hard edges on white.
    let square = RgbImage::from_fn(100, 100, |x, y| {
        if (30..70).contains(&x) && (30..70).contains(&y) {
            BLACK
        } else {
            WHITE
        }
    });
    // A grey pixel where white meets the outline, as anti-aliasing leaves it.
    let mut soft = square.clone();
    soft.put_pixel(29, 50, Rgb([200, 200, 200]));

    let out = rgba(&cut_out_white(&png_of(&soft)).unwrap().png);

    let edge = out.get_pixel(30, 40);
    assert!(edge[3] > 0 && edge[3] < 255, "outer edge {edge:?}");
    assert_eq!(out.get_pixel(50, 50).0, [0, 0, 0, 255]);
    assert_eq!(out.get_pixel(34, 50).0, [0, 0, 0, 255]);
    // The grey edge pixel is mostly see-through and dark, not a light halo.
    let grey = out.get_pixel(29, 50);
    assert!(grey[3] < 128, "{grey:?}");
    assert!(grey.0[..3].iter().all(|&c| c < 64), "{grey:?}");
    // Cut-away pixels carry no colour.
    assert_eq!(out.get_pixel(5, 5).0, [0, 0, 0, 0]);
}

#[test]
fn an_unreadable_picture_is_an_error() {
    let error = cut_out_white(b"\x89PNG\r\n\x1a\n not a picture").unwrap_err();
    assert!(matches!(error, CutOutError::Unreadable(_)), "{error:?}");
    assert_eq!(error.note(), None);
}

/// A 1600x1600 sticker keys out well under a second in a release build (the flood is a queue, never recursion).
#[test]
fn cut_out_of_a_full_size_sticker_is_quick() {
    let png = paper_plane();
    let started = Instant::now();
    cut_out_white(&png).unwrap();
    let took = started.elapsed();
    eprintln!("cut-out of 1600x1600: {took:?}");
    if cfg!(not(debug_assertions)) {
        assert!(took.as_millis() < 500, "{took:?}");
    }
}
