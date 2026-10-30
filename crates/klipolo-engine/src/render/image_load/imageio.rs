//! ImageIO decoding (macOS): EXIF orientation, HEIC/AVIF/TIFF, colour profiles converted to sRGB.

use std::ffi::c_void;
use std::path::Path;

use cidre::{arc, cf, cg};
use image::RgbaImage;

use super::{UNSUPPORTED, unreadable};

/// RGBA bytes in memory order: premultiplied alpha last, 32-bit big-endian.
const RGBA_PREMULTIPLIED: u32 = 1 | (4 << 12);

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    static kCGImageSourceCreateThumbnailFromImageAlways: &'static cf::String;
    static kCGImageSourceCreateThumbnailWithTransform: &'static cf::String;
    static kCGImageSourceThumbnailMaxPixelSize: &'static cf::String;
    static kCGImagePropertyPixelWidth: &'static cf::String;
    static kCGImagePropertyPixelHeight: &'static cf::String;
    static kCGImagePropertyOrientation: &'static cf::String;
    fn CGImageSourceCreateWithURL(url: &cf::Url, options: *const c_void) -> *const c_void;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: *const c_void,
        index: usize,
        options: *const c_void,
    ) -> Option<arc::R<cf::Dictionary>>;
    fn CGImageSourceGetType(source: *const c_void) -> *const c_void;
    fn CGImageSourceGetCount(source: *const c_void) -> usize;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: *const c_void,
        index: usize,
        options: &cf::Dictionary,
    ) -> Option<arc::R<cg::Image>>;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: &cg::ColorSpace,
        bitmap_info: u32,
    ) -> Option<arc::R<cg::Context>>;
    fn CGContextDrawImage(context: &cg::Context, rect: cg::Rect, image: &cg::Image);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

/// An image source, released on drop.
struct Source(*const c_void);

impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: created by CGImageSourceCreateWithURL (+1), released once.
        unsafe { CFRelease(self.0) }
    }
}

pub(super) fn display_size(path: &Path) -> Option<(u32, u32)> {
    let url = cf::Url::with_file_path(path)?;
    // SAFETY: plain ImageIO calls on a valid URL; a null source means the file couldn't be opened.
    let source = unsafe { CGImageSourceCreateWithURL(&url, std::ptr::null()) };
    if source.is_null() {
        return None;
    }
    let source = Source(source);
    // SAFETY: `source` is live; reading the properties doesn't decode pixels.
    let props = unsafe { CGImageSourceCopyPropertiesAtIndex(source.0, 0, std::ptr::null()) }?;
    let number = |key: &cf::String| {
        props.value(key.as_type_ref()).and_then(|v| {
            // SAFETY: these properties are CFNumbers.
            unsafe { &*(v as *const cf::Type as *const cf::Number) }.to_i32()
        })
    };
    // SAFETY: the keys are ImageIO's constant strings.
    let (w, h, orientation) = unsafe {
        (
            number(kCGImagePropertyPixelWidth)?,
            number(kCGImagePropertyPixelHeight)?,
            number(kCGImagePropertyOrientation).unwrap_or(1),
        )
    };
    let (w, h) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    Some(if (5..=8).contains(&orientation) {
        (h, w)
    } else {
        (w, h)
    })
}

pub(super) fn decode(path: &Path, max_side: u32, premultiplied: bool) -> Result<RgbaImage, String> {
    let url = cf::Url::with_file_path(path).ok_or_else(|| unreadable(path))?;
    // SAFETY: plain ImageIO calls on a valid URL; a null source means the file couldn't be opened.
    let source = unsafe { CGImageSourceCreateWithURL(&url, std::ptr::null()) };
    if source.is_null() {
        return Err(unreadable(path));
    }
    let source = Source(source);
    // SAFETY: `source` is a live image source.
    if unsafe { CGImageSourceGetType(source.0).is_null() || CGImageSourceGetCount(source.0) == 0 } {
        return Err(UNSUPPORTED.into());
    }
    let max = cf::Number::from_i32(max_side.min(i32::MAX as u32) as i32);
    // SAFETY: the keys are ImageIO's constant strings.
    let keys = unsafe {
        [
            kCGImageSourceCreateThumbnailFromImageAlways.as_type_ref(),
            kCGImageSourceCreateThumbnailWithTransform.as_type_ref(),
            kCGImageSourceThumbnailMaxPixelSize.as_type_ref(),
        ]
    };
    let yes = cf::Boolean::value_true().as_type_ref();
    let options = cf::Dictionary::with_keys_values(&keys, &[yes, yes, max.as_type_ref()])
        .ok_or_else(|| unreadable(path))?;
    // The whole image, turned upright by its EXIF orientation and scaled down to `max_side` when larger.
    // SAFETY: `source` is live and `options` is a valid dictionary.
    let image = unsafe { CGImageSourceCreateThumbnailAtIndex(source.0, 0, &options) }
        .ok_or_else(|| unreadable(path))?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return Err(unreadable(path));
    }
    let mut pixels = vec![0u8; width * height * 4];
    let srgb = cg::ColorSpace::with_name(cg::color_space::names::srgb())
        .ok_or_else(|| unreadable(path))?;
    // SAFETY: `pixels` holds width × height × 4 bytes and outlives the context, which is dropped below.
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            &srgb,
            RGBA_PREMULTIPLIED,
        )
    }
    .ok_or_else(|| unreadable(path))?;
    let rect = cg::Rect {
        origin: cg::Point { x: 0.0, y: 0.0 },
        size: cg::Size {
            width: width as cg::Float,
            height: height as cg::Float,
        },
    };
    // SAFETY: the context draws into `pixels`, which is still alive.
    unsafe { CGContextDrawImage(&context, rect, &image) };
    drop(context);
    if premultiplied {
        return RgbaImage::from_raw(width as u32, height as u32, pixels)
            .ok_or_else(|| unreadable(path));
    }
    for px in pixels.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width as u32, height as u32, pixels).ok_or_else(|| unreadable(path))
}
