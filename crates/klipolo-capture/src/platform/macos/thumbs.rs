//! The Record dialog's stills on the Mac (`Backend::still`): one ScreenCaptureKit screenshot of a display or of an app's
//! windows (`SCScreenshotManager`, macOS 14 and later) with the take's own content filter (`filter`: Klipolo's windows
//! and the notification apps left out of a display, an app's windows on their display), scaled by ScreenCaptureKit
//! itself to the width asked, in the target's shape, and made a JPEG through Core Image (`jpeg`), as the live view's
//! frames are. Nothing is written.
//!
//! **Never a prompt:** the guard first, then Screen Recording's preflight (`CGPreflightScreenCaptureAccess`, which never
//! asks); a refusal calls nothing more. Only then is the content list read and the screenshot taken (macOS's own
//! periodic "continue to allow" confirmation may appear then: macOS's own reminder, not a request Klipolo makes).
//!
//! The screenshot comes as a sample buffer, whose BGRA pixel buffer `jpeg` encodes as it does the live view's frames.
//! Each still runs inside its own autorelease pool, so the system's objects it makes (the content list, the screenshot,
//! Core Image's image and its JPEG) are freed with it, on a thread that has no run loop to drain them. Each still reads
//! the content list again, so a window that opened or an app that quit since the last one shows as it is now. Memory:
//! one BGRA buffer of at most 320 pixels wide (about 400 KB for a 16:10 display), one Core Image context kept for the
//! worker's life, and one JPEG.

use std::sync::mpsc::sync_channel;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use cidre::{arc, blocks, cg, cm, cv, ns, objc, sc};
use objc2_core_graphics::CGPreflightScreenCaptureAccess;

use super::filter;
use super::jpeg::Jpeg;
use crate::backend::{PreviewFrame, Target};
use crate::guard::assert_real_capture_allowed;
use crate::pipe::PipeError;
use crate::platform::common::sck::USER_DECLINED;
use crate::vendor::cap::sck::StreamCfgBuilder;

/// How long a screenshot may take before its turn is given up.
const SHOT_WAIT: Duration = Duration::from_secs(2);

/// The stills' JPEG maker, made at the first still and kept.
#[derive(Default)]
pub(super) struct Stills {
    jpeg: Mutex<Option<Jpeg>>,
}

impl Stills {
    /// A still of `target` `width` pixels wide (a narrower target keeps its own width), as a JPEG. Only while Screen
    /// Recording is allowed already: `PermissionDenied` otherwise, with nothing read.
    pub(super) fn take(&self, target: &Target, width: u32) -> Result<PreviewFrame, PipeError> {
        assert_real_capture_allowed("a still of the screen");
        if matches!(target, Target::Camera { .. } | Target::Portal { .. }) {
            return Err(PipeError::Unsupported);
        }
        if !CGPreflightScreenCaptureAccess() {
            return Err(PipeError::PermissionDenied);
        }
        objc::ar_pool(|| {
            let content = filter::content()?;
            let picked = filter::pick(&content, target)?;
            drop(content);
            let size = still_size(picked.pixels, width);
            let shot = shoot(&picked.filter, &still_config(size, picked.window))?;
            let image = shot
                .image_buf()
                .ok_or_else(|| PipeError::Os("the still has no picture".into()))?;
            let mut jpeg = self.jpeg.lock().unwrap_or_else(PoisonError::into_inner);
            let jpeg = match &mut *jpeg {
                Some(jpeg) => jpeg,
                none => none.insert(Jpeg::new()?),
            };
            let bytes = jpeg
                .encode(image)
                .ok_or_else(|| PipeError::Os("the still's JPEG".into()))?;
            Ok(PreviewFrame {
                jpeg: bytes,
                width: image.width() as u32,
                height: image.height() as u32,
            })
        })
    }
}

/// `pixels` scaled to `width` (never wider than they are), in their shape: at least 2 × 2.
fn still_size(pixels: (u32, u32), width: u32) -> (u32, u32) {
    let (w, h) = (pixels.0.max(1), pixels.1.max(1));
    let wide = width.min(w).max(2);
    let high = (u64::from(wide) * u64::from(h) / u64::from(w)).max(2);
    (wide, high as u32)
}

/// One picture of `size`, BGRA in sRGB, no cursor; a window's picture fitted into it.
fn still_config(size: (u32, u32), window: bool) -> arc::R<sc::StreamCfg> {
    let mut c = StreamCfgBuilder::default();
    c.set_width(size.0 as usize);
    c.set_height(size.1 as usize);
    c.set_pixel_format(cv::PixelFormat::_32_BGRA);
    c.set_color_space_name(cg::color_space::names::srgb());
    c.set_shows_cursor(false);
    c.set_presenter_overlay_alert(sc::PresenterOverlayAlertSetting::Never);
    if window {
        c.set_scales_to_fit(true);
        c.set_preserves_aspect_ratio(true);
    }
    c.build()
}

/// ScreenCaptureKit's screenshot of `filter`, waited for at most `SHOT_WAIT`.
fn shoot(
    filter: &sc::ContentFilter,
    cfg: &sc::StreamCfg,
) -> Result<arc::R<cm::SampleBuf>, PipeError> {
    let (tx, rx) = sync_channel(1);
    let mut done = blocks::ResultCh::<cm::SampleBuf>::new2(
        move |shot: Option<&cm::SampleBuf>, error: Option<&ns::Error>| {
            let answer = match (shot, error) {
                (Some(shot), _) => Ok(shot.retained()),
                (None, Some(e)) if e.code() == USER_DECLINED => Err(PipeError::PermissionDenied),
                (None, Some(e)) => Err(PipeError::Os(format!("the still: {}", e.code()))),
                (None, None) => Err(PipeError::Os("the still: no answer".into())),
            };
            let _ = tx.try_send(answer);
        },
    );
    sc::ScreenshotManager::capture_sample_buf_ch(filter, cfg, Some(&mut done));
    rx.recv_timeout(SHOT_WAIT)
        .unwrap_or_else(|_| Err(PipeError::Os("the still didn't come".into())))
}
