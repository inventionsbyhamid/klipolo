//! The caption build thread: every page's layout and atlas is built here, never on the
//! render thread. The thread makes the atlas builder and the draw pipeline (the pipeline goes back first), builds one
//! tiny page to warm them, and keeps a font system of its own (an exception to the shared font system), refreshed by
//! the fonts' generation before each build as the compositor's is. It answers build requests in order and fires the
//! redraw signal after each, so the preview draws a page once its atlas is there; an export waits for the reply on the
//! same path. A dropped drawer drops the request channel, and the thread ends after its current build.
//!
//! Queue order: a build's `queue.submit` returns before its reply is sent, so the render thread's later submit, which
//! draws from the atlas, comes after it on the shared queue.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use klipolo_model::captions::Pages;
use klipolo_model::captions::style::CaptionStyleDef;
use klipolo_model::project::FormatKind;
use tokio::sync::Notify;

use super::atlas::{AtlasBuilder, PageAtlas};
use super::cache::{Key, cjk_missing};
use super::draw::DrawPipe;
use super::layout::{PageLayout, lay_out_page};
use crate::render::build_times::{BuildTime, BuildTimes};
use crate::render::device::RenderDevice;
use crate::render::fonts::FaceChoice;
use crate::render::shared_fonts::SharedFonts;

/// The perf harness's page-build timing: off unless `time_builds` turns it on.
static TIMES: BuildTimes = BuildTimes::new();

/// Perf harness: time every page build from now on (each then waits for the GPU on the build thread, never on the
/// render thread), or stop.
#[doc(hidden)]
pub fn time_builds(on: bool) {
    TIMES.set(on);
}

/// Perf harness: the page builds timed since the last call.
#[doc(hidden)]
pub fn build_times() -> Vec<BuildTime> {
    TIMES.take()
}

/// A page to build, and what it is built from (everything shared or small: the style, pages and language are `Arc`s).
pub(super) struct Request {
    pub(super) key: Key,
    /// The drawer's font epoch when asked: a reply from before a font change is dropped.
    pub(super) epoch: u64,
    pub(super) style: Arc<CaptionStyleDef>,
    pub(super) face: FaceChoice,
    pub(super) language: Arc<str>,
    pub(super) pages: Arc<Pages>,
    pub(super) page: u32,
    pub(super) format: FormatKind,
    pub(super) out: (u32, u32),
    pub(super) max_width: f64,
}

/// A page built.
pub(super) struct Built {
    pub(super) key: Key,
    pub(super) epoch: u64,
    pub(super) layout: PageLayout,
    pub(super) atlas: PageAtlas,
    /// Its CJK asks for a Noto Sans that isn't downloaded (`frame.cjkFontMissing`).
    pub(super) cjk_missing: bool,
}

pub(super) enum Reply {
    /// The draw pipeline, always the first reply.
    Pipe(DrawPipe),
    Built(Box<Built>),
}

/// The thread is gone (it panicked, or never started).
pub(super) struct Gone;

/// The render thread's end of the build thread.
pub(super) struct Worker {
    requests: Sender<Request>,
    replies: Receiver<Reply>,
}

impl Worker {
    /// Starts the thread on `dev`. `wait` (an export): its font system waits for the system font scan, as the
    /// compositor's does. `redraw` fires after each reply.
    pub(super) fn start(dev: &Arc<RenderDevice>, redraw: Option<Arc<Notify>>, wait: bool) -> Self {
        let (requests, asked) = mpsc::channel();
        let (answer, replies) = mpsc::channel();
        let dev = dev.clone();
        let spawned = std::thread::Builder::new()
            .name("klipolo-captions".into())
            .spawn(move || run(&dev, redraw, wait, &asked, &answer));
        if let Err(error) = spawned {
            // Both channel ends the thread held are gone: the drawer says so (`Gone`).
            tracing::warn!(%error, "caption build thread");
        }
        Self { requests, replies }
    }

    pub(super) fn send(&self, request: Request) -> Result<(), Gone> {
        self.requests.send(request).map_err(|_| Gone)
    }

    /// A reply if one is there.
    pub(super) fn try_reply(&self) -> Result<Option<Reply>, Gone> {
        match self.replies.try_recv() {
            Ok(reply) => Ok(Some(reply)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Gone),
        }
    }

    /// The next reply, waited for.
    pub(super) fn reply(&self) -> Result<Reply, Gone> {
        self.replies.recv().map_err(|_| Gone)
    }
}

fn run(
    dev: &RenderDevice,
    redraw: Option<Arc<Notify>>,
    wait: bool,
    asked: &Receiver<Request>,
    answer: &Sender<Reply>,
) {
    // Its builds are work someone is waiting to see (the export caps it under its own ceiling).
    crate::qos::set_current(crate::qos::Qos::UserInitiated);
    let (device, queue) = (&dev.device, &dev.queue);
    let mut builder = AtlasBuilder::new(device, queue);
    if answer.send(Reply::Pipe(DrawPipe::new(device))).is_err() {
        return;
    }
    let mut fonts = SharedFonts::new(wait);
    builder.warm(device, queue, fonts.system());
    let notify = || {
        if let Some(redraw) = &redraw {
            redraw.notify_one();
        }
    };
    notify();
    while let Ok(r) = asked.recv() {
        if fonts.refresh() {
            builder.fonts_changed();
        }
        let started = TIMES.start();
        let layout = lay_out_page(
            fonts.system(),
            &r.style,
            &r.face,
            &r.language,
            &r.pages,
            r.page as usize,
            r.format,
            r.out,
            r.max_width,
        );
        let atlas = builder.build_atlas(device, queue, fonts.system(), &r.style, &layout);
        TIMES.finish(started, device);
        let built = Built {
            key: r.key,
            epoch: r.epoch,
            cjk_missing: cjk_missing(&layout),
            layout,
            atlas,
        };
        if answer.send(Reply::Built(Box::new(built))).is_err() {
            return;
        }
        notify();
    }
}
