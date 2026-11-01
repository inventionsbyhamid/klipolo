//! The previews' JPEG on the Mac: Core Image's `JPEGRepresentationOfImage:colorSpace:options:`
//! of a BGRA pixel buffer, at quality 0.6 in sRGB, on the GPU where Core Image runs (sent through the Objective-C
//! runtime, as cidre has no binding for it). Check camera's preview and the live view each keep one, used from their
//! stream's serial queue.

use cidre::{arc, ci, cv, ns};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
use objc2_foundation::NSData;

use crate::pipe::PipeError;

const QUALITY: f64 = 0.6;

/// Core Image's JPEG of a pixel buffer.
pub(super) struct Jpeg {
    context: arc::R<ci::Context>,
    space: CFRetained<CGColorSpace>,
    options: arc::R<ns::Dictionary<ns::String, ns::Id>>,
}

impl Jpeg {
    pub(super) fn new() -> Result<Self, PipeError> {
        let context = ci::Context::with_opts(None).ok_or_else(|| os("Core Image context"))?;
        // SAFETY: an immutable CoreGraphics string constant.
        let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
            .ok_or_else(|| os("sRGB colour space"))?;
        // SAFETY: an immutable ImageIO string constant.
        let key = unsafe { kCGImageDestinationLossyCompressionQuality };
        let quality = ns::Number::with_f64(QUALITY);
        let options = ns::Dictionary::with_keys_values(&[key.as_ns()], &[quality.as_id_ref()]);
        Ok(Self {
            context,
            space,
            options,
        })
    }

    /// The JPEG's bytes, or None when Core Image made none.
    pub(super) fn encode(&self, image: &cv::PixelBuf) -> Option<Vec<u8>> {
        let picture = ci::Image::with_cv_pixel_buf(image, None)?;
        let object = |o: &ns::Id| -> &AnyObject {
            // SAFETY: cidre's `ns::Id` is an Objective-C object, the same one as `AnyObject`.
            unsafe { &*(o as *const ns::Id).cast::<AnyObject>() }
        };
        // SAFETY: `-[CIContext JPEGRepresentationOfImage:colorSpace:options:]` takes a CIImage, a CGColorSpaceRef and an
        // NSDictionary and returns an autoreleased NSData (or nil); each argument is what it expects.
        let data: Option<Retained<NSData>> = unsafe {
            msg_send![
                object(self.context.as_id_ref()),
                JPEGRepresentationOfImage: object(picture.as_id_ref()),
                colorSpace: &*self.space,
                options: object(self.options.as_id_ref())
            ]
        };
        data.map(|d| d.to_vec())
    }
}

// SAFETY: a CIContext may be used from any thread (one render at a time here: the stream's serial queue); the colour
// space and the options are immutable.
unsafe impl Send for Jpeg {}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    /// The JPEG's quality key (0-1), which `JPEGRepresentationOfImage:colorSpace:options:` reads.
    static kCGImageDestinationLossyCompressionQuality: &'static cidre::cf::String;
}

fn os(what: &str) -> PipeError {
    PipeError::Os(format!("the preview's {what}"))
}
