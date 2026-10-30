//! Still images for backgrounds and image layers, decoded upright and no larger than needed. On macOS ImageIO
//! decodes them: it applies the EXIF orientation (phone photos are stored sideways), reads HEIC/HEIF, AVIF and TIFF,
//! and converts embedded colour profiles to sRGB. Elsewhere the `image` crate decodes PNG, JPEG and WebP, and a JPEG
//! is turned upright by its EXIF orientation (`orientation`).

use std::path::{Path, PathBuf};

use image::RgbaImage;

/// What the preview says about an image it can't decode at all.
pub const UNSUPPORTED: &str = "This image format isn't supported yet.";

/// The message for an image that isn't there (export says it as is; the preview says it as a missing video is).
pub fn missing(path: &Path) -> String {
    crate::render::pool::missing(&path.to_string_lossy())
}

/// Why an image didn't load: its file isn't there (not found, or not a file), which the preview says as it says a
/// missing video (`api::MEDIA_MISSING`), or the message to show (unreadable, unsupported).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    Missing(PathBuf),
    Message(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(path) => f.write_str(&missing(path)),
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl From<String> for ImageError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

/// The metadata of the file at `path` if it is there to read: missing when it isn't found or isn't a file, unreadable
/// on any other error (no permission, a failing drive).
pub fn check(path: &Path) -> Result<std::fs::Metadata, ImageError> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(meta),
        Ok(_) => Err(ImageError::Missing(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(ImageError::Missing(path.to_path_buf()))
        }
        Err(_) => Err(ImageError::Message(unreadable(path))),
    }
}

pub fn unreadable(path: &Path) -> String {
    crate::render::pool::unreadable(&path.to_string_lossy())
}

/// The image at `path` as straight-alpha RGBA, upright, with its longer side at most `max_side`. Errors are the
/// message to show (the layer is skipped; the frame still draws).
pub fn decode_rgba(path: &Path, max_side: u32) -> Result<RgbaImage, ImageError> {
    decode(path, max_side, false)
}

/// Like `decode_rgba` but with premultiplied alpha, which is what ImageIO draws and what the image quad blends: no
/// per-pixel pass on the CPU.
pub fn decode_premultiplied(path: &Path, max_side: u32) -> Result<RgbaImage, ImageError> {
    decode(path, max_side, true)
}

/// The image's size as shown: its stored size, with width and height swapped when its EXIF orientation turns it a
/// quarter turn (orientations 5–8: phone photos taken in portrait). None when the file can't be read. Imports store
/// this size, so a portrait photo's layout box has a portrait shape.
pub fn display_size(path: &Path) -> Option<(u32, u32)> {
    #[cfg(target_os = "macos")]
    return imageio::display_size(path);
    #[cfg(not(target_os = "macos"))]
    orientation::shown_size(path)
}

fn decode(path: &Path, max_side: u32, premultiplied: bool) -> Result<RgbaImage, ImageError> {
    check(path)?;
    #[cfg(target_os = "macos")]
    return imageio::decode(path, max_side.max(1), premultiplied).map_err(ImageError::from);
    #[cfg(not(target_os = "macos"))]
    {
        let img = image::open(path).map_err(|e| {
            ImageError::Message(match e {
                image::ImageError::Unsupported(_) => UNSUPPORTED.to_string(),
                _ => unreadable(path),
            })
        })?;
        let img = if img.width().max(img.height()) > max_side {
            img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let img = orientation::upright(img, orientation::of(path));
        let mut rgba = img.into_rgba8();
        if premultiplied {
            for px in rgba.pixels_mut() {
                let a = u16::from(px[3]);
                for c in &mut px.0[..3] {
                    *c = ((u16::from(*c) * a + 127) / 255) as u8;
                }
            }
        }
        Ok(rgba)
    }
}

#[cfg(target_os = "macos")]
mod imageio;
#[cfg(not(target_os = "macos"))]
mod orientation;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// A JPEG's APP1 segment holding only an EXIF orientation tag (big-endian TIFF, one IFD entry).
    fn exif_orientation(orientation: u8) -> Vec<u8> {
        let mut tiff = b"MM\0\x2A\0\0\0\x08\0\x01".to_vec();
        tiff.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0]);
        tiff.extend([0, 0, 0, 0]);
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend(((2 + 6 + tiff.len()) as u16).to_be_bytes());
        app1.extend(b"Exif\0\0");
        app1.extend(tiff);
        app1
    }

    #[test]
    fn phone_photos_decode_upright_and_heic_decodes() {
        let dir = tempfile::tempdir().unwrap();
        // Stored 40×20: left half red, right half blue.
        let stored = RgbaImage::from_fn(40, 20, |x, _| {
            image::Rgba(if x < 20 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            })
        });
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&image::DynamicImage::ImageRgba8(stored.clone()).to_rgb8())
            .unwrap();
        // Orientation 6 ("turn 90° clockwise to view"), as phones write for portrait shots.
        let mut tagged = jpeg[..2].to_vec();
        tagged.extend(exif_orientation(6));
        tagged.extend(&jpeg[2..]);
        let path = dir.path().join("phone.jpg");
        std::fs::write(&path, tagged).unwrap();

        assert_eq!(
            display_size(&path),
            Some((20, 40)),
            "imports store the portrait size"
        );
        let upright = decode_rgba(&path, 4096).unwrap();
        assert_eq!(upright.dimensions(), (20, 40), "shown portrait");
        let red = |p: &image::Rgba<u8>| p[0] > 200 && p[2] < 60;
        let blue = |p: &image::Rgba<u8>| p[2] > 200 && p[0] < 60;
        assert!(red(upright.get_pixel(10, 5)), "stored left is on top");
        assert!(blue(upright.get_pixel(10, 35)), "stored right is below");
        let small = decode_rgba(&path, 10).unwrap();
        assert_eq!(small.dimensions(), (5, 10), "scaled to the longer side");

        let gone = decode_rgba(&dir.path().join("gone.png"), 64).unwrap_err();
        assert_eq!(gone, ImageError::Missing(dir.path().join("gone.png")));
        assert_eq!(gone.to_string(), "Some media is missing: gone.png");
        std::fs::write(dir.path().join("notes.png"), b"not an image").unwrap();
        assert_eq!(
            decode_rgba(&dir.path().join("notes.png"), 64).unwrap_err(),
            ImageError::Message(UNSUPPORTED.into())
        );

        // HEIC (iPhone's default) through ImageIO: the stored picture above, encoded once by macOS (`sips -s format
        // heic`) and kept. Made during the test, it was flaky: now and then (right after a rebuild, under heavy load)
        // `sips` encodes the right half as white, with the same parameter sets and different picture data, while the
        // same PNG re-encodes correctly. ImageIO's decode of a given file is the same every time.
        let heic = dir.path().join("photo.heic");
        std::fs::write(&heic, include_bytes!("image_load/red-blue-40x20.heic")).unwrap();
        let decoded = decode_rgba(&heic, 4096).unwrap();
        assert_eq!(decoded.dimensions(), (40, 20));
        let (left, right) = (decoded.get_pixel(5, 10), decoded.get_pixel(30, 10));
        assert!(red(left) && blue(right), "{left:?} {right:?}");
    }
}
