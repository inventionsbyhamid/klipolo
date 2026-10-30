//! Attaching the native presenter: the app made a native view under the webview (a CAMetalLayer on macOS);
//! frames go there instead of the socket from the next one on.

use std::sync::{Arc, PoisonError};

use super::Actor;
use crate::api::EngineEvent;
use crate::frames::trailer::FLAG_PLAYBACK;
use crate::platform::NativeSurface;
use crate::present::PresenterKind;
use crate::present::native::ShownSink;
use crate::session::PreviewBackend;

impl<B: PreviewBackend> Actor<B> {
    /// Starts the native presenter on `surface`; an error leaves the socket in charge (the UI keeps drawing frames).
    pub(super) fn attach_native(&mut self, surface: NativeSurface) -> Result<(), String> {
        if self.presenter.kind() == PresenterKind::Native && !self.presenter.failed() {
            return Ok(());
        }
        let sink = self.p.sink.clone();
        let shown: ShownSink = Arc::new(move |meta, shown_ms, presented| {
            let sink = sink.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(sink) = sink {
                sink(EngineEvent::Shown {
                    session: meta.session,
                    seq: meta.seq,
                    at_us: meta.time_us,
                    playback: meta.flags & FLAG_PLAYBACK != 0,
                    sent_ms: meta.sent_ms,
                    shown_ms,
                    presented,
                });
            }
        });
        let presenter = self.p.backend.attach_native(surface, shown)?;
        self.present_log = Some(presenter.log());
        self.presenter = Box::new(presenter);
        self.presenter.set_visible(self.visible);
        // Frames on their way to the socket are dropped; the still is drawn again into the layer. Nothing measures a
        // delivery delay here: frames are queued as soon as they are drawn. The display link fires about a refresh
        // before the refresh it prepares, so frames are drawn that much further ahead to be queued by then.
        self.inflight.clear();
        self.present_us = super::VSYNC_US;
        self.update_lead();
        self.dirty = true;
        Ok(())
    }
}
