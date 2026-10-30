//! EXIF orientation where ImageIO doesn't apply it (Windows, Linux). The `image` crate (0.25.1) decodes a JPEG as
//! stored, so a phone's portrait photo, stored sideways with orientation 6 or 8, would show sideways and import with a
//! landscape box. Phones save JPEG (HEIC needs ImageIO), so only a JPEG's EXIF segment is read: the segments before
//! the picture data, in at most the first 128 KB of the file (an APP1 segment holds at most 64 KB). The transforms are
//! the `image` crate's own for each value (`Orientation::from_exif` and `apply_orientation` in its later versions).

use std::io::Read;
use std::path::Path;

use image::DynamicImage;

/// How much of a file is read to find its orientation.
const HEAD: u64 = 128 * 1024;

/// The EXIF orientation (1–8) of the JPEG at `path`; 1, as stored, when it has none or isn't a JPEG.
pub(super) fn of(path: &Path) -> u8 {
    let Ok(file) = std::fs::File::open(path) else {
        return 1;
    };
    let mut head = Vec::new();
    if file.take(HEAD).read_to_end(&mut head).is_err() {
        return 1;
    }
    jpeg(&head).unwrap_or(1)
}

/// The image's size as shown (`display_size`): its stored size, turned a quarter for orientations 5–8.
pub(super) fn shown_size(path: &Path) -> Option<(u32, u32)> {
    let (width, height) = image::image_dimensions(path).ok()?;
    Some(if (5..=8).contains(&of(path)) {
        (height, width)
    } else {
        (width, height)
    })
}

/// `img` as shown: turned and mirrored as `orientation` says.
pub(super) fn upright(img: DynamicImage, orientation: u8) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// The orientation in a JPEG's EXIF segment (APP1, "Exif\0\0", then TIFF), looked for in the segments before the
/// picture data.
fn jpeg(bytes: &[u8]) -> Option<u8> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2;
    loop {
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        // A fill byte before the marker.
        if marker == 0xFF {
            at += 1;
            continue;
        }
        // The picture data (start of scan), or the end: no EXIF before it.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([
            *bytes.get(at + 2)?,
            *bytes.get(at + 3)?,
        ]));
        let body = bytes.get(at + 4..at + 2 + len.max(2))?;
        let found = match body.strip_prefix(b"Exif\0\0") {
            Some(t) if marker == 0xE1 => tiff(t),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        at += 2 + len.max(2);
    }
}

/// The orientation tag (0x0112, one SHORT) of a TIFF block's first directory, either byte order.
fn tiff(t: &[u8]) -> Option<u8> {
    let big = match t.get(..4)? {
        [0x4D, 0x4D, 0, 42] => true,
        [0x49, 0x49, 42, 0] => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let b: [u8; 2] = t.get(i..i + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let b: [u8; 4] = t.get(i..i + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    for k in 0..usize::from(u16_at(ifd)?) {
        let entry = ifd + 2 + 12 * k;
        if u16_at(entry)? == 0x0112 && u16_at(entry + 2)? == 3 && u32_at(entry + 4)? == 1 {
            let value = u16_at(entry + 8)?;
            return u8::try_from(value).ok().filter(|v| (1..=8).contains(v));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use image::RgbaImage;

    use super::super::{decode_rgba, display_size};

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

    /// The macOS test's photo (`phone_photos_decode_upright_and_heic_decodes`), shown as ImageIO shows it.
    #[test]
    fn phone_photos_import_and_decode_upright_off_macos() {
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
            .encode_image(&image::DynamicImage::ImageRgba8(stored).to_rgb8())
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
        // Without the tag, as stored.
        let plain = dir.path().join("plain.jpg");
        std::fs::write(&plain, &jpeg).unwrap();
        assert_eq!(display_size(&plain), Some((40, 20)));
    }
}
