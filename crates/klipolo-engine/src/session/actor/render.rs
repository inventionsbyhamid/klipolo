//! The actor's render path: plan the still at the playhead, render it, and hand it to the presenter (the socket
//! stamps the trailer and publishes; the native layer shows it at its due refresh).

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::Actor;
use crate::api::EngineEvent;
use crate::frames::trailer::{FLAG_PLAYBACK, FrameMeta, now_ms};
use crate::plan::{FramePlan, Layer, TextOwner, frame_plan};
use crate::present::Frame;
use crate::session::{Pixels, PreviewBackend, RenderRequest, Rendered};

/// Frames the GPU may be drawing at once (Apple: two to three in flight). A third render first publishes the oldest.
const MAX_IN_FLIGHT: usize = 2;

pub(crate) struct InFlight {
    pixels: Pixels,
    meta: FrameMeta,
    /// A playback frame: when it was planned (its publish measures the engine's share of the delivery delay).
    ticked: Option<Instant>,
}

/// The oldest in-flight frame, once it is ready to show; never resolves when nothing is in flight. Cancel-safe.
pub(super) async fn wait_front(inflight: &mut VecDeque<InFlight>) -> Result<Frame, String> {
    match inflight.front_mut() {
        Some(front) => front.pixels.frame().await,
        None => std::future::pending().await,
    }
}

impl<B: PreviewBackend> Actor<B> {
    pub(super) async fn render_still(&mut self) {
        self.dirty = false;
        if !self.visible {
            return;
        }
        let (Some(cur), Some(view)) = (&self.current, self.view) else {
            return;
        };
        let at_us = self.clamp_time(self.playhead_us);
        let mut plan = frame_plan(&cur.ctx, at_us, view.format);
        hide_edited(&mut plan, self.editing.as_deref());
        let meta = FrameMeta {
            seq: self.seq,
            session: self.session,
            revision: cur.revision as u32,
            time_us: at_us,
            ..FrameMeta::default()
        };
        let output = self.presenter.output(false);
        // The seek this still shows: a newer one arriving while it is drawn stops its reads early (`Superseded`).
        let seek = u64::from(self.p.seek.borrow().1);
        self.drawing_still.store(seek, Ordering::Relaxed);
        self.render(
            RenderRequest {
                plan,
                width: view.width,
                height: view.height,
                output,
            },
            meta,
        )
        .await;
        self.drawing_still.store(super::NO_STILL, Ordering::Relaxed);
    }

    /// Submits a frame. It is presented when ready (`publish_front`: read-back pixels once the GPU is done, a native
    /// slot at once), in submission order.
    pub(super) async fn render(&mut self, req: RenderRequest, mut meta: FrameMeta) {
        let ticked = self.ticked.take();
        while self.inflight.len() >= MAX_IN_FLIGHT {
            let frame = wait_front(&mut self.inflight).await;
            self.publish_front(frame);
        }
        match self.p.backend.render(req).await {
            Ok(Rendered {
                pixels,
                width,
                height,
                stride,
                format,
                warnings,
                missing,
                not_answering,
            }) => {
                self.last_error = None;
                let note = self
                    .waiting
                    .frame(&not_answering, tokio::time::Instant::now());
                self.note(note);
                for warning in warnings {
                    if self.warned.insert(warning.clone()) {
                        // The frame's own notes are codes (the app words them); the rest are still English.
                        match crate::api::frame_notice(&warning) {
                            Some(notice) => self.emit(EngineEvent::Notice { notice }),
                            None => self.emit(EngineEvent::Warning { message: warning }),
                        }
                    }
                }
                for path in missing {
                    if self.missing_said.insert(path.clone()) {
                        self.emit(EngineEvent::Notice {
                            notice: crate::api::missing_notice(&path),
                        });
                    }
                }
                meta.width = width;
                meta.height = height;
                meta.stride = stride;
                meta.format = format;
                let ready = pixels.is_ready() && self.inflight.is_empty();
                self.inflight.push_back(InFlight {
                    pixels,
                    meta,
                    ticked,
                });
                if ready {
                    let frame = wait_front(&mut self.inflight).await;
                    self.publish_front(frame);
                }
            }
            Err(message) => self.report_error(message),
        }
        self.emit_guard_events();
    }

    /// Presents the oldest in-flight frame (or reports why there is none).
    pub(super) fn publish_front(&mut self, frame: Result<Frame, String>) {
        let Some(InFlight {
            mut meta, ticked, ..
        }) = self.inflight.pop_front()
        else {
            return;
        };
        match frame {
            Ok(frame) => {
                if let Some(ticked) = ticked {
                    let us = ticked.elapsed().as_micros() as f64;
                    self.pipeline_us += (us.min(500_000.0) - self.pipeline_us) * 0.1;
                    self.update_lead();
                }
                meta.sent_ms = now_ms();
                let playback = meta.flags & FLAG_PLAYBACK != 0;
                self.presenter.present(frame, meta);
                // The run's late-frame warm-up starts at its first frame shown.
                if playback {
                    self.note_presented(Instant::now());
                }
            }
            Err(message) => self.report_error(message),
        }
    }

    fn report_error(&mut self, message: String) {
        if self.last_error.as_deref() != Some(message.as_str()) {
            self.emit(EngineEvent::Error {
                message: message.clone(),
            });
            self.last_error = Some(message);
        }
    }
}

/// The text being typed on the canvas is drawn by the editor over it, never twice: the preview
/// leaves that one layer out. Export never sees it (an export renders its own job).
pub(super) fn hide_edited(plan: &mut FramePlan, editing: Option<&str>) {
    if let Some(id) = editing {
        plan.layers.retain(
            |l| !matches!(l, Layer::Text(t) if t.owner == TextOwner::Layer && &*t.id == id),
        );
    }
}

/// The requested size scaled down (keeping its shape) to at most `max_pixels` and 4096 on either side, in even
/// numbers of at least 2. The UI asks for CSS size × devicePixelRatio already within budget; this is the guard.
pub(super) fn within_budget(width: u32, height: u32, max_pixels: u32) -> (u32, u32) {
    let (w, h) = (f64::from(width.max(2)), f64::from(height.max(2)));
    let scale = (f64::from(max_pixels.max(4)) / (w * h))
        .sqrt()
        .min(4096.0 / w)
        .min(4096.0 / h)
        .min(1.0);
    let even = |n: f64| ((n / 2.0).floor() as u32 * 2).max(2);
    (even(w * scale), even(h * scale))
}
