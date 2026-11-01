//! The AppKit side of the native presenter (macOS).
//! - `install` puts a layer-hosting `KlipoloPreviewView` under wry's WKWebView, inside the webview's parent view
//!   (`NSWindowBelow`), and makes the webview stop drawing its background (the private `drawsBackground` key; Klipolo
//!   ships as a Developer ID DMG, not on the App Store). The window itself stays opaque.
//! - The view hosts our own CAMetalLayer (BGRA8, sRGB, opaque, framebuffer-only, three drawables, display sync) and
//!   never takes a click: `hitTest:` returns nil.
//! - `set_rect` places it where the UI's transparent preview hole is (page points, sent once per animation frame),
//!   and notes the display's refresh interval for the presenter's scheduling. The page doesn't start at the webview's
//!   corner: Tauri's default title bar style makes the window's content full size under an opaque title bar, and
//!   WebKit insets the page by the title bar's height (`page_origin`). AppKit places the layer with the view;
//!   only its `contentsScale` (the window's backing scale) is ours to set. The drawable keeps the compositor's size,
//!   which Core Animation scales to the layer (`contentsGravity` resize).
//!
//! Everything here runs on the main thread, except `uninstall_soon`, `refresh_ns` and `in_transaction`, which any
//! thread may call. There is one preview window, so the installed view lives in a main-thread slot.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};

use objc2::encode::{Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSResponder, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSSize, NSString};

use crate::api::ViewRect;
pub use inspect::{Inspection, inspect};

mod inspect;

/// The refresh interval of the display the preview is on (60 Hz until the window says otherwise; ProMotion: 120 Hz).
static REFRESH_NS: AtomicU64 = AtomicU64::new(16_666_667);

thread_local! {
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Our CAMetalLayer (retained). The presenter thread draws into it; the main thread places it.
#[derive(Clone)]
pub struct NativeLayer {
    layer: metal::MetalLayer,
    /// In a window (made by `install`): its changes go through the main thread. A detached layer (tests) has no main
    /// run loop behind it, so it is changed where it is, inside a transaction.
    hosted: bool,
}

// SAFETY: CAMetalLayer's drawable calls are made for render threads; its geometry is only changed on the main thread
// (and its drawable size inside an explicit CATransaction on the presenter thread).
unsafe impl Send for NativeLayer {}
unsafe impl Sync for NativeLayer {}

/// A `CGColorSpaceRef`, encoded the way `-[CAMetalLayer setColorspace:]` declares it.
#[repr(C)]
struct CGColorSpace {
    _private: [u8; 0],
}

// SAFETY: an opaque CoreGraphics struct, passed only by pointer.
unsafe impl RefEncode for CGColorSpace {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGColorSpace", &[]));
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: *const c_void;
    fn CGColorSpaceCreateWithName(name: *const c_void) -> *mut CGColorSpace;
    fn CGColorSpaceRelease(space: *mut CGColorSpace);
}

unsafe extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(queue: *const u8, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn dispatch_sync_f(queue: *const u8, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

impl NativeLayer {
    fn new(hosted: bool) -> Self {
        let mut made = None;
        // Off the main thread (tests) the layer's setup is committed here rather than left to a run loop.
        in_transaction(|| {
            let layer = metal::MetalLayer::new();
            layer.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm);
            layer.set_framebuffer_only(true);
            layer.set_maximum_drawable_count(3);
            layer.set_display_sync_enabled(true);
            layer.set_presents_with_transaction(false);
            layer.set_opaque(true);
            let this = Self { layer, hosted };
            // SAFETY: property sets on a live CAMetalLayer; the layer retains the colour space.
            unsafe {
                // A drawable that doesn't come within a second returns nil instead of blocking for ever.
                let _: () = msg_send![this.object(), setAllowsNextDrawableTimeout: true];
                let srgb = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
                if !srgb.is_null() {
                    let _: () = msg_send![this.object(), setColorspace: srgb];
                    CGColorSpaceRelease(srgb);
                }
            }
            made = Some(this);
        });
        made.expect("made above")
    }

    pub(crate) fn layer(&self) -> &metal::MetalLayerRef {
        &self.layer
    }

    /// Runs a change to the layer on the main thread (`wait`: and waits for it), or right here when detached.
    fn change(&self, wait: bool, f: impl FnOnce(&Self) + Send + 'static) {
        let layer = self.clone();
        let run = move || in_transaction(|| f(&layer));
        match (self.hosted, wait) {
            (false, _) => run(),
            (true, true) => on_main_sync(run),
            (true, false) => on_main(run),
        }
    }

    /// The layer as an Objective-C object.
    fn object(&self) -> &AnyObject {
        // SAFETY: a CAMetalLayer is an Objective-C object, alive as long as `self`.
        unsafe { &*(self.layer() as *const metal::MetalLayerRef as *const AnyObject) }
    }

    /// Sets the drawable size (pixels) on the main thread, waiting for it, so the next drawable has it. Called by the
    /// presenter thread; the main thread never waits for that thread, so this can't deadlock.
    pub(crate) fn set_drawable_size(&self, (width, height): (u32, u32)) {
        let size = NSSize::new(f64::from(width), f64::from(height));
        // SAFETY: `drawableSize` is a CGSize property of CAMetalLayer.
        self.change(true, move |layer| unsafe {
            let _: () = msg_send![layer.object(), setDrawableSize: size];
        });
    }

    /// Draws from `device` (wgpu's MTLDevice). Waits for the main thread, so the first drawable comes from it: call it
    /// from the presenter thread only.
    pub(crate) fn set_device(&self, device: metal::Device) {
        self.change(true, move |layer| layer.layer().set_device(&device));
    }

    /// A layer in no window (tests of the presenter selection; nothing is drawn on screen).
    #[doc(hidden)]
    pub fn detached() -> Self {
        Self::new(false)
    }
}

define_class!(
    // SAFETY: NSView may be subclassed; `hitTest:` keeps its signature.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KlipoloPreviewView"]
    struct PreviewView;

    impl PreviewView {
        /// Never the target of a click: the webview above gets every event.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

impl PreviewView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

struct Installed {
    view: Retained<PreviewView>,
    webview: Retained<NSView>,
    layer: NativeLayer,
    rect: Option<ViewRect>,
    backdrop: Option<u32>,
    /// Notified when the window moves to another display (its refresh rate may differ even at the same scale).
    screen_observer: Option<Retained<AnyObject>>,
}

impl Drop for Installed {
    fn drop(&mut self) {
        if let Some(observer) = self.screen_observer.take() {
            // SAFETY: the observer token `addObserverForName:…` returned, removed once.
            unsafe {
                let center: Retained<AnyObject> =
                    msg_send![class("NSNotificationCenter"), defaultCenter];
                let _: () = msg_send![&*center, removeObserver: &*observer];
            }
        }
        self.view.removeFromSuperview();
    }
}

/// Watches `window` moving to another display: the presenter schedules by that display's refresh rate.
fn observe_screen(window: &objc2_app_kit::NSWindow) -> Option<Retained<AnyObject>> {
    note_refresh(window);
    let name = NSString::from_str("NSWindowDidChangeScreenNotification");
    let block = block2::RcBlock::new(|_note: std::ptr::NonNull<AnyObject>| {
        INSTALLED.with_borrow(|installed| {
            if let Some(window) = installed.as_ref().and_then(|i| i.view.window()) {
                note_refresh(&window);
            }
        });
    });
    // SAFETY: NSNotificationCenter's block API on the main queue; the token is kept and removed on drop.
    unsafe {
        let center: Retained<AnyObject> = msg_send![class("NSNotificationCenter"), defaultCenter];
        let queue: Retained<AnyObject> = msg_send![class("NSOperationQueue"), mainQueue];
        msg_send![&*center, addObserverForName: &*name, object: window, queue: &*queue, usingBlock: &*block]
    }
}

/// Puts the preview view under `webview` (a `WKWebView*`, from Tauri's `with_webview`) and returns its layer.
/// Installing again for the same webview returns the same layer. Main thread only.
pub fn install(webview: *mut c_void) -> Result<NativeLayer, String> {
    let mtm = MainThreadMarker::new().ok_or("The preview view must be made on the main thread.")?;
    INSTALLED.with_borrow_mut(|installed| {
        if let Some(current) = installed.as_ref()
            && Retained::as_ptr(&current.webview) as *mut c_void == webview
        {
            return Ok(current.layer.clone());
        }
        *installed = None;
        // SAFETY: Tauri hands us its live WKWebView (an NSView); retaining it keeps it valid while installed.
        let webview: Retained<NSView> =
            unsafe { Retained::retain(webview.cast()) }.ok_or("The window has no webview.")?;
        let parent = unsafe { webview.superview() }.ok_or("The webview has no parent view.")?;
        stop_drawing_background(&webview);
        let layer = NativeLayer::new(true);
        let view = PreviewView::new(mtm);
        // SAFETY: a layer-hosting view: the layer is set before `wantsLayer`, so AppKit never draws into it.
        unsafe {
            let _: () = msg_send![&*view, setLayer: layer.object()];
        }
        view.setWantsLayer(true);
        view.setHidden(true);
        // During a live resize AppKit moves it with the window until the UI sends the exact rect (1–2 frames).
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin
                | NSAutoresizingMaskOptions::ViewMaxXMargin
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin
                | NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        parent.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Below, Some(&webview));
        let screen_observer = webview.window().and_then(|w| observe_screen(&w));
        *installed = Some(Installed {
            view,
            webview,
            layer: layer.clone(),
            screen_observer,
            rect: None,
            backdrop: None,
        });
        Ok(layer)
    })
}

fn class(name: &str) -> &'static AnyClass {
    AnyClass::get(&std::ffi::CString::new(name).expect("class name")).expect("a Foundation class")
}

/// Whether the WKWebView paints its own (white) background, through its private `_drawsBackground` accessor; `None`
/// when this WebKit doesn't have it. Only asked after `respondsToSelector:`, so it can never raise.
pub(crate) fn draws_background(webview: &NSView) -> Option<bool> {
    // SAFETY: `respondsToSelector:` is public; `_drawsBackground` returns a BOOL and is only sent when it exists.
    unsafe {
        let has: bool = msg_send![webview, respondsToSelector: sel!(_drawsBackground)];
        has.then(|| msg_send![webview, _drawsBackground])
    }
}

/// Makes sure the WKWebView doesn't paint its white background, so the page is see-through over the hole. Tauri
/// already arranges this: the window's `backgroundColor` is passed to wry, which turns `drawsBackground` off in the
/// webview's configuration. This is the fallback if that ever changes, through the private `_setDrawsBackground:`
/// (Klipolo ships as a Developer ID DMG), sent only after `respondsToSelector:` (an unknown private call would raise).
fn stop_drawing_background(webview: &NSView) {
    if draws_background(webview) != Some(true) {
        return;
    }
    // SAFETY: as above; `_setDrawsBackground:` takes a BOOL.
    unsafe {
        let can: bool = msg_send![webview, respondsToSelector: sel!(_setDrawsBackground:)];
        if can {
            // Tauri no longer arranges it: worth knowing (the on-device trial checks this line never appears).
            tracing::warn!(
                "the webview drew its background; turned it off with the private setter"
            );
            let _: () = msg_send![webview, _setDrawsBackground: false];
        } else {
            tracing::warn!("the webview draws its background and can't be told not to");
        }
    }
}

/// Where the page's CSS (0, 0) is in the webview's own (flipped) coordinates. Tauri's default title bar style
/// (`Visible`) makes the window's content view full size under an opaque title bar, and WebKit then insets the page
/// by the title bar's height (its automatic content insets): the page starts 28 pt down the webview, and a hole
/// placed at its CSS numbers in the webview showed the picture 28 pt above it. The inset is the one
/// WebKit applies: the public `obscuredContentInsets` (WebKit of macOS 26 on) and the private `_topContentInset`
/// (earlier), each sent only after `respondsToSelector:`; where both answer, the larger top (they are the same inset).
/// It changes with the title bar (full screen): the page's size changes with it, and the UI sends the hole again.
pub(crate) fn page_origin(webview: &NSView) -> NSPoint {
    // SAFETY: `respondsToSelector:` is public; each getter is only sent when it exists, with its declared return type
    // (`NSEdgeInsets`; `CGFloat`, an f64 on every Mac Klipolo runs on).
    let (public, private) = unsafe {
        let has_public: bool = msg_send![webview, respondsToSelector: sel!(obscuredContentInsets)];
        let has_private: bool = msg_send![webview, respondsToSelector: sel!(_topContentInset)];
        let public: Option<NSEdgeInsets> =
            has_public.then(|| msg_send![webview, obscuredContentInsets]);
        let private: Option<f64> = has_private.then(|| msg_send![webview, _topContentInset]);
        (public, private)
    };
    let left = public.map_or(0.0, |i| i.left);
    let top = public.map_or(0.0, |i| i.top).max(private.unwrap_or(0.0));
    NSPoint::new(left.max(0.0), top.max(0.0))
}

/// `page_origin` of the WKWebView `webview` (Tauri's), as (x, y): for something native placed at a rect the page names
/// (the export's share menu beside its button). None without a webview. Main thread only.
pub fn page_origin_in(webview: *mut c_void) -> Option<(f64, f64)> {
    // SAFETY: Tauri hands us its live WKWebView (an NSView); retaining it keeps it valid while it is read.
    let webview: Retained<NSView> = unsafe { Retained::retain(webview.cast()) }?;
    let origin = page_origin(&webview);
    Some((origin.x, origin.y))
}

/// Places the preview view over the UI's preview hole (`None`, or an empty rect, hides it). The rect is in the page's
/// coordinates (CSS px at page zoom 1, top-left origin), which start at `page_origin` in the webview. False when called
/// off the main thread (nothing was done: call it again there).
pub fn set_rect(rect: Option<ViewRect>) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    INSTALLED.with_borrow_mut(|installed| {
        let Some(i) = installed.as_mut() else { return };
        let rect = rect.filter(|r| r.width > 0.0 && r.height > 0.0);
        // Compared with where the view actually is, not with the last rect: a live resize moves it on its own
        // (autoresizing), and the UI then sends the same rect again. The page's inset is read each time, so a rect
        // sent again after the title bar changed (full screen) moves the view with the page.
        let frame = rect.and_then(|r| {
            let parent = unsafe { i.view.superview() }?;
            let page = page_origin(&i.webview);
            let local = NSRect::new(
                NSPoint::new(page.x + r.x, page.y + r.y),
                NSSize::new(r.width, r.height),
            );
            Some(parent.convertRect_fromView(local, Some(&i.webview)))
        });
        let placed = frame.is_some_and(|f| f == i.view.frame()) && !i.view.isHidden();
        if i.rect == rect && (placed || rect.is_none()) {
            return;
        }
        in_transaction(|| match (rect, frame) {
            (Some(r), Some(frame)) => {
                // AppKit moves and sizes the hosted layer with the view, in the superview's coordinates. The layer's
                // own frame is never set: (0, 0) there is the content view's corner, not the hole (placed there, it showed
                // only the sliver of a cropped frame where the two overlapped).
                i.view.setFrame(frame);
                let window = i.view.window();
                let scale = window.as_ref().map_or(r.scale, |w| w.backingScaleFactor());
                i.layer.layer().set_contents_scale(scale);
                if let Some(window) = &window {
                    note_refresh(window);
                }
                i.view.setHidden(false);
            }
            _ => i.view.setHidden(true),
        });
        if let Some(backdrop) = rect.and_then(|r| r.backdrop)
            && i.backdrop != Some(backdrop)
            && let Some(window) = i.view.window()
        {
            // What shows through the hole for a frame or two while the window resizes: the stage colour.
            let channel = |shift: u32| f64::from((backdrop >> shift) & 0xff) / 255.0;
            let colour = NSColor::colorWithSRGBRed_green_blue_alpha(
                channel(16),
                channel(8),
                channel(0),
                1.0,
            );
            window.setBackgroundColor(Some(&colour));
            i.backdrop = Some(backdrop);
        }
        i.rect = rect;
    });
    true
}

/// Takes the preview view out of the window (the native presenter stopped). Main thread only.
pub fn uninstall() {
    if MainThreadMarker::new().is_some() {
        INSTALLED.with_borrow_mut(|installed| *installed = None);
    }
}

/// `uninstall` from any thread (it runs on the main thread soon after).
pub(crate) fn uninstall_soon() {
    extern "C" fn apply(_: *mut c_void) {
        uninstall();
    }
    main_async(std::ptr::null_mut(), apply);
}

/// The refresh interval of the preview's display (host ns), for scheduling presents.
pub(crate) fn refresh_ns() -> u64 {
    REFRESH_NS.load(Ordering::Relaxed)
}

/// Notes the refresh rate of the screen `window` is on (it changes when the window moves to another display).
fn note_refresh(window: &objc2_app_kit::NSWindow) {
    // SAFETY: `-[NSWindow screen]` and `-[NSScreen maximumFramesPerSecond]` (macOS 12+), on the main thread.
    let fps: isize = unsafe {
        let screen: Option<Retained<AnyObject>> = msg_send![window, screen];
        match screen {
            Some(screen) => msg_send![&*screen, maximumFramesPerSecond],
            None => 60,
        }
    };
    if (24..=240).contains(&fps) {
        REFRESH_NS.store(1_000_000_000 / fps as u64, Ordering::Relaxed);
    }
}

/// Runs `f` on the main thread soon (layer changes belong there).
pub(crate) fn on_main(f: impl FnOnce() + Send + 'static) {
    extern "C" fn run(context: *mut c_void) {
        // SAFETY: made from `Box::into_raw` below and run exactly once.
        let f = unsafe { Box::from_raw(context.cast::<Box<dyn FnOnce() + Send>>()) };
        f();
    }
    let f: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
    main_async(Box::into_raw(f).cast(), run);
}

/// Runs `f` on the main thread and waits for it (at once when already there).
fn on_main_sync(f: impl FnOnce() + Send) {
    if MainThreadMarker::new().is_some() {
        return f();
    }
    extern "C" fn run(context: *mut c_void) {
        // SAFETY: points at the `Option` below, alive for the whole synchronous call; taken once.
        let f = unsafe { &mut *context.cast::<Option<Box<dyn FnOnce() + Send + '_>>>() };
        if let Some(f) = f.take() {
            f();
        }
    }
    let mut f: Option<Box<dyn FnOnce() + Send + '_>> = Some(Box::new(f));
    // SAFETY: the main dispatch queue is a process-wide static; `dispatch_sync_f` returns after `run` did.
    unsafe { dispatch_sync_f(&raw const _dispatch_main_q, (&raw mut f).cast(), run) };
}

fn main_async(context: *mut c_void, work: extern "C" fn(*mut c_void)) {
    // SAFETY: the main dispatch queue is a process-wide static; `context` is only a flag.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, context, work) };
}

/// Runs layer changes inside an explicit `CATransaction` with implicit animations off. Off the main thread this
/// commits them at once instead of waiting for a run loop the thread doesn't have.
pub(crate) fn in_transaction(f: impl FnOnce()) {
    let ca = class("CATransaction");
    // SAFETY: CATransaction's class methods may be called on any thread.
    unsafe {
        let _: () = msg_send![ca, begin];
        let _: () = msg_send![ca, setDisableActions: true];
    }
    f();
    // SAFETY: as above.
    unsafe {
        let _: () = msg_send![ca, commit];
    }
}
