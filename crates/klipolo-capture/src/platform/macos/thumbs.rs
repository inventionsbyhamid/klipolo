//! The Record dialog's stills on the Mac (`Backend::still`): one ScreenCaptureKit screenshot of a display or of an app's
//! windows (`SCScreenshotManager`, macOS 14 and later) with the take's own content filter (`filter`: Klipolo's windows
//! and the notification apps left out of a display, an app's windows on their display), scaled by ScreenCaptureKit
//! itself to the width asked, in the target's shape, and made a JPEG through Core Image (`jpeg`), as the live view's
//! frames are. Nothing is written.
//!
//! **Never a prompt:** the guard first, then Screen Recording's preflight (`CGPreflightScreenCaptureAccess`, which never
//! asks); a refusal calls nothing more. Only then is the content list read and the screenshot taken (macOS's own
//! periodic "continue to allow" confirmation may appear then: macOS's own reminder, not a request Klipolo makes; a
//! "no" to it comes back as `PermissionDenied`, which ends the stills).
//!
//! **Its cost:** the content list (a round trip to the system's capture service, which lists every window) is read
//! once a round (`ROUND`, the stills' `THUMB_EVERY`, counted from the read's end) and kept with the filters picked from
//! it, so it is at most a round old, as the cards are; a read that failed is kept as long, so a service that fails at
//! once isn't asked again at every still (`common::stills_service`). One request to the service is in flight at a
//! time: a screenshot or a content read the stills stopped waiting for still runs there, so until it answers no new one
//! is sent, and that turn fails as `Os` (tried again after the worker's rest). Each still runs inside its own
//! autorelease pool, so the system's objects it makes (the screenshot, Core Image's image and its JPEG) are freed with
//! it, on a thread that has no run loop to drain them.
//! The screenshot comes as a sample buffer, whose BGRA pixel buffer `jpeg` encodes as it does the live view's frames.
//! Memory: one round's content list and filters, one BGRA buffer at most 320 pixels wide (about 400 KB for a 16:10
//! display), one Core Image context kept for the worker's life, and one JPEG.

use std::sync::mpsc::sync_channel;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use cidre::{arc, blocks, cg, cm, cv, ns, objc, sc};
use objc2_core_graphics::CGPreflightScreenCaptureAccess;

use super::filter::{self, Picked};
use super::jpeg::Jpeg;
use crate::backend::{PreviewFrame, Target};
use crate::guard::assert_real_capture_allowed;
use crate::pipe::PipeError;
use crate::platform::common::sck::USER_DECLINED;
use crate::platform::common::stills_service::{InFlight, RoundCache};
use crate::run::thumbs::THUMB_EVERY;
use crate::vendor::cap::sck::StreamCfgBuilder;

/// How long a screenshot may take before its turn is given up.
const SHOT_WAIT: Duration = Duration::from_secs(2);
/// How long one content list, and the filters picked from it, serve: one round of the stills.
const ROUND: Duration = THUMB_EVERY;

/// The stills' JPEG maker, made at the first still and kept, the round's content list, and the request in flight.
pub(super) struct Stills {
    jpeg: Mutex<Option<Jpeg>>,
    round: Mutex<RoundCache<Round>>,
    service: InFlight,
}

impl Default for Stills {
    fn default() -> Self {
        Self {
            jpeg: Mutex::default(),
            round: Mutex::new(RoundCache::new(ROUND)),
            service: InFlight::default(),
        }
    }
}

/// One round's content list and the filters picked from it so far.
struct Round {
    content: arc::R<sc::ShareableContent>,
    picked: Vec<(Target, Picked)>,
}

// SAFETY: the content list and the content filters are immutable once made (cidre marks the list `Send`), and they
// are used only under `Stills::round`'s lock, one still at a time.
unsafe impl Send for Round {}

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
            let shot = self.through(target, |picked| {
                let size = still_size(picked.pixels, width);
                shoot(
                    &picked.filter,
                    &still_config(size, picked.window),
                    &self.service,
                )
            })?;
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

    /// `with` `target`'s filter, picked from this round's content list, which is read again once the round is over.
    fn through<R>(
        &self,
        target: &Target,
        with: impl FnOnce(&Picked) -> Result<R, PipeError>,
    ) -> Result<R, PipeError> {
        let mut round = self.round.lock().unwrap_or_else(PoisonError::into_inner);
        let round = round.get(&Instant::now, || {
            let content = filter::content_answering(self.service.begin()?)?;
            Ok(Round {
                content,
                picked: vec![],
            })
        })?;
        let at = match round.picked.iter().position(|(t, _)| t == target) {
            Some(at) => at,
            None => {
                let picked = filter::pick(&round.content, target)?;
                round.picked.push((target.clone(), picked));
                round.picked.len() - 1
            }
        };
        with(&round.picked[at].1)
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

/// ScreenCaptureKit's screenshot of `filter`, waited for at most `SHOT_WAIT`; `Os` at once while `service` has a
/// request that hasn't answered.
fn shoot(
    filter: &sc::ContentFilter,
    cfg: &sc::StreamCfg,
    service: &InFlight,
) -> Result<arc::R<cm::SampleBuf>, PipeError> {
    let mut answered = service.begin()?;
    let (tx, rx) = sync_channel(1);
    let mut done = blocks::ResultCh::<cm::SampleBuf>::new2(
        move |shot: Option<&cm::SampleBuf>, error: Option<&ns::Error>| {
            answered.answer();
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
