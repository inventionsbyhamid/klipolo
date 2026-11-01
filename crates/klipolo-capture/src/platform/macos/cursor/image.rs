//! A cursor's image, read only: the current system cursor
//! (`NSCursor.currentSystemCursor`, nil meaning "shape unknown"); a cursor's identity (point size and
//! hotspot); its key, a hash of its CGImage's bytes at natural size with the identity and pixel size;
//! and its picture for a new key, as straight RGBA8 within 256 pixels a side: drawn at 8× its points when it has an
//! `NSPDFImageRep` (vector-backed), else from its largest bitmap representation (2× for `currentSystem` on this Mac,
//! scaled down, never up). Also Core Graphics' cursor visibility and its private cursor seed, looked up at run time.
//!
//! Every function runs inside the caller's autorelease pool: the cursors and images it reads are autoreleased there.

use std::ffi::c_void;
use std::hash::{DefaultHasher, Hasher};

use objc2::encode::{Encoding, RefEncode};
use objc2::rc::AutoreleasePool;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{class, msg_send};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::cursor::ShapeImage;
use crate::platform::common::cursor_pace::{VECTOR_SCALE, shape_px, unpremultiply};

/// Core Graphics' opaque image, named as the Objective-C runtime encodes it.
#[repr(C)]
pub(super) struct CGImage([u8; 0]);
// SAFETY: an opaque struct behind a pointer, encoded as AppKit declares `CGImageRef`.
unsafe impl RefEncode for CGImage {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
}

/// `kCGImageAlphaPremultipliedLast` (RGBA in memory), `kCGInterpolationHigh`.
const RGBA_PREMULTIPLIED: u32 = 1;
const INTERPOLATION_HIGH: i32 = 3;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: *const c_void;
    fn CGCursorIsVisible() -> u32;
    fn CGImageGetWidth(image: *const CGImage) -> usize;
    fn CGImageGetHeight(image: *const CGImage) -> usize;
    fn CGImageGetDataProvider(image: *const CGImage) -> *const c_void;
    fn CGDataProviderCopyData(provider: *const c_void) -> *const c_void;
    fn CFDataGetLength(data: *const c_void) -> isize;
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFRelease(object: *const c_void);
    fn CGColorSpaceCreateWithName(name: *const c_void) -> *const c_void;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits: usize,
        row: usize,
        space: *const c_void,
        info: u32,
    ) -> *const c_void;
    fn CGContextSetInterpolationQuality(context: *const c_void, quality: i32);
    fn CGContextDrawImage(context: *const c_void, rect: NSRect, image: *const CGImage);
}

/// A cursor's point size and hotspot (points, from its image's top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Identity {
    pub(super) pts: (f64, f64),
    pub(super) hotspot: (f64, f64),
}

/// The current system cursor; None when AppKit gives none (shape unknown, never hidden).
pub(super) fn current_system<'p>(_pool: AutoreleasePool<'p>) -> Option<&'p AnyObject> {
    // SAFETY: a class property; the cursor is autoreleased into the caller's pool, which outlives the borrow.
    unsafe {
        let cursor: *mut AnyObject = msg_send![class!(NSCursor), currentSystemCursor];
        cursor.as_ref()
    }
}

/// A cursor's identity.
pub(super) fn identity(cursor: &AnyObject) -> Identity {
    // SAFETY: NSCursor's `image` and `hotSpot`, and NSImage's `size`; the image lives in the caller's pool.
    unsafe {
        let image: *mut AnyObject = msg_send![cursor, image];
        let size: NSSize = msg_send![image, size];
        let hotspot: NSPoint = msg_send![cursor, hotSpot];
        Identity {
            pts: (size.width, size.height),
            hotspot: (hotspot.x, hotspot.y),
        }
    }
}

/// A cursor's key: its CGImage's bytes at natural size (the representation AppKit picks with no context), its
/// identity and its pixel size, hashed. None without a CGImage.
pub(super) fn key(cursor: &AnyObject, id: Identity) -> Option<u64> {
    // SAFETY: the image's CGImage is owned by the image (in the caller's pool); the copied data is released here.
    unsafe {
        let image: *mut AnyObject = msg_send![cursor, image];
        let none = std::ptr::null_mut::<AnyObject>();
        let rect = std::ptr::null_mut::<NSRect>();
        let cg: *const CGImage =
            msg_send![image, CGImageForProposedRect: rect, context: none, hints: none];
        if cg.is_null() {
            return None;
        }
        let mut hash = DefaultHasher::new();
        let data = CGDataProviderCopyData(CGImageGetDataProvider(cg));
        if !data.is_null() {
            let len = CFDataGetLength(data).max(0) as usize;
            hash.write(std::slice::from_raw_parts(CFDataGetBytePtr(data), len));
            CFRelease(data);
        }
        let (pts, hot) = (id.pts, id.hotspot);
        [pts.0, pts.1, hot.0, hot.1]
            .iter()
            .for_each(|v| hash.write_u64(v.to_bits()));
        hash.write_usize(CGImageGetWidth(cg) << 16 | CGImageGetHeight(cg));
        Some(hash.finish())
    }
}

/// A cursor's picture for a new key: vector-backed at 8× its points, else its largest bitmap (see the module notes).
/// AppKit makes the CGImage for the size asked (`CGImageForProposedRect:`), picking the representation itself, so no
/// representation class is assumed.
pub(super) fn picture(cursor: &AnyObject, id: Identity) -> Option<ShapeImage> {
    let pdf = AnyClass::get(c"NSPDFImageRep");
    // SAFETY: NSImage's representations and their pixel sizes, and its CGImage for a size, all in the caller's pool.
    let (cg, px) = unsafe {
        let image: *mut AnyObject = msg_send![cursor, image];
        let reps: *mut AnyObject = msg_send![image, representations];
        let count: usize = msg_send![reps, count];
        let (mut vector, mut largest) = (false, (0isize, 0isize));
        for i in 0..count {
            let rep: *mut AnyObject = msg_send![reps, objectAtIndex: i];
            vector |= pdf.is_some_and(|c| msg_send![rep, isKindOfClass: c]);
            let size: (isize, isize) = (msg_send![rep, pixelsWide], msg_send![rep, pixelsHigh]);
            if size.0 * size.1 > largest.0 * largest.1 {
                largest = size;
            }
        }
        let (asked, px) = if vector {
            let px = shape_px(id.pts, VECTOR_SCALE);
            ((f64::from(px.0), f64::from(px.1)), px)
        } else {
            let size = (largest.0 as f64, largest.1 as f64);
            (size, shape_px(size, 1.0))
        };
        let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(asked.0, asked.1));
        let (rect, none): (*mut NSRect, *mut AnyObject) = (&mut rect, std::ptr::null_mut());
        let cg: *const CGImage =
            msg_send![image, CGImageForProposedRect: rect, context: none, hints: none];
        (cg, px)
    };
    let rgba = draw(cg, px)?;
    let scale = |px: u16, pts: f64| f64::from(px) / pts.max(f64::MIN_POSITIVE);
    let hotspot_px = (
        (id.hotspot.0 * scale(px.0, id.pts.0)).clamp(0.0, px.0.into()) as f32,
        (id.hotspot.1 * scale(px.1, id.pts.1)).clamp(0.0, px.1.into()) as f32,
    );
    Some(ShapeImage {
        px,
        rgba,
        hotspot_px,
        pts: id.pts.1 as f32,
        sys: None,
    })
}

/// `cg` drawn as sRGB RGBA8 at `px`, straight alpha.
fn draw(cg: *const CGImage, px: (u16, u16)) -> Option<Vec<u8>> {
    if cg.is_null() {
        return None;
    }
    let (w, h) = (usize::from(px.0), usize::from(px.1));
    let mut rgba = vec![0u8; w * h * 4];
    // SAFETY: a context over our own buffer (w × h × 4 bytes, rows of w × 4), released before the buffer is used.
    unsafe {
        let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        let context = CGBitmapContextCreate(
            rgba.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            space,
            RGBA_PREMULTIPLIED,
        );
        if !space.is_null() {
            CFRelease(space);
        }
        if context.is_null() {
            return None;
        }
        CGContextSetInterpolationQuality(context, INTERPOLATION_HIGH);
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w as f64, h as f64));
        CGContextDrawImage(context, rect, cg);
        CFRelease(context);
    }
    unpremultiply(&mut rgba);
    Some(rgba)
}

/// Whether Core Graphics shows the cursor (`CGCursorIsVisible`): the only "hidden" there is.
pub(super) fn visible() -> bool {
    // SAFETY: a plain query.
    unsafe { CGCursorIsVisible() != 0 }
}

/// Core Graphics' cursor seed (`CGSCurrentCursorSeed`, private: it moves when the cursor changes), looked up at run
/// time; None when this macOS hasn't it (the public looks then).
pub(super) fn seed() -> Option<extern "C" fn() -> i32> {
    // SAFETY: a lookup by name in the loaded images.
    let f = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"CGSCurrentCursorSeed".as_ptr()) };
    // SAFETY: the symbol is a C function taking nothing and returning an int (measured so on macOS).
    (!f.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, extern "C" fn() -> i32>(f) })
}
