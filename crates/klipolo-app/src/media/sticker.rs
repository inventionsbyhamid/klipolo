//! A sticker's cut-out. Muse Image can't draw on a transparent background, so stickers are drawn with a bold dark
//! outline on plain white and the white around the subject is cut away here: a flood fill from every border pixel
//! through near-white, which stops at the outline, so white inside the outline (a highlight, a cup's inside) stays.
//! The edge where white met the subject gets a 1 to 2 pixel feathered alpha, with the white taken out of its colour.
//!
//! A cut-out that would leave under 5% of the picture (a white subject, or one cut off by the frame, which lets the
//! flood inside) or over 95% (no plain white around it) is refused: the caller keeps the opaque picture and shows the
//! note.

use std::collections::VecDeque;
use std::fmt;
use std::io::Cursor;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::io::{Limits, Reader as ImageReader};
use image::{ExtendedColorType, ImageEncoder, ImageFormat, RgbaImage};

/// A pixel is near-white when its darkest channel is at least `255 - NEAR_WHITE` and its channels differ by at most
/// `NEAR_WHITE` (no tint). Tuned on the paper plane: its background is 246 to 255 with a spread of at most 6, and the
/// kept share hardly moves from 10 to 40; 24 leaves room for noisier pictures without reaching pale colours.
pub const NEAR_WHITE: u8 = 24;
/// The least share of the picture a cut-out may keep.
pub const MIN_KEPT: f64 = 0.05;
/// The most share of the picture a cut-out may keep.
pub const MAX_KEPT: f64 = 0.95;
/// What the person reads when the white stays because the cut-out would have erased most of the picture.
pub const ERASES_MOST_NOTE: &str =
    "Kept the white background. The cut-out would have erased most of the picture.";
/// What the person reads when the background stays because there was no plain white around the picture.
pub const NOTHING_TO_CUT_NOTE: &str =
    "Kept the background. There was no plain white around the picture to cut away.";

/// The outermost ring of the subject is at most this opaque: a 1 pixel feather even where the drawing has a hard
/// edge. The second ring keeps only what its colour says, and only where it carries on a soft edge.
const FIRST_RING_ALPHA: f32 = 2.0 / 3.0;
/// A pixel this transparent in the input counts as background.
const SEE_THROUGH: u8 = 16;
/// The biggest picture read: Muse Image's are 1600 px on their long side.
const MAX_SIDE: u32 = 8192;

/// The cut-out: an RGBA PNG, and the share of the picture's pixels it kept (0 to 1).
#[derive(Clone, PartialEq)]
pub struct KeyedPng {
    pub png: Vec<u8>,
    pub kept_fraction: f64,
}

/// The size, not the bytes.
impl fmt::Debug for KeyedPng {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyedPng")
            .field("png", &format_args!("{} bytes", self.png.len()))
            .field("kept_fraction", &self.kept_fraction)
            .finish()
    }
}

/// Why there is no cut-out.
#[derive(Clone, Debug, PartialEq)]
pub enum CutOutError {
    /// The picture couldn't be read as a PNG, or the cut-out couldn't be written (the detail, for logs).
    Unreadable(String),
    /// Under `MIN_KEPT` would be left: a white subject, or one cut off by the frame.
    ErasesMost { kept_fraction: f64 },
    /// Over `MAX_KEPT` would be left: no plain white around the subject.
    NothingToCut { kept_fraction: f64 },
}

impl CutOutError {
    /// The note to show when the opaque picture is used instead; none for an unreadable picture, which has no
    /// fallback.
    pub fn note(&self) -> Option<&'static str> {
        match self {
            Self::Unreadable(_) => None,
            Self::ErasesMost { .. } => Some(ERASES_MOST_NOTE),
            Self::NothingToCut { .. } => Some(NOTHING_TO_CUT_NOTE),
        }
    }
}

impl fmt::Display for CutOutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(detail) => write!(f, "the picture couldn't be read: {detail}"),
            Self::ErasesMost { kept_fraction } => {
                write!(
                    f,
                    "the cut-out would keep only {:.1}%",
                    kept_fraction * 100.0
                )
            }
            Self::NothingToCut { kept_fraction } => {
                write!(f, "the cut-out would keep {:.1}%", kept_fraction * 100.0)
            }
        }
    }
}

impl std::error::Error for CutOutError {}

/// `png` with the near-white around its subject made transparent (see the module). CPU work of a few tens of
/// milliseconds for a 1600 px picture in a release build: call it off the async threads.
pub fn cut_out_white(png: &[u8]) -> Result<KeyedPng, CutOutError> {
    let mut pixels = decode(png)?;
    let (width, height) = pixels.dimensions();
    let background = flood_from_border(&pixels);
    let total = background.len();
    let kept = background.iter().filter(|&&b| !b).count();
    let kept_fraction = kept as f64 / total as f64;
    if kept_fraction < MIN_KEPT {
        return Err(CutOutError::ErasesMost { kept_fraction });
    }
    if kept_fraction > MAX_KEPT {
        return Err(CutOutError::NothingToCut { kept_fraction });
    }
    feather(&mut pixels, &background, width as usize, height as usize);
    Ok(KeyedPng {
        png: encode(&pixels)?,
        kept_fraction,
    })
}

fn decode(png: &[u8]) -> Result<RgbaImage, CutOutError> {
    let mut reader = ImageReader::with_format(Cursor::new(png), ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| CutOutError::Unreadable(e.to_string()))?;
    let pixels = image.to_rgba8();
    if pixels.width() == 0 || pixels.height() == 0 {
        return Err(CutOutError::Unreadable("the picture is empty".into()));
    }
    Ok(pixels)
}

fn near_white(p: [u8; 4]) -> bool {
    if p[3] < SEE_THROUGH {
        return true;
    }
    let low = p[0].min(p[1]).min(p[2]);
    let high = p[0].max(p[1]).max(p[2]);
    low >= 255 - NEAR_WHITE && high - low <= NEAR_WHITE
}

/// Which pixels are background: near-white and connected (through near-white, side by side) to a border pixel. An
/// iterative flood with a queue, so a large picture can't overflow the stack.
fn flood_from_border(pixels: &RgbaImage) -> Vec<bool> {
    let (width, height) = (pixels.width() as usize, pixels.height() as usize);
    let raw = pixels.as_raw();
    let white = |i: usize| near_white([raw[4 * i], raw[4 * i + 1], raw[4 * i + 2], raw[4 * i + 3]]);
    let mut background = vec![false; width * height];
    let mut queue = VecDeque::new();
    let visit = |i: usize, background: &mut Vec<bool>, queue: &mut VecDeque<usize>| {
        if !background[i] && white(i) {
            background[i] = true;
            queue.push_back(i);
        }
    };
    for x in 0..width {
        visit(x, &mut background, &mut queue);
        visit((height - 1) * width + x, &mut background, &mut queue);
    }
    for y in 0..height {
        visit(y * width, &mut background, &mut queue);
        visit(y * width + width - 1, &mut background, &mut queue);
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % width, i / width);
        if x > 0 {
            visit(i - 1, &mut background, &mut queue);
        }
        if x + 1 < width {
            visit(i + 1, &mut background, &mut queue);
        }
        if y > 0 {
            visit(i - width, &mut background, &mut queue);
        }
        if y + 1 < height {
            visit(i + width, &mut background, &mut queue);
        }
    }
    background
}

/// Background pixels become transparent (and colourless); the subject's two outer rings get an alpha from how much of
/// the white they hold, with that white taken out of their colour, so no light halo is left on a dark video. The second
/// ring is feathered only where it carries on a soft edge (a ring-1 neighbour holds some white and it is no lighter
/// than that neighbour): just inside a thin outline it is the subject itself, white or not, and stays as drawn.
fn feather(pixels: &mut RgbaImage, background: &[bool], width: usize, height: usize) {
    // 1 and 2: the subject's outer rings, by side-by-side steps from the background; 0 anywhere else.
    let mut ring = vec![0u8; width * height];
    let sides = |i: usize| {
        let (x, y) = (i % width, i / width);
        [
            (x > 0).then(|| i - 1),
            (x + 1 < width).then(|| i + 1),
            (y > 0).then(|| i - width),
            (y + 1 < height).then(|| i + width),
        ]
        .into_iter()
        .flatten()
    };
    for i in 0..width * height {
        if !background[i] && sides(i).any(|j| background[j]) {
            ring[i] = 1;
        }
    }
    for i in 0..width * height {
        if !background[i] && ring[i] == 0 && sides(i).any(|j| ring[j] == 1) {
            ring[i] = 2;
        }
    }
    let original = pixels.clone();
    let raw = original.as_raw();
    let at = |i: usize| [raw[4 * i], raw[4 * i + 1], raw[4 * i + 2], raw[4 * i + 3]];
    let out: &mut [u8] = pixels;
    for i in 0..width * height {
        let px = &mut out[4 * i..4 * i + 4];
        if background[i] {
            px.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        if ring[i] == 0 {
            continue;
        }
        let pixel = at(i);
        let coverage_at =
            |j: usize| white_coverage(at(j), darkest_nearby(&at, background, width, height, j));
        let soft_edge = |j: usize| ring[j] == 1 && coverage_at(j) < 1.0 && low(pixel) <= low(at(j));
        if ring[i] == 2 && !sides(i).any(soft_edge) {
            continue;
        }
        let coverage = coverage_at(i);
        let alpha = if ring[i] == 1 {
            coverage.min(FIRST_RING_ALPHA)
        } else {
            coverage
        };
        if coverage > 0.0 && coverage < 1.0 {
            // The pixel is `coverage` of the subject's colour over white: take the white out.
            for (out, &mixed) in px[..3].iter_mut().zip(&pixel[..3]) {
                let unmixed = (f32::from(mixed) - (1.0 - coverage) * 255.0) / coverage;
                *out = unmixed.round().clamp(0.0, 255.0) as u8;
            }
        }
        px[3] = (alpha * f32::from(pixel[3])).round().clamp(0.0, 255.0) as u8;
    }
}

/// The darkest subject pixel within 2 pixels of `i`: the colour the edge blends towards (the outline's, usually).
fn darkest_nearby(
    at: &impl Fn(usize) -> [u8; 4],
    background: &[bool],
    width: usize,
    height: usize,
    i: usize,
) -> [u8; 4] {
    let (x, y) = ((i % width) as isize, (i / width) as isize);
    let mut darkest = at(i);
    for ny in (y - 2).max(0)..=(y + 2).min(height as isize - 1) {
        for nx in (x - 2).max(0)..=(x + 2).min(width as isize - 1) {
            let j = ny as usize * width + nx as usize;
            if !background[j] && low(at(j)) < low(darkest) {
                darkest = at(j);
            }
        }
    }
    darkest
}

fn low(p: [u8; 4]) -> u8 {
    p[0].min(p[1]).min(p[2])
}

/// How much of `pixel` is `ink` rather than white (0 to 1), from their darkest channels: a pixel as dark as the ink is
/// all ink.
fn white_coverage(pixel: [u8; 4], ink: [u8; 4]) -> f32 {
    let room = 255.0 - f32::from(low(ink));
    if room < 1.0 {
        return 1.0;
    }
    ((255.0 - f32::from(low(pixel))) / room).clamp(0.0, 1.0)
}

fn encode(pixels: &RgbaImage) -> Result<Vec<u8>, CutOutError> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|e| CutOutError::Unreadable(e.to_string()))?;
    Ok(out)
}
