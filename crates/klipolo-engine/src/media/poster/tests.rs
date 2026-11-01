//! Posters made from real files: which picture, its size, its turn and its colours.

use klipolo_platform::durable::Os;

use super::{is_dull, make_poster};
use crate::media::thumbs::poster;

fn fixture(name: &str) -> String {
    let path = crate::fixture_set::fixture_dir().join(name);
    assert!(
        path.exists(),
        "{name} is missing: run scripts/gen-engine-fixtures.sh"
    );
    path.to_string_lossy().into_owned()
}

/// The mean colour of the `(x, y, w, h)` region of `picture`.
fn mean_rgb(picture: &image::RgbImage, (x, y, w, h): (u32, u32, u32, u32)) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for py in y..y + h {
        for px in x..x + w {
            let p = picture.get_pixel(px, py);
            for c in 0..3 {
                sum[c] += f64::from(p[c]);
            }
        }
    }
    sum.map(|s| s / f64::from(w * h))
}

fn near(found: [f64; 3], want: [f64; 3], within: f64) -> bool {
    found.iter().zip(want).all(|(f, w)| (f - w).abs() <= within)
}

/// A phone video stored turned: its first picture (red over blue, not dull) is the poster, upright and fitted in
/// 640 × 360 (shown 720 × 1280: 202 × 360), in the colours playback shows (BT.709): red stays red, not the orange a
/// JPEG's own BT.601 reading of the same values gives.
#[test]
fn the_first_picture_is_the_poster_upright_in_playbacks_colours() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("phone-portrait.mov");
    assert_eq!(poster(root.path(), &video), None);
    let made = make_poster(root.path(), &Os, &video)
        .unwrap()
        .expect("a poster");
    assert_eq!((made.w, made.h, made.current), (202, 360, true));
    assert_eq!(poster(root.path(), &video), Some(made.clone()));
    let picture = image::open(&made.path).unwrap().to_rgb8();
    assert_eq!(picture.dimensions(), (202, 360));
    let top = mean_rgb(&picture, (20, 20, 160, 120));
    assert!(near(top, [255.0, 0.0, 0.0], 8.0), "top {top:?}");
    let bottom = mean_rgb(&picture, (20, 220, 160, 120));
    assert!(near(bottom, [0.0, 0.0, 255.0], 8.0), "bottom {bottom:?}");
}

/// A plain picture (one colour all over) gives way to the one at 1 s, here the same blue: in playback's colours,
/// #1E64DC.
#[test]
fn a_plain_blue_file_keeps_playbacks_blue() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("solid-blue.mp4");
    let made = make_poster(root.path(), &Os, &video)
        .unwrap()
        .expect("a poster");
    assert_eq!((made.w, made.h), (640, 360));
    let picture = image::open(&made.path).unwrap().to_rgb8();
    let all = mean_rgb(&picture, (0, 0, 640, 360));
    assert!(near(all, [30.0, 100.0, 220.0], 6.0), "{all:?}");
}

/// A first picture that is black gives way to the picture at 1 s: here flat grey whose luma names the frame (16 + 8 ×
/// (frame mod 25)), frame 0 black and frame 30 (1.001 s at 29.97 fps) at 56. A small file keeps its own size.
#[test]
fn a_black_first_picture_gives_way_to_the_one_at_1_s() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("ntsc-luma-10s.mp4");
    let made = make_poster(root.path(), &Os, &video)
        .unwrap()
        .expect("a poster");
    assert_eq!((made.w, made.h), (320, 180), "never enlarged");
    let picture = image::open(&made.path).unwrap().to_rgb8();
    let grey = (56.0 - 16.0) * 255.0 / 219.0;
    let all = mean_rgb(&picture, (0, 0, 320, 180));
    assert!(near(all, [grey; 3], 3.0), "{all:?}, frame 30 is {grey}");
}

/// A short file's black first picture gives way to the one 10 % in: 0.2 s of 2 s, frame 6 at 30 fps (luma 64).
#[test]
fn a_short_files_black_first_picture_gives_way_to_the_one_a_tenth_in() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("luma-30fps-2s.mp4");
    let made = make_poster(root.path(), &Os, &video)
        .unwrap()
        .expect("a poster");
    let picture = image::open(&made.path).unwrap().to_rgb8();
    let grey = (64.0 - 16.0) * 255.0 / 219.0;
    let all = mean_rgb(&picture, (0, 0, made.w, made.h));
    assert!(near(all, [grey; 3], 3.0), "{all:?}, frame 6 is {grey}");
}

/// A sound file has no picture: no poster, and nothing written.
#[test]
fn a_sound_file_has_no_poster() {
    let root = tempfile::tempdir().unwrap();
    let sound = fixture("tone-440.m4a");
    assert_eq!(make_poster(root.path(), &Os, &sound), Ok(None));
    assert_eq!(poster(root.path(), &sound), None);
}

/// Planes of a `w × h` picture: `luma(x, y)` and one chroma value.
fn picture(w: usize, h: usize, luma: impl Fn(usize, usize) -> u8, u: u8, v: u8) -> [Vec<u8>; 3] {
    let y = (0..h)
        .flat_map(|py| (0..w).map(move |px| (px, py)))
        .map(|(px, py)| luma(px, py))
        .collect();
    [y, vec![u; w * h / 4], vec![v; w * h / 4]]
}

fn dull(planes: &[Vec<u8>; 3], w: usize) -> bool {
    is_dull([&planes[0], &planes[1], &planes[2]], (w, planes[0].len() / w))
}

/// Black, white, one colour, noise on black and a black frame with a small title say nothing; bars, a page of text and
/// a two-colour picture do.
#[test]
fn a_picture_that_is_nearly_all_one_colour_is_dull() {
    let (w, h) = (160, 90);
    assert!(dull(&picture(w, h, |_, _| 16, 128, 128), w), "black");
    assert!(dull(&picture(w, h, |_, _| 235, 128, 128), w), "white");
    assert!(dull(&picture(w, h, |_, _| 90, 200, 60), w), "one colour");
    let noise = |x: usize, y: usize| 16 + ((x * 7 + y * 13) % 5) as u8;
    assert!(dull(&picture(w, h, noise, 128, 128), w), "noise on black");
    // A title 20 × 6 (under 1 % of the picture).
    let title = |x: usize, y: usize| if (70..90).contains(&x) && (42..48).contains(&y) { 235 } else { 16 };
    assert!(dull(&picture(w, h, title, 128, 128), w), "a small title on black");

    let bars = |x: usize, _| [235, 210, 170, 145, 106, 81, 41, 16][x * 8 / w];
    assert!(!dull(&picture(w, h, bars, 128, 128), w), "bars");
    // Black lines of text over a fifth of a white page.
    let page = |x: usize, y: usize| if y % 10 < 2 && (10..150).contains(&x) { 16 } else { 235 };
    assert!(!dull(&picture(w, h, page, 128, 128), w), "a page of text");
    let halves = |_, y: usize| if y < h / 2 { 81 } else { 41 };
    assert!(!dull(&picture(w, h, halves, 128, 128), w), "two colours");
}
