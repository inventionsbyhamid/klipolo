//! The title build thread: a title the playback lookahead reaches is shaped, rasterised,
//! outlined and mipmapped here, about a second before the frame it enters, so the render thread only draws its
//! texture. Started by the drawer's first `ahead` (only the preview's lookahead asks; an export never starts it). It
//! keeps a builder and a font system of its own, as the caption build thread does (an exception to the one shared
//! font system), refreshed by the fonts' generation before each build. It answers requests in order; a dropped drawer
//! drops the request channel, and the thread ends after its current build.
//!
//! Queue order: a build's submissions return before its reply is sent, so the render thread's later submission, which
//! samples the texture, comes after them on the shared queue. The thread also waits for its build's GPU work before
//! replying: a title hands over a finished texture, never one still being drawn. It waits through
//! the device's poll thread (`finished`), which holds wgpu-core's fence lock while it waits, as this thread's
//! own `poll` would: the wait is moved there, not removed.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

use super::build::{Key, Layout, TitleBuilder};
use super::geometry::{Geometry, Try, Window};
use super::{Slot, shown};
use crate::render::build_times::{BuildTime, BuildTimes};
use crate::render::device::RenderDevice;
use crate::render::shared_fonts::SharedFonts;

/// The perf harness's title-build timing: off unless `time_builds` turns it on.
static TIMES: BuildTimes = BuildTimes::new();

/// Perf harness: time every title build from now on (each then waits for the GPU on the build thread), or stop.
#[doc(hidden)]
pub fn time_builds(on: bool) {
    TIMES.set(on);
}

/// Perf harness: the title builds timed since the last call.
#[doc(hidden)]
pub fn build_times() -> Vec<BuildTime> {
    TIMES.take()
}

/// A title to build: its layout's key at its first try, its geometry as the frame that asked has it (its sizes, outline
/// and where it is placed, for the window of a text far bigger than the frame), the room its texture may take and its
/// first try (the ladder a cold build climbs), and what the drawer keeps it under.
pub(super) struct Request {
    /// Its number (`Asked::ticket`), handed back with the build.
    pub(super) ticket: u64,
    pub(super) slot: Slot,
    pub(super) key: Key,
    pub(super) geometry: Geometry,
    pub(super) room: u64,
    pub(super) first: Try,
    pub(super) whole: (u32, (u32, u32)),
    /// The drawer's font epoch when asked: a build from before a font change is dropped.
    pub(super) epoch: u64,
}

/// A build that landed: its title, or None when it didn't fit its room even at its softest (the frame refuses it).
pub(super) struct Built {
    pub(super) ticket: u64,
    pub(super) slot: Slot,
    pub(super) whole: (u32, (u32, u32)),
    pub(super) epoch: u64,
    pub(super) title: Option<Title>,
}

/// A title built: the key and resolution it was laid out at, its layout and, unless none of it shows from where it was
/// asked, its texture and window.
pub(super) struct Title {
    pub(super) key: Key,
    pub(super) layout: Layout,
    pub(super) drawn: Option<(Window, wgpu::Texture)>,
    pub(super) mipmapped: bool,
    pub(super) down: f64,
}

/// The thread is gone (it panicked, or never started).
pub(super) struct Gone;

/// The render thread's end of the title build thread.
pub(super) struct Worker {
    requests: Sender<Request>,
    replies: Receiver<Built>,
}

impl Worker {
    pub(super) fn start(dev: &Arc<RenderDevice>) -> Self {
        let (requests, asked) = mpsc::channel();
        let (answer, replies) = mpsc::channel();
        let dev = dev.clone();
        let spawned = std::thread::Builder::new()
            .name("klipolo-titles".into())
            .spawn(move || run(&dev, &asked, &answer));
        if let Err(error) = spawned {
            // Both channel ends the thread held are gone: the drawer builds every title itself (`Gone`).
            tracing::warn!(%error, "title build thread");
        }
        Self { requests, replies }
    }

    pub(super) fn send(&self, request: Request) -> Result<(), Gone> {
        self.requests.send(request).map_err(|_| Gone)
    }

    /// A build if one has landed.
    pub(super) fn try_reply(&self) -> Result<Option<Built>, Gone> {
        match self.replies.try_recv() {
            Ok(built) => Ok(Some(built)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Gone),
        }
    }

    /// The next build, waited for up to `limit` (None when it didn't land in time).
    pub(super) fn reply_within(&self, limit: Duration) -> Result<Option<Built>, Gone> {
        match self.replies.recv_timeout(limit) {
            Ok(built) => Ok(Some(built)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(Gone),
        }
    }
}

fn run(dev: &RenderDevice, asked: &Receiver<Request>, answer: &Sender<Built>) {
    // Its builds are frames someone is about to watch.
    crate::qos::set_current(crate::qos::Qos::UserInitiated);
    let (device, queue) = (&dev.device, &dev.queue);
    let mut builder = TitleBuilder::new(device, queue);
    builder.warm(device);
    // The preview's fonts: it never waits for the system font scan (an export never starts this thread).
    let mut fonts = SharedFonts::new(false);
    while let Ok(r) = asked.recv() {
        if fonts.refresh() {
            builder.fonts_changed();
        }
        let started = TIMES.start();
        builder.begin(device, queue);
        let title = build(&mut builder, device, queue, &mut fonts, &r);
        // The perf harness's build timing first: its CPU ends at the submission.
        TIMES.finish(started, device);
        if builder.take_submission().is_some() {
            finished(dev);
        }
        let built = Built {
            ticket: r.ticket,
            slot: r.slot,
            whole: r.whole,
            epoch: r.epoch,
            title,
        };
        if answer.send(built).is_err() {
            return;
        }
    }
}

/// Waits (two seconds at most: a lost device) until the GPU work submitted so far is done: the device's one poll thread
/// waits, as it does for every frame's pixels, and tells this one (`on_submitted_work_done`). The fence-lock wait moved
/// there, it wasn't removed: that thread's `poll(Wait)` holds wgpu-core's fence lock until everything submitted is
/// done, so a frame the render thread submits meanwhile still waits on the CPU behind the build's GPU work (and behind
/// frames submitted after it, as its wait is for the newest submission), as it would behind a `poll` of this thread's
/// own. With wgpu 25 only not waiting would avoid it; a check run by hand on a Mac after a Mac build found no frame
/// over budget from it.
fn finished(dev: &RenderDevice) {
    let (done, wait) = mpsc::sync_channel(1);
    dev.queue.on_submitted_work_done(move || {
        let _ = done.try_send(());
    });
    dev.poll_soon();
    let _ = wait.recv_timeout(Duration::from_secs(2));
}

/// `r`'s title, down the ladder a cold build climbs (`Try`): laid out at each try's resolution until its
/// texture (cut to its ink) fits the room, then drawn; None when it fits at none.
fn build(
    builder: &mut TitleBuilder,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fonts: &mut SharedFonts,
    r: &Request,
) -> Option<Title> {
    let mut t = r.first;
    loop {
        let scale = r.geometry.up * t.down;
        let (font_px, box_px) = r.geometry.sizes(scale);
        let key = Key {
            font_px,
            box_px,
            ..r.key.clone()
        };
        // Its outline, and the margin its padded filters draw past it, at this size.
        let (stroke, margin) = (r.geometry.stroke * scale, r.geometry.margin * scale);
        let layout = builder.lay_out(fonts, &key, stroke as f32, margin as f32);
        let mipmapped = scale > 1.0;
        let Some(windows) = shown(&layout, &r.geometry.place(scale)) else {
            return Some(Title {
                key,
                layout,
                drawn: None,
                mipmapped,
                down: t.down,
            });
        };
        if windows.cut.bytes(mipmapped) > r.room {
            t = t.next(&r.geometry, r.room)?;
            continue;
        }
        let texture = builder.draw(device, queue, fonts, &key, &layout, windows.cut, mipmapped);
        return Some(Title {
            key,
            layout,
            drawn: Some((windows.cut, texture)),
            mipmapped,
            down: t.down,
        });
    }
}
