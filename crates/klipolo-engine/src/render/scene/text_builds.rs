//! The scene text cache's builds: asked of the text build thread (`text_worker.rs`) at
//! most [`MAX_IN_FLIGHT`] at once and each once, and landed when they answer: their draws' buffers made on the render
//! thread (the layout, the raster and the atlas's upload are the thread's) and kept in
//! `text_cache.rs`'s entries. A reply nobody waits for any more (its entries were cleared) is dropped. A thread that is
//! gone (it failed to start or stopped) is remembered (`gone`), so the frame says its text can't be built; and an
//! export's frame waits out the builds asked ahead of its own rather than draw without it
//! (`request_waiting`). A moving scene without text asks for its survey alone the same way (at most
//! [`MAX_IN_FLIGHT`] at once too), kept until its instance takes it (`take_survey`).

use std::sync::Arc;

use tokio::sync::Notify;

use super::Drawn;
use super::sizes::Sizes;
use super::text::MAX_EM_PX;
use super::text_cache::{MAX_IN_FLIGHT, Place, TextCache, TextEntry, Upload, Whose};
use super::text_gpu::TextGpu;
use super::text_layouts::TextLayouts;
use super::text_worker::{Answer, Built, Gone, TextReply, TextRequest, TextWorker};
use crate::plan::SceneLayer;

/// A build asked and not answered yet.
pub(super) struct Pending {
    id: u64,
    whose: Whose,
    place: Place,
    /// Its fonts wait for this computer's (an export's).
    wait: bool,
}

impl TextCache {
    /// Asks the build thread (started on the first ask) for `layer`'s text at `place`, its fonts waiting for this
    /// computer's when `wait` (an export's), once: the pending build's id, or None when the thread is gone or
    /// `MAX_IN_FLIGHT` builds are asked.
    pub(super) fn request(
        &mut self,
        layer: &SceneLayer,
        place: Place,
        wait: bool,
        redraw: Option<&Arc<Notify>>,
        (device, queue, _): Upload,
    ) -> Option<u64> {
        if let Some(p) = self
            .pending
            .iter()
            .find(|p| p.place == place && p.whose.is(layer))
        {
            return Some(p.id);
        }
        if self.pending.len() >= MAX_IN_FLIGHT {
            return None;
        }
        let (id, request) = self.ask(layer, place, (device, queue), redraw);
        let request = TextRequest { wait, ..request };
        self.send(request)?;
        self.pending.push(Pending {
            id,
            whose: Whose::of(layer),
            place,
            wait,
        });
        Some(id)
    }

    /// Asks the build thread for the survey alone of `layer` (a moving scene without text) drawn at `place`: its id, or
    /// None when the thread is gone or `MAX_IN_FLIGHT` surveys are asked.
    pub(super) fn request_survey(
        &mut self,
        layer: &SceneLayer,
        place: Place,
        redraw: Option<&Arc<Notify>>,
        (device, queue): (&wgpu::Device, &wgpu::Queue),
    ) -> Option<u64> {
        if self.surveys_asked.len() >= MAX_IN_FLIGHT {
            return None;
        }
        let (id, request) = self.ask(layer, place, (device, queue), redraw);
        self.send(TextRequest {
            survey: true,
            ..request
        })?;
        self.surveys_asked.push(id);
        Some(id)
    }

    /// A survey that landed for ask `id`: Some(None) when it couldn't be made there; None until it lands.
    pub(super) fn take_survey(&mut self, id: u64) -> Option<Option<Sizes>> {
        let k = self.surveys.iter().position(|s| s.0 == id)?;
        Some(self.surveys.swap_remove(k).1)
    }

    fn send(&mut self, request: TextRequest) -> Option<()> {
        if self.worker.as_ref()?.send(request).is_err() {
            self.gone = true;
            return None;
        }
        Some(())
    }

    /// A request's id and its text request for `layer` at `place` (the thread started on the first ask).
    fn ask(
        &mut self,
        layer: &SceneLayer,
        place: Place,
        (device, queue): (&wgpu::Device, &wgpu::Queue),
        redraw: Option<&Arc<Notify>>,
    ) -> (u64, TextRequest) {
        if self.worker.is_none() {
            let gpu = (device.clone(), queue.clone());
            self.worker = Some(TextWorker::start(redraw.cloned(), gpu));
        }
        self.next_id += 1;
        let id = self.next_id;
        let (w, h) = place.at.size;
        let request = TextRequest {
            id,
            wait: false,
            survey: false,
            scene: layer.scene.clone(),
            params: layer.params.clone(),
            fills_animate: layer.fills_animate,
            images: layer.images.clone(),
            language: layer.language.clone(),
            drawn: Drawn {
                size: place.at.size,
                k: f64::from_bits(place.k),
            },
            max_px: (w.max(h) as f32).min(MAX_EM_PX),
            #[cfg(any(test, feature = "test-support"))]
            delay: self.delay,
        };
        (id, request)
    }

    /// [`request`](Self::request) for a frame that waits for its text (an export's): while `MAX_IN_FLIGHT` builds
    /// asked ahead of it (the lookahead's) fill the thread's queue, the oldest is waited for and kept, then it is asked.
    /// None only when the thread is gone.
    pub(super) fn request_waiting(
        &mut self,
        layer: &SceneLayer,
        place: Place,
        wait: bool,
        redraw: Option<&Arc<Notify>>,
        gpu: Upload,
        now: u64,
    ) -> Option<u64> {
        for _ in 0..=MAX_IN_FLIGHT {
            if let Some(id) = self.request(layer, place, wait, redraw, gpu) {
                return Some(id);
            }
            if self.gone {
                return None;
            }
            // Ids grow and the thread answers in order: the smallest is the next to land (`land` swaps entries out).
            let oldest = self.pending.iter().map(|p| p.id).min()?;
            self.wait_for(oldest, gpu, now);
        }
        None
    }

    /// How many builds asked and not landed wait for this computer's fonts.
    pub(super) fn pending_waiting(&self) -> usize {
        self.pending.iter().filter(|p| p.wait).count()
    }

    /// The builds that landed, kept: how many.
    pub(super) fn collect(&mut self, gpu: Upload, now: u64) -> usize {
        let mut landed = 0;
        loop {
            match self.worker.as_ref().map(TextWorker::try_reply) {
                Some(Ok(Some(reply))) => {
                    self.land(reply, gpu, now);
                    landed += 1;
                }
                Some(Err(Gone)) => {
                    self.gone = true;
                    return landed;
                }
                Some(Ok(None)) | None => return landed,
            }
        }
    }

    /// Waits for build `id` (an export), keeping every build that lands meanwhile: its entry's index.
    pub(super) fn wait_for(&mut self, id: u64, gpu: Upload, now: u64) -> Option<usize> {
        loop {
            let Ok(reply) = self.worker.as_ref()?.reply() else {
                self.gone = true;
                return None;
            };
            let ours = reply.id == id;
            let at = self.land(reply, gpu, now);
            if ours {
                return at;
            }
        }
    }

    /// `reply` uploaded and kept, if it is still wanted: its entry's index (none for a survey).
    fn land(&mut self, reply: TextReply, (device, _, pipeline): Upload, now: u64) -> Option<usize> {
        let laid = match reply.answer {
            Answer::Text(laid) => laid,
            Answer::Survey(survey) => {
                let k = self.surveys_asked.iter().position(|&id| id == reply.id)?;
                self.surveys_asked.swap_remove(k);
                self.keep_survey(reply.id, survey, now);
                return None;
            }
        };
        let k = self.pending.iter().position(|p| p.id == reply.id)?;
        let Pending {
            id, whose, place, ..
        } = self.pending.swap_remove(k);
        let empty = || Built {
            layouts: TextLayouts::empty(f64::from_bits(place.k)),
            atlas: None,
            survey: None,
        };
        let Built {
            layouts,
            atlas,
            survey,
        } = laid.map(|b| *b).unwrap_or_else(empty);
        // Its atlas came uploaded: only its draws' and paints' buffers are made here.
        let room = (layouts.most, layouts.paints);
        let gpu = atlas.map(|atlas| TextGpu::new(device, pipeline, atlas, room));
        let bytes = gpu.as_ref().map_or(0, |g| g.bytes) + layouts.bytes();
        let entry = TextEntry {
            whose,
            place,
            id,
            layouts,
            survey,
            gpu,
            bytes,
            drawn: false,
            used: now,
        };
        Some(self.insert(entry, now))
    }
}

/// Test hooks: the text build thread lost (as one that failed to start), and each build made slow.
#[cfg(any(test, feature = "test-support"))]
impl super::SceneDrawer {
    pub fn lose_text_thread(&mut self) {
        self.texts.worker = Some(TextWorker::gone());
    }

    pub fn delay_text_builds(&mut self, delay: std::time::Duration) {
        self.texts.delay = delay;
    }
}
