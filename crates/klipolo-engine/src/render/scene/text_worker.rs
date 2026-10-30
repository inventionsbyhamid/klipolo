//! The scene text build thread: every instance's
//! text is laid out and its atlas drawn here, never on the render thread. It keeps a font system of its own (the
//! exception to the shared font system, as the caption thread's), made on its first build and refreshed by the fonts'
//! generation before each one; an export's build (`wait`) first waits for this computer's fonts here, on this thread
//!, and the preview's never does. It builds its own copy of the instance from the compiled scene and the layer's
//! settings (cheap beside the shaping and the raster), uploads its atlas into textures of its own (the
//! queue orders the writes before the render thread's next frame, as the caption thread's), and answers in order,
//! firing the redraw signal after each so the preview draws the text once it lands. The render thread only makes the
//! draws' buffers (`text_builds.rs`). A moving scene without text asks it for its survey alone, so its
//! first frame packs only itself. A dropped drawer drops the request channel, and the thread ends after its current
//! build.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use klipolo_model::params::ParamValue;
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::timeline::CompiledScene;
use tokio::sync::Notify;

use super::Drawn;
use super::sizes::Sizes;
use super::sizes_built::survey_built;
use super::statics::statics;
use super::text_counts::BUILD_THREAD;
use super::text_gpu::{AtlasTextures, upload_atlas};
use super::text_layouts::{TextLayouts, lay_out_all};
use crate::render::shared_fonts::SharedFonts;

/// One instance's text to build: the entry it is for (`id`), what the instance is made from, where it is drawn, the
/// em's largest size, and whether its fonts wait for this computer's (an export's). With `survey`, only its survey
/// (a scene without text).
pub(super) struct TextRequest {
    pub id: u64,
    pub wait: bool,
    pub survey: bool,
    pub scene: Arc<CompiledScene>,
    pub params: Arc<BTreeMap<String, ParamValue>>,
    pub fills_animate: bool,
    pub images: Arc<[(Arc<str>, Arc<str>)]>,
    pub language: Arc<str>,
    pub drawn: Drawn,
    pub max_px: f32,
    /// Tests: the build waits this long first (a slow build, `SceneDrawer::delay_text_builds`).
    #[cfg(any(test, feature = "test-support"))]
    pub delay: std::time::Duration,
}

/// What a build made: its text (None: the instance has no text), or a survey alone (None: it couldn't be made here).
pub(super) struct TextReply {
    pub id: u64,
    pub answer: Answer,
}

/// A reply's answer (the text boxed: it is the larger by far).
pub(super) enum Answer {
    Text(Option<Box<Built>>),
    Survey(Option<Sizes>),
}

/// A text built: its layouts, its atlas's textures (None: refused for the budget), and its instance's survey where the
/// build thread could make it (`sizes_built.rs`).
pub(super) struct Built {
    pub layouts: TextLayouts,
    pub atlas: Option<AtlasTextures>,
    pub survey: Option<Sizes>,
}

/// The thread is gone (it panicked, or never started).
pub(super) struct Gone;

/// The render thread's end of the build thread.
pub(super) struct TextWorker {
    requests: Sender<TextRequest>,
    replies: Receiver<TextReply>,
}

impl TextWorker {
    /// Starts the thread, uploading on `gpu`. `redraw` fires after each reply.
    pub(super) fn start(redraw: Option<Arc<Notify>>, gpu: (wgpu::Device, wgpu::Queue)) -> Self {
        let (requests, asked) = mpsc::channel::<TextRequest>();
        let (answer, replies) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(BUILD_THREAD.into())
            .spawn(move || run(redraw, &gpu, &asked, &answer));
        if let Err(error) = spawned {
            // Both channel ends the thread held are gone: the cache sees `Gone`.
            tracing::warn!(%error, "scene text build thread");
        }
        Self { requests, replies }
    }

    /// Tests: a thread that is gone (it never started), as `start` leaves one whose spawn failed.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn gone() -> Self {
        let (requests, _) = mpsc::channel();
        let (_, replies) = mpsc::channel();
        Self { requests, replies }
    }

    pub(super) fn send(&self, request: TextRequest) -> Result<(), Gone> {
        self.requests.send(request).map_err(|_| Gone)
    }

    /// A reply if one is there.
    pub(super) fn try_reply(&self) -> Result<Option<TextReply>, Gone> {
        match self.replies.try_recv() {
            Ok(reply) => Ok(Some(reply)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Gone),
        }
    }

    /// The next reply, waited for (an export).
    pub(super) fn reply(&self) -> Result<TextReply, Gone> {
        self.replies.recv().map_err(|_| Gone)
    }
}

fn run(
    redraw: Option<Arc<Notify>>,
    (device, queue): &(wgpu::Device, wgpu::Queue),
    asked: &Receiver<TextRequest>,
    answer: &Sender<TextReply>,
) {
    // Its builds are work someone is waiting to see.
    crate::qos::set_current(crate::qos::Qos::UserInitiated);
    let mut slot: Option<SharedFonts> = None;
    while let Ok(r) = asked.recv() {
        #[cfg(any(test, feature = "test-support"))]
        std::thread::sleep(r.delay);
        let mut inst = SceneInstance::new(&r.scene, &r.params);
        inst.set_fills_animate(r.fills_animate);
        let (statics, _, spans) = statics(&inst, &r.images);
        let parts = (&statics[..], &spans[..]);
        let reply = if r.survey {
            // A survey alone needs no fonts.
            Answer::Survey(survey_built(&inst, parts, r.drawn, None, 0))
        } else {
            // Made on the first build; made again, waiting, for an export's build before this computer's fonts are in.
            if slot.as_ref().is_none_or(|f| r.wait && !f.scanned()) {
                slot = Some(SharedFonts::new(r.wait));
            }
            let Some(fonts) = slot.as_mut() else {
                continue;
            };
            fonts.refresh();
            let side = device.limits().max_texture_dimension_2d;
            let laid = lay_out_all(
                fonts,
                &r.language,
                &mut inst,
                &statics,
                (r.drawn, r.max_px),
                side,
            );
            // Its atlas uploaded here, its pixels let go here: the render thread gets the textures, and the survey made
            // here when it can be.
            Answer::Text(laid.map(|l| {
                let atlas = l.image.map(|image| upload_atlas(device, queue, &image));
                let survey = survey_built(&inst, parts, r.drawn, Some(&l.layouts), r.id);
                Box::new(Built {
                    layouts: l.layouts,
                    atlas,
                    survey,
                })
            }))
        };
        let reply = TextReply {
            id: r.id,
            answer: reply,
        };
        if answer.send(reply).is_err() {
            return;
        }
        if let Some(redraw) = &redraw {
            redraw.notify_one();
        }
    }
}
