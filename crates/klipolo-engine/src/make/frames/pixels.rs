//! A frame's pixels as the worker sends them: packed rows, a draft over a checkerboard, a project's frame opaque, and
//! several frames as one contact sheet.

use image::RgbImage;

use crate::make::stats::{FrameStats, Luma};

/// Rows of `size.0` pixels, without the readback's padding.
pub(super) fn packed(bytes: &[u8], stride: u32, size: (u32, u32)) -> Vec<u8> {
    let row = size.0 as usize * 4;
    bytes
        .chunks(stride as usize)
        .take(size.1 as usize)
        .flat_map(|r| &r[..row.min(r.len())])
        .copied()
        .collect()
}

/// Premultiplied `rgba` over a mid-grey checkerboard of 16 px squares.
pub(super) fn over_checkerboard(rgba: &[u8], size: (u32, u32)) -> RgbImage {
    RgbImage::from_fn(size.0, size.1, |x, y| {
        let i = (y as usize * size.0 as usize + x as usize) * 4;
        let under = if (x / 16 + y / 16) % 2 == 0 {
            102.0
        } else {
            153.0
        };
        let a = f32::from(rgba[i + 3]) / 255.0;
        let px = |c: u8| (f32::from(c) + (1.0 - a) * under).round().clamp(0.0, 255.0) as u8;
        image::Rgb([px(rgba[i]), px(rgba[i + 1]), px(rgba[i + 2])])
    })
}

pub(super) fn opaque(rgba: &[u8], size: (u32, u32)) -> RgbImage {
    RgbImage::from_fn(size.0, size.1, |x, y| {
        let i = (y as usize * size.0 as usize + x as usize) * 4;
        image::Rgb([rgba[i], rgba[i + 1], rgba[i + 2]])
    })
}

/// The frames in a grid (up to 3 × 3) of the frames' size, and their statistics together.
pub(super) fn contact_sheet(frames: &[(RgbImage, FrameStats)]) -> (RgbImage, FrameStats) {
    let cols = (frames.len() as f64).sqrt().ceil() as u32;
    let rows = (frames.len() as u32).div_ceil(cols);
    let (w, h) = frames[0].0.dimensions();
    let (cw, ch) = ((w / cols).max(1), (h / cols).max(1));
    let mut sheet = RgbImage::new(cw * cols, ch * rows);
    for (i, (image, _)) in frames.iter().enumerate() {
        let small = image::imageops::resize(image, cw, ch, image::imageops::FilterType::Triangle);
        let (x, y) = ((i as u32 % cols) * cw, (i as u32 / cols) * ch);
        image::imageops::replace(&mut sheet, &small, i64::from(x), i64::from(y));
    }
    let n = frames.len() as f64;
    let all = frames.iter().map(|(_, s)| s);
    let stats = FrameStats {
        luma: Luma {
            mean: all.clone().map(|s| s.luma.mean).sum::<f64>() / n,
            min: all.clone().map(|s| s.luma.min).fold(f64::MAX, f64::min),
            max: all.clone().map(|s| s.luma.max).fold(0.0, f64::max),
        },
        alpha_coverage: all.clone().map(|s| s.alpha_coverage).sum::<f64>() / n,
        nan_fixed: all.clone().map(|s| s.nan_fixed).sum(),
        change_from_prev: None,
        loop_seam: frames.last().and_then(|(_, s)| s.loop_seam),
    };
    (sheet, stats)
}
