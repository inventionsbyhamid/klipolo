//! What a screen pipe does with each buffer ScreenCaptureKit delivers, on the stream's own queue, never
//! blocking it:
//! - every buffer counts by its status (`Complete`, `Idle`, `Blank`, `Suspended`; the stall rule reads them all);
//! - a `Complete` frame is copied into our pool at once (the capture buffer goes back to ScreenCaptureKit in
//!   microseconds) and handed to the encoder thread over a bounded channel; a full one drops the frame, counted as the
//!   encoder's (it was behind), which breaks no GOP since the frame was never encoded;
//! - a `Complete` frame that changed only Klipolo's own windows (the camera bubble, the HUD; `damage`) is counted as
//!   `Idle` and never encoded, unless a keyframe is wanted, it follows a blank picture, or no real frame was sent yet;
//! - an `Idle` buffer is only counted: the still screen's re-feed is the encoder thread's, on its own clock, since on a
//!   fully still screen ScreenCaptureKit stops sending even idle buffers (3.4 s without one in the smoke run);
//! - every buffer's arrival is the stream's sign of life (`Shared::alive`), which the encoder thread watches;
//! - a blank or suspended picture is reported once as `Blank(true)` and its end as `Blank(false)`;
//! - while paused, buffers are counted and released;
//! - Presenter Overlay in the picture warns once (`capture.health.presenterOverlay`).
//!
//! The cursor isn't sampled here: it is its own source (`cursor::MacCursorSource`).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;

use cidre::{cf, cm, sc};

use super::super::handoff::{Shared, ToEncoder};
use super::super::host_time::host_ns_of;
use super::super::pool::SurfacePool;
use super::damage::OwnDamage;
use crate::pipe::PipeEvent;

pub(in crate::platform::macos) struct ScreenFeed {
    pub(super) pool: SurfacePool,
    pub(super) encoder: SyncSender<ToEncoder>,
    pub(super) shared: Arc<Shared>,
    pub(super) events: SyncSender<PipeEvent>,
    pub(super) blank: bool,
    pub(super) overlay_noted: bool,
    pub(super) overlay_logged: bool,
    /// Frames that changed only Klipolo's own windows (`damage`), and whether the file's picture is fresh.
    pub(super) damage: OwnDamage,
}

impl ScreenFeed {
    /// One screen buffer.
    pub(in crate::platform::macos) fn screen(&mut self, sample: &cm::SampleBuf) {
        let s = &self.shared;
        if s.stopped.load(Ordering::Acquire) {
            return;
        }
        s.delivered.fetch_add(1, Ordering::Relaxed);
        s.alive
            .store(klipolo_platform::clock::host_ns(), Ordering::Release);
        let at = host_ns_of(sample.pts());
        let paused = s.paused.load(Ordering::Acquire);
        if paused {
            // The screen may change while its frames go uncounted: the first frame after the resume is encoded.
            self.damage.paused();
        }
        match status_of(sample) {
            sc::FrameStatus::Complete if paused => {
                s.complete.fetch_add(1, Ordering::Relaxed);
                self.picture_back();
            }
            sc::FrameStatus::Complete => {
                let after_blank = self.blank;
                self.picture_back();
                self.overlay(sample);
                let key_wanted = self.shared.key_wanted.load(Ordering::Acquire);
                // Only Klipolo's windows changed (the camera bubble, the HUD): an idle frame, never encoded; the encoder
                // thread re-feeds the last real one.
                // The cursor's live switch can put it in the frames mid-run.
                let cursor_in_frames = self.shared.cursor_in_frames.load(Ordering::Acquire);
                self.damage.shows_cursor(cursor_in_frames);
                if self.damage.idle(sample, key_wanted, after_blank) {
                    self.shared.idle.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                self.shared.complete.fetch_add(1, Ordering::Relaxed);
                self.complete(sample, at);
            }
            sc::FrameStatus::Idle => {
                s.idle.fetch_add(1, Ordering::Relaxed);
            }
            sc::FrameStatus::Blank | sc::FrameStatus::Suspended => {
                s.blank.fetch_add(1, Ordering::Relaxed);
                if !std::mem::replace(&mut self.blank, true) {
                    let _ = self.events.try_send(PipeEvent::Blank(true));
                }
            }
            sc::FrameStatus::Started | sc::FrameStatus::Stopped => {}
        }
    }

    fn picture_back(&mut self) {
        if std::mem::replace(&mut self.blank, false) {
            let _ = self.events.try_send(PipeEvent::Blank(false));
        }
    }

    /// A real frame to the encoder. One that doesn't get there (no picture, a full pool or channel) leaves the file's
    /// picture stale, so the next frame is encoded whatever changed.
    fn complete(&mut self, sample: &cm::SampleBuf, at: u64) {
        let frame = (sample.image_buf()).and_then(|image| self.shared.copy(&mut self.pool, image));
        let Some(frame) = frame else {
            self.damage.sent(false);
            return;
        };
        let queued = self.shared.send(&self.encoder, frame, at);
        self.damage.sent(queued);
    }

    /// Presenter Overlay shows the camera inside the picture: warn once (its privacy alert is off). The attachment's
    /// rectangle is what counts, not its presence.
    fn overlay(&mut self, sample: &cm::SampleBuf) {
        if self.overlay_noted {
            return;
        }
        let key = sc::FrameInfo::presenter_overlay_content_rect().as_cf();
        let Some(rect) = info_of(sample).and_then(|i| i.value(key)) else {
            return;
        };
        let side = |name: &cf::String| {
            (rect.try_as_dictionary())
                .and_then(|d| d.value(name))
                .and_then(|n| n.try_as_number())
                .and_then(|n| n.to_f64())
                .unwrap_or(0.0)
        };
        let (w, h) = (
            side(cidre::cfstr!(c"Width")),
            side(cidre::cfstr!(c"Height")),
        );
        if !std::mem::replace(&mut self.overlay_logged, true) {
            tracing::info!(w, h, "screen: Presenter Overlay's content rectangle");
        }
        if w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 {
            self.overlay_noted = true;
            let note = PipeEvent::Note("capture.health.presenterOverlay".into());
            let _ = self.events.try_send(note);
        }
    }
}

/// The frame's own information (`SCStreamFrameInfo`): the first sample's attachments.
pub(super) fn info_of(
    sample: &cm::SampleBuf,
) -> Option<&cidre::cf::DictionaryOf<cidre::cf::String, cidre::cf::Plist>> {
    let all = sample.attaches(false)?;
    (!all.is_empty()).then(|| &all[0])
}

/// The buffer's status; one without it is `Complete` when it holds a picture, else `Idle`. The live view reads it too.
pub(in crate::platform::macos) fn status_of(sample: &cm::SampleBuf) -> sc::FrameStatus {
    let key = sc::FrameInfo::status().as_cf();
    let raw = info_of(sample)
        .and_then(|i| i.value(key))
        .and_then(|v| v.try_as_number())
        .and_then(|n| n.to_i64());
    match raw {
        Some(0) => sc::FrameStatus::Complete,
        Some(1) => sc::FrameStatus::Idle,
        Some(2) => sc::FrameStatus::Blank,
        Some(3) => sc::FrameStatus::Suspended,
        Some(4) => sc::FrameStatus::Started,
        Some(5) => sc::FrameStatus::Stopped,
        _ if sample.image_buf().is_some() => sc::FrameStatus::Complete,
        _ => sc::FrameStatus::Idle,
    }
}
