//! The stills the app hands the render worker for frames of a project: a decoded video frame turned
//! upright, fitted to `MAX_STILL_EDGE` on its long edge and written as a PNG (`png_of`), and an image asset too large
//! to copy as it is, read within limits and written smaller (`image_within`, or `image_file_within` from its file). The
//! worker reads them back with `frozen::read_png`; it never opens a file of the person's.

use std::io::Cursor;

use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::imageops::FilterType;
use image::{ImageEncoder, RgbaImage};

use crate::vendor::cap::rendering::decoder::{DecodedFrame, PixelFormat};

/// The pixels an image may decode to (`image_within`, `image_file_within`): with the RGBA copy a non-RGBA image needs,
/// a decode peaks under about 600 MiB.
const MAX_IMAGE_ALLOC: u64 = 256 << 20;

/// `frame` (turned by `rotation` degrees clockwise, as the decoder pool says) as a PNG within `edge` on both sides; None
/// when its planes don't hold what its size says.
pub fn png_of(frame: &DecodedFrame, rotation: u16, edge: u32) -> Option<Vec<u8>> {
    let rgba = rgba_of(frame)?;
    let upright = match rotation % 360 {
        90 => image::imageops::rotate90(&rgba),
        180 => image::imageops::rotate180(&rgba),
        270 => image::imageops::rotate270(&rgba),
        _ => rgba,
    };
    encode(&fit(upright, edge))
}

/// An image file's `bytes`, decoded within limits, as a PNG within `edge` on both sides (an asset over the size the app
/// copies as it is).
pub fn image_within(bytes: &[u8], edge: u32) -> Result<Vec<u8>, String> {
    within(image::io::Reader::new(Cursor::new(bytes)), edge)
}

/// The image file at `path`, decoded from the file within limits (never read into memory whole first), as a PNG within
/// `edge` on both sides.
pub fn image_file_within(path: &std::path::Path, edge: u32) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("the image can't be opened: {e}"))?;
    within(image::io::Reader::new(std::io::BufReader::new(file)), edge)
}

fn within<R: std::io::BufRead + std::io::Seek>(
    reader: image::io::Reader<R>,
    edge: u32,
) -> Result<Vec<u8>, String> {
    let mut reader = reader
        .with_guessed_format()
        .map_err(|e| format!("the image can't be read: {e}"))?;
    let mut limits = image::io::Limits::default();
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| {
        format!(
            "the image can't be read within {} MiB: {e}",
            MAX_IMAGE_ALLOC >> 20
        )
    })?;
    encode(&fit(image.into_rgba8(), edge)).ok_or_else(|| "the image couldn't be written".into())
}

/// `image` scaled down (never up) so both sides fit `edge`.
fn fit(image: RgbaImage, edge: u32) -> RgbaImage {
    let (w, h) = image.dimensions();
    let long = w.max(h);
    if long <= edge || edge == 0 {
        return image;
    }
    let scale = f64::from(edge) / f64::from(long);
    let side = |n: u32| ((f64::from(n) * scale).round() as u32).clamp(1, edge);
    image::imageops::resize(&image, side(w), side(h), FilterType::Triangle)
}

/// A PNG of `image`, compressed quickly (the worker reads it once).
fn encode(image: &RgbaImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, PngFilter::Adaptive)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(out)
}

/// `frame`'s pixels as RGBA: copied for RGBA, converted from BT.709 video range for NV12 and YUV 4:2:0.
fn rgba_of(frame: &DecodedFrame) -> Option<RgbaImage> {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let ys = frame.y_stride() as usize;
    let mut out = vec![0u8; w * h * 4];
    match frame.format() {
        PixelFormat::Rgba => {
            let data = frame.data();
            let stride = ys.max(w * 4);
            for row in 0..h {
                let src = data.get(row * stride..row * stride + w * 4)?;
                out[row * w * 4..(row + 1) * w * 4].copy_from_slice(src);
            }
        }
        PixelFormat::Nv12 => {
            let (y, uv, uvs) = (
                frame.y_plane()?,
                frame.uv_plane()?,
                frame.uv_stride() as usize,
            );
            for row in 0..h {
                for col in 0..w {
                    let luma = *y.get(row * ys + col)?;
                    let at = (row / 2) * uvs + (col / 2) * 2;
                    let (u, v) = (*uv.get(at)?, *uv.get(at + 1)?);
                    put(&mut out, row * w + col, luma, u, v);
                }
            }
        }
        PixelFormat::Yuv420p => {
            let (y, u, v) = (frame.y_plane()?, frame.u_plane()?, frame.v_plane()?);
            let uvs = frame.uv_stride() as usize;
            for row in 0..h {
                for col in 0..w {
                    let at = (row / 2) * uvs + col / 2;
                    put(
                        &mut out,
                        row * w + col,
                        *y.get(row * ys + col)?,
                        *u.get(at)?,
                        *v.get(at)?,
                    );
                }
            }
        }
    }
    RgbaImage::from_raw(w as u32, h as u32, out)
}

/// One pixel from BT.709 video-range Y′CbCr, opaque.
fn put(out: &mut [u8], pixel: usize, y: u8, u: u8, v: u8) {
    let y = 1.164_383 * (f32::from(y) - 16.0);
    let (u, v) = (f32::from(u) - 128.0, f32::from(v) - 128.0);
    let channel = |c: f32| c.round().clamp(0.0, 255.0) as u8;
    let at = pixel * 4;
    out[at] = channel(y + 1.792_741 * v);
    out[at + 1] = channel(y - 0.213_249 * u - 0.532_909 * v);
    out[at + 2] = channel(y + 2.112_402 * u);
    out[at + 3] = 255;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::frozen::read_png;

    /// A turned frame comes back upright and within the edge, as the worker reads it.
    #[test]
    fn a_frame_is_turned_upright_fitted_and_read_back_as_the_worker_reads_it() {
        let (w, h) = (40u32, 20u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        for px in data.chunks_exact_mut(4) {
            px.copy_from_slice(&[200, 40, 90, 255]);
        }
        let frame = DecodedFrame::new(data, w, h);
        let png = png_of(&frame, 90, 16).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1.png");
        std::fs::write(&path, &png).unwrap();
        let back = read_png(&path, 16).unwrap();
        assert_eq!(back.dimensions(), (8, 16), "turned and fitted");
        assert_eq!(back.get_pixel(3, 3).0, [200, 40, 90, 255]);
    }

    /// Video-range grey converts to grey; a frame whose planes are short is refused, not read past.
    #[test]
    fn nv12_converts_and_short_planes_are_refused() {
        let (w, h) = (4u32, 2u32);
        let mut data = vec![126u8; (w * h) as usize];
        data.extend_from_slice(&[128u8; 4]);
        let frame = DecodedFrame::new_nv12(data, w, h, w, w);
        let rgba = rgba_of(&frame).unwrap();
        let px = rgba.get_pixel(1, 1).0;
        assert!(px[..3].iter().all(|c| (127..=129).contains(c)), "{px:?}");
        let short = DecodedFrame::new_nv12(vec![0; 3], w, h, w, w);
        assert!(png_of(&short, 0, 64).is_none());
    }
}
