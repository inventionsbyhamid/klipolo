//! The camera bubble on macOS: a small round live picture of the take's camera on the screen it records,
//! so the person sees their framing and looks near the lens.
//! - **The picture** is a preview layer on the take's own camera session (`preview_layer`, made before it runs, then
//!   `BubbleFeed::set` from `camera_pipe`): no second open of the device, no frame copied, composited by the window
//!   server on the GPU. The camera is already open for the take, so the bubble opens nothing new. It is
//!   mirrored as the person's "Mirror my camera" says; the file never is.
//! - **Never in the video:** its panel's `sharingType` is None, and it is the worker's own window, which every display
//!   filter leaves out with the worker (`filter.rs`; a window or app take captures only its own windows). Its window
//!   number is kept for the filter's log (`window_id`).
//! - **Where:** top centre of the recorded display, just under the menu bar (under a MacBook's camera), else of the
//!   main screen. The person can drag it; AppKit keeps where it was left (`setFrameAutosaveName`), used again when it is
//!   on the recorded display. It takes clicks (it can be dragged), so nothing below is clicked through it.
//! - **When:** the session says (`StatusView::bubble`, `shell_view::bubble_shows`). It never joins a fullscreen app's
//!   Space (no `FullScreenAuxiliary`), so a fullscreen app never has it.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use cidre::{arc, av, ca};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen, NSStatusWindowLevel, NSView,
    NSWindowCollectionBehavior, NSWindowSharingType, NSWindowStyleMask,
};
use objc2_foundation::{NSNumber, NSPoint, NSRect, NSSize, NSString, ns_string};

use super::own_windows::{self, Own};
use crate::platform::common::camera::BubbleMirroring;

/// The bubble's diameter, in points.
const DIAMETER: f64 = 132.0;
/// Its gap under the menu bar.
const TOP_GAP: f64 = 10.0;
/// Where AppKit keeps where the person left it (`NSWindow Frame …` in Klipolo's defaults).
fn autosave() -> &'static NSString {
    ns_string!("klipolo.recorder.cameraBubble")
}

/// The bubble panel's window number, once made (0: none): `filter.rs` logs whether it is among what a display leaves
/// out.
static WINDOW: AtomicU32 = AtomicU32::new(0);

/// The bubble's window number, if this worker made one.
pub(super) fn window_id() -> Option<u32> {
    Some(WINDOW.load(Ordering::Acquire)).filter(|&id| id != 0)
}

/// A preview layer, handed from the camera's thread to the main thread.
struct Layer(arc::R<av::CaptureVideoPreviewLayer>);

// SAFETY: off the main thread the layer is only made (inside its own transaction), retained and released; it is put
// in a view and changed on the main thread only (`Bubble`).
unsafe impl Send for Layer {}

/// The live layer of the take's running camera session, for the shell's bubble: set as the camera starts, cleared as it
/// stops. Each new session's layer has a new number, so the bubble knows to take it.
#[derive(Default)]
pub(super) struct BubbleFeed {
    layer: Mutex<Option<(u64, Layer)>>,
    made: AtomicU64,
}

/// A preview layer on `session`, the take's camera's, made before it runs (`camera::CameraSession::open`): no device is
/// opened and no frame copied. Its picture is mirrored as `mirroring` says, set before the session runs;
/// only the layer's connection is, never the output the file is made from.
pub(super) fn preview_layer(
    session: &av::capture::Session,
    mirroring: BubbleMirroring,
) -> arc::R<av::CaptureVideoPreviewLayer> {
    ca::Transaction::perform_with_disabled_actions(|| {
        let mut layer = av::CaptureVideoPreviewLayer::with_session(session);
        layer.set_video_gravity(av::LayerVideoGravity::resize_aspect_fill());
        mirror(&layer, mirroring);
        layer
    })
}

/// Sets the layer's connection's mirroring, never left to AVFoundation's automatic rule. That goes off first:
/// AVFoundation raises an exception for `videoMirrored` while it is on, as it does for a connection that can't mirror,
/// which is left as it is (logged).
fn mirror(layer: &av::CaptureVideoPreviewLayer, mirroring: BubbleMirroring) {
    let Some(mut connection) = layer.connection().map(|c| c.retained()) else {
        tracing::warn!("bubble: the preview layer has no connection to mirror");
        return;
    };
    connection.set_automatically_adjusts_video_mirroring(mirroring.automatic);
    if !mirroring.automatic && connection.is_video_mirroring_supported() {
        connection.set_video_mirrored(mirroring.mirrored);
    } else {
        tracing::warn!("bubble: the camera's preview can't be mirrored");
    }
    tracing::info!(
        mirrored = connection.is_video_mirrored(),
        "bubble: its mirroring is set"
    );
}

impl BubbleFeed {
    /// The take's camera started with its bubble's layer (`preview_layer`): the bubble shows it from now.
    pub(super) fn set(&self, layer: arc::R<av::CaptureVideoPreviewLayer>) {
        let n = self.made.fetch_add(1, Ordering::AcqRel) + 1;
        *self.lock() = Some((n, Layer(layer)));
    }

    /// The camera stopped: its layer goes.
    pub(super) fn detach(&self) {
        self.lock().take();
    }

    fn current(&self) -> Option<(u64, arc::R<av::CaptureVideoPreviewLayer>)> {
        (self.lock().as_ref()).map(|(n, layer)| (*n, layer.0.retained()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(u64, Layer)>> {
        self.layer.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The bubble's panel, on the main thread.
pub(super) struct Bubble {
    mtm: MainThreadMarker,
    panel: Retained<NSPanel>,
    /// The feed's layer it shows, by number, and the layer itself.
    shown: Option<(u64, arc::R<av::CaptureVideoPreviewLayer>)>,
    /// The display the take records (`CGDirectDisplayID`).
    display: Option<u32>,
    /// It has been placed on its screen (later shows keep where the person left it).
    placed: bool,
}

impl Bubble {
    /// The panel, hidden, made at the shell's start (before any filter is built, so it has its window number then).
    pub(super) fn new(mtm: MainThreadMarker, display: Option<String>) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(DIAMETER, DIAMETER));
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: the panel is owned by this `Retained` and never closed, so AppKit must not release it on close.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setLevel(NSStatusWindowLevel);
        // Every ordinary Space, never a fullscreen app's (no `FullScreenAuxiliary`).
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setIgnoresMouseEvents(false);
        panel.setMovableByWindowBackground(true);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setHidesOnDeactivate(false);
        panel.setSharingType(NSWindowSharingType::None);
        let content = NSView::initWithFrame(mtm.alloc(), frame);
        content.setWantsLayer(true);
        if let Some(host) = host_layer(&content) {
            // SAFETY: the content view's own backing layer; CALayer's corner radius and clipping take these.
            unsafe {
                let _: () = msg_send![&*host, setCornerRadius: DIAMETER / 2.0];
                let _: () = msg_send![&*host, setMasksToBounds: true];
            }
        }
        panel.setContentView(Some(&content));
        let id = u32::try_from(panel.windowNumber()).unwrap_or(0);
        WINDOW.store(id, Ordering::Release);
        tracing::info!(
            window = id,
            sharing = panel.sharingType().0,
            "bubble: made, hidden until the take shows it"
        );
        Self {
            mtm,
            panel,
            shown: None,
            display: display.and_then(|id| id.parse().ok()),
            placed: false,
        }
    }

    /// Shows the bubble with the feed's layer (the newest session's), or hides it.
    pub(super) fn apply(&mut self, show: bool, feed: &BubbleFeed) {
        let layer = feed.current().filter(|_| show);
        let Some((n, layer)) = layer else {
            return self.hide();
        };
        if self.shown.as_ref().map(|(m, _)| *m) != Some(n) {
            self.take(n, layer);
        }
        if !self.placed {
            self.place();
        }
        // Where it is now (a drag shows here within the take's next status), before it shows: the screen's frames that
        // changed only its picture are idle ones (`own_windows`).
        own_windows::set(Own::Bubble, Some(self.panel.frame()));
        if !self.panel.isVisible() {
            self.panel.orderFrontRegardless();
            tracing::info!(
                window = window_id(),
                sharing = self.panel.sharingType().0,
                "bubble: shown"
            );
        }
    }

    pub(super) fn hide(&self) {
        if self.panel.isVisible() {
            self.panel.orderOut(None);
        }
        own_windows::set(Own::Bubble, None);
    }

    /// The panel shows layer `n` in place of the last one.
    fn take(&mut self, n: u64, layer: arc::R<av::CaptureVideoPreviewLayer>) {
        let Some(content) = self.panel.contentView() else {
            return;
        };
        let Some(host) = host_layer(&content) else {
            return;
        };
        let bounds = content.bounds();
        let new = std::ptr::from_ref::<av::CaptureVideoPreviewLayer>(&layer).cast::<AnyObject>();
        // SAFETY: both are CALayers alive for the call (`shown` keeps the old one until it is replaced below); the frame
        // is a CGRect, as NSRect is.
        unsafe {
            if let Some((_, old)) = &self.shown {
                let old =
                    std::ptr::from_ref::<av::CaptureVideoPreviewLayer>(old).cast::<AnyObject>();
                let _: () = msg_send![old, removeFromSuperlayer];
            }
            let _: () = msg_send![new, setFrame: bounds];
            let _: () = msg_send![&*host, addSublayer: new];
        }
        self.shown = Some((n, layer));
    }

    /// Where the person last left it, when that is on the recorded screen; else top centre under its menu bar. Only
    /// then does AppKit start keeping its frame (`setFrameAutosaveName` saves the frame it has at once, so set at
    /// creation it saved the unplaced corner, and the next take restored the bubble there: found by a
    /// device check).
    fn place(&mut self) {
        self.placed = true;
        let Some(screen) = self.screen() else {
            return;
        };
        let visible = screen.visibleFrame();
        let restored = self.panel.setFrameUsingName(autosave());
        let at = self.panel.frame().origin;
        let inside = at.x >= visible.origin.x
            && at.y >= visible.origin.y
            && at.x + DIAMETER <= visible.origin.x + visible.size.width
            && at.y + DIAMETER <= visible.origin.y + visible.size.height;
        if !(restored && inside) {
            let top_centre = NSPoint::new(
                visible.origin.x + (visible.size.width - DIAMETER) / 2.0,
                visible.origin.y + visible.size.height - TOP_GAP - DIAMETER,
            );
            self.panel.setFrameOrigin(top_centre);
        }
        self.panel.setFrameAutosaveName(autosave());
    }

    /// The recorded display's screen, by its `NSScreenNumber`, else the main screen.
    fn screen(&self) -> Option<Retained<NSScreen>> {
        let wanted = self.display;
        let recorded = NSScreen::screens(self.mtm).into_iter().find(|screen| {
            (screen.deviceDescription())
                .objectForKey(ns_string!("NSScreenNumber"))
                .and_then(|n| n.downcast_ref::<NSNumber>().map(NSNumber::unsignedIntValue))
                .is_some_and(|n| Some(n) == wanted)
        });
        recorded.or_else(|| NSScreen::mainScreen(self.mtm))
    }
}

/// A layer-backed view's own layer.
fn host_layer(view: &NSView) -> Option<Retained<AnyObject>> {
    // SAFETY: `-[NSView layer]` returns the view's backing CALayer (or nil), retained here.
    unsafe { msg_send![view, layer] }
}
