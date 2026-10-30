//! The presenter thread (macOS). It owns every `nextDrawable` call: drawables are acquired from our own
//! CAMetalLayer with the metal crate, outside wgpu-core's locks (wgpu's own surface holds the device's fence lock
//! across `nextDrawable`, which would stall the compositor's submits), and with the layer's one-second timeout left
//! on, so a hidden or resizing window can never hang it.
//!
//! Pacing: each playback frame is committed a little ahead (at most `HORIZON` refreshes) and presented
//! with `presentDrawable:atTime:` on the refresh nearest its due time, the moment its sound is heard, with the cadence
//! held steady (`pick::cadence`). Stills, and frames that came late, show at once. Nothing waits for a display-link
//! tick on the main thread, so a busy main thread can't make a frame miss its refresh, and nothing wakes 60 times a
//! second. (`presentedTime` isn't used to place the refresh grid: for a layer in a window that WindowServer composites,
//! it scatters by several ms around the refresh.)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use objc2::msg_send;
use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;

use super::{Msg, PresentEntry, PresentLog, ShownSink, Slots, slots::Taken};
use crate::frames::trailer::{FLAG_PLAYBACK, now_ms};
use crate::present::macos::{self, NativeLayer};
use crate::present::pick::{cadence, host_ns};
use crate::render::device::RenderDevice;

/// The drawable format: 8-bit BGRA holding the compositor's sRGB-encoded values as they are (the layer's colour space
/// is sRGB, like the WebGL canvas the socket path draws into).
pub(super) const DRAWABLE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
/// Frames are committed at most this many refreshes before they are due: with 30 fps frames on a 60 Hz display, one
/// frame waits in a drawable at a time (the layer has three: on screen, waiting, being drawn).
const HORIZON: f64 = 1.5;
/// While playing steadily, every this many presents is reported to the UI (about 4 a second at 30 fps): its clock
/// runs on its own between reports, so the playhead needs no message per frame.
pub(super) const SHOWN_EVERY: u32 = 8;
/// After a drawable timed out, try again this soon.
const RETRY: Duration = Duration::from_millis(100);

pub(super) struct Parts {
    pub dev: Arc<RenderDevice>,
    /// wgpu's MTLDevice: the layer's drawables must come from it (set on the main thread when the thread starts).
    pub device: metal::Device,
    pub layer: NativeLayer,
    pub pipeline: wgpu::RenderPipeline,
    pub slots: Arc<Slots>,
    pub failed: Arc<AtomicBool>,
    pub log: Arc<PresentLog>,
    pub shown: ShownSink,
}

struct Presenter {
    p: Parts,
    queue: metal::CommandQueue,
    drawable_size: (u32, u32),
    visible: bool,
    /// What is on screen, kept to draw again (after a drawable timed out).
    on_screen: Option<Taken>,
    /// The layer shows one cleared pixel (`release`) and its frame-sized drawables are gone.
    released: bool,
    retry: bool,
    /// The previous playback frame's refresh and due time in this run of playback (the cadence lock).
    cadence: Option<(u64, u64)>,
    /// Presents since the UI was last told (`SHOWN_EVERY`).
    unreported: u32,
}

/// Starts the thread; its sender takes frames and visibility changes.
pub(super) fn spawn(p: Parts) -> Result<mpsc::Sender<Msg>, String> {
    // SAFETY: only reads wgpu's Metal command queue handle (retained); wgpu keeps using it as before. Command buffers
    // from several threads on one MTLCommandQueue are allowed.
    let queue = unsafe {
        p.dev
            .queue
            .as_hal::<wgpu_hal::api::Metal, _, _>(|q| q.map(|q| q.as_raw().lock().clone()))
    }
    .ok_or("The graphics card isn't a Metal device.")?;
    let (tx, rx) = mpsc::channel();
    let presenter = Presenter {
        p,
        queue,
        drawable_size: (0, 0),
        visible: true,
        on_screen: None,
        released: false,
        retry: false,
        cadence: None,
        unreported: 0,
    };
    std::thread::Builder::new()
        .name("klipolo-present".into())
        .spawn(move || {
            crate::qos::set_current(crate::qos::Qos::UserInteractive);
            presenter.run(rx);
        })
        .map_err(|e| format!("The preview couldn't start: {e}"))?;
    Ok(tx)
}

impl Presenter {
    fn run(mut self, rx: mpsc::Receiver<Msg>) {
        // Here rather than on the actor: this waits for the main thread, which never waits for this thread (the
        // actor could be waited on by the main thread at quit).
        self.p.layer.set_device(self.p.device.clone());
        loop {
            let msg = match self.wait() {
                Some(timeout) => match rx.recv_timeout(timeout) {
                    Ok(msg) => Some(msg),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return,
                },
            };
            // Everything already waiting is applied first.
            let mut next = msg;
            loop {
                match next {
                    Some(Msg::Stop) => return,
                    Some(Msg::Visible(visible)) => {
                        self.visible = visible;
                        if !visible {
                            self.release();
                        }
                    }
                    // Hidden or minimised, or no project open: nothing is drawn, and the slot it showed and the
                    // layer's drawables go (the next frame sizes them again).
                    Some(Msg::Release) => self.release(),
                    Some(Msg::Queued) | None => {}
                }
                match rx.try_recv() {
                    Ok(msg) => next = Some(msg),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            self.step();
        }
    }

    /// Lets the drawables' memory go (hidden, minimised, or no project open): the layer's pool shrinks to one pixel,
    /// and one cleared pixel replaces the last frame on it, so that frame's surface goes too. (Setting the layer's
    /// `contents` to nil instead is undefined for a CAMetalLayer, and Core Animation logs it.) The next frame sizes the
    /// pool again.
    fn release(&mut self) {
        self.on_screen = None;
        self.retry = false;
        self.cadence = None;
        if self.released {
            return;
        }
        self.released = true;
        self.p.layer.set_drawable_size((1, 1));
        self.drawable_size = (1, 1);
        autoreleasepool(|_| {
            // A fresh pool at the new size: this doesn't wait for the frames still on screen.
            let Some(drawable) = self.p.layer.next_drawable() else {
                return;
            };
            let pass = metal::RenderPassDescriptor::new();
            if let Some(colour) = pass.color_attachments().object_at(0) {
                colour.set_texture(Some(drawable.texture()));
                colour.set_load_action(metal::MTLLoadAction::Clear);
                colour.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 1.0));
                colour.set_store_action(metal::MTLStoreAction::Store);
            }
            let buffer = self.queue.new_command_buffer();
            buffer.new_render_command_encoder(pass).end_encoding();
            buffer.present_drawable(drawable);
            buffer.commit();
        });
    }

    fn refresh_ns(&self) -> u64 {
        macos::refresh_ns()
    }

    /// How long to wait for the next message: until the next waiting frame should be committed.
    fn wait(&self) -> Option<Duration> {
        if !self.visible || self.p.failed.load(Ordering::Relaxed) {
            return None;
        }
        if self.retry {
            return Some(RETRY);
        }
        let due = self.p.slots.next_due()?;
        let commit_at = due.saturating_sub((self.refresh_ns() as f64 * HORIZON) as u64);
        Some(Duration::from_nanos(
            commit_at.saturating_sub(host_ns()).max(500_000),
        ))
    }

    /// Commits every frame that is due, or due within the horizon (each scheduled for its own refresh).
    fn step(&mut self) {
        if !self.visible || self.p.failed.load(Ordering::Relaxed) {
            return;
        }
        let refresh = self.refresh_ns();
        let horizon = (refresh as f64 * HORIZON) as u64;
        let mut committed = false;
        while let Some((taken, at)) = self.p.slots.take_next(host_ns(), refresh, horizon) {
            committed = true;
            // A scheduled playback frame goes on its refresh with the cadence held; anything shown at once restarts it.
            // The first frame of a run (and anything shown at once) is reported to the UI straight away.
            let report = at.is_none() || self.cadence.is_none();
            let at = at.map(|_| {
                let on = cadence(taken.due_ns, self.cadence, refresh);
                self.cadence = Some((on, taken.due_ns));
                // Half a refresh early: Core Animation shows it on the first refresh after, which is `on`.
                on.saturating_sub(refresh / 2)
            });
            if at.is_none() {
                self.cadence = None;
            }
            let ok = self.show(&taken, at, report);
            self.retry = !ok;
            self.on_screen = Some(taken);
            if !ok {
                break;
            }
        }
        if !committed
            && self.retry
            && let Some(taken) = self.on_screen.clone()
        {
            self.retry = !self.show(&taken, None, true);
        }
    }

    /// Copies `taken` into the next drawable and presents it after the copy, at host time `at` (`None`: at once), and
    /// tells the UI if `report` (or it is a still, or `SHOWN_EVERY` presents went unreported). False when no drawable
    /// came.
    fn show(&mut self, taken: &Taken, at: Option<u64>, report: bool) -> bool {
        self.released = false;
        autoreleasepool(|_| {
            if self.drawable_size != taken.size {
                self.p.layer.set_drawable_size(taken.size);
                self.drawable_size = taken.size;
            }
            let Some(drawable) = self.p.layer.next_drawable() else {
                self.p.log.timeouts.fetch_add(1, Ordering::Relaxed);
                return false;
            };
            let texture = drawable.texture().to_owned();
            let size = (texture.width() as u32, texture.height() as u32);
            let device = &self.p.dev.device;
            // SAFETY: the drawable's texture is a live 2D BGRA8 render target on wgpu's own Metal device; wgpu holds
            // a reference to it until this frame's work is done.
            let target = unsafe {
                let hal = wgpu_hal::metal::Device::texture_from_raw(
                    texture,
                    DRAWABLE_FORMAT,
                    metal::MTLTextureType::D2,
                    1,
                    1,
                    wgpu_hal::CopyExtent {
                        width: size.0,
                        height: size.1,
                        depth: 1,
                    },
                );
                device.create_texture_from_hal::<wgpu_hal::api::Metal>(
                    hal,
                    &wgpu::TextureDescriptor {
                        label: Some("Klipolo drawable"),
                        size: wgpu::Extent3d {
                            width: size.0,
                            height: size.1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: DRAWABLE_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    },
                )
            };
            let view = target.create_view(&Default::default());
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Klipolo native present"),
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Klipolo native present"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.p.pipeline);
                pass.set_bind_group(0, &taken.bind, &[]);
                pass.draw(0..3, 0..1);
            }
            self.p.dev.queue.submit([encoder.finish()]);
            // The present goes on the same Metal queue after the copy (as wgpu's own surfaces do).
            let present = self.queue.new_command_buffer();
            let meta = &taken.meta;
            let now = host_ns();
            // When it should be on the glass (ms since the epoch): the UI's playhead and latency readout use it.
            let shown_ms = now_ms() + at.map_or(0.0, |at| at.saturating_sub(now) as f64 / 1e6);
            let index = if self.p.log.enabled() {
                let entry = PresentEntry {
                    at_us: meta.time_us,
                    seq: meta.seq,
                    playback: meta.flags & FLAG_PLAYBACK != 0,
                    due_ns: taken.due_ns,
                    scheduled_ns: at.unwrap_or(0),
                    committed_ns: now,
                    presented_ns: 0,
                    sent_ms: meta.sent_ms,
                    shown_ms,
                };
                self.p.log.push(entry)
            } else {
                None
            };
            // The trial log records when Core Animation reports each frame on screen.
            if let Some(index) = index {
                let log = self.p.log.clone();
                let handler = block::ConcreteBlock::new(move |d: &metal::DrawableRef| {
                    let on = d.presented_time();
                    if on > 0.0 {
                        log.presented(index, (on * 1e9) as u64);
                    }
                })
                .copy();
                drawable.add_presented_handler(&handler);
            }
            match at {
                // SAFETY: `presentDrawable:atTime:` (CFTimeInterval host seconds) on a live command buffer, with the
                // drawable it was given.
                Some(at) => unsafe {
                    let buffer = &*(present as *const metal::CommandBufferRef as *const AnyObject);
                    let drawable =
                        &*(drawable as *const metal::MetalDrawableRef as *const AnyObject);
                    let _: () =
                        msg_send![buffer, presentDrawable: drawable, atTime: at as f64 / 1e9];
                },
                None => present.present_drawable(drawable),
            }
            present.commit();
            self.unreported += 1;
            if report || meta.flags & FLAG_PLAYBACK == 0 || self.unreported >= SHOWN_EVERY {
                (self.p.shown)(meta, shown_ms, self.unreported);
                self.unreported = 0;
            }
            true
        })
    }
}

impl NativeLayer {
    fn next_drawable(&self) -> Option<&metal::MetalDrawableRef> {
        self.layer().next_drawable()
    }
}
