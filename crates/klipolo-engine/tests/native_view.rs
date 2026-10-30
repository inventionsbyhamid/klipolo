//! Where the native preview lands in a window (macOS, AppKit). The layer the presenter draws into must sit exactly
//! over the UI's hole, at the hole's size and the window's scale, and stay there through a window resize and a
//! display-scale change. A view can sit over the hole with its layer moved to the content view's corner, so only
//! a sliver of a cropped frame shows through the hole.
//!
//! The picture can also land 28 pt above its hole. Tauri's default title bar style (`Visible`) makes the window's
//! content view full size under an opaque title bar, and WebKit then insets the page by the title bar's height, so the
//! page's CSS (0, 0) is not the WKWebView's corner. The second part checks this with a real WKWebView in such a window:
//! a click in the middle of the preview must reach the page in the middle of the hole.
//!
//! AppKit wants the main thread, so this runs without the test harness (`harness = false`), in off-screen windows
//! that are never shown. Nothing is drawn or captured; the click is a synthetic event sent straight to the webview.
//! Skipped when there is no display.

#[cfg(target_os = "macos")]
fn main() {
    mac::run();
    mac::run_webkit();
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::rc::Rc;

    use block2::RcBlock;

    use klipolo_engine::api::ViewRect;
    use klipolo_engine::present::macos::{self, Inspection};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{MainThreadMarker, MainThreadOnly, class, define_class, msg_send};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBackingStoreType, NSResponder, NSView, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    define_class!(
        // SAFETY: NSView may be subclassed; `isFlipped` keeps its signature.
        #[unsafe(super(NSView, NSResponder, NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "KlipoloTestWebView"]
        // Stands in for the WKWebView: flipped (top-left origin, like the page), filling the content view.
        struct WebView;

        impl WebView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }
        }
    );

    struct Window {
        window: Retained<NSWindow>,
        parent: Retained<NSView>,
        webview: Retained<WebView>,
    }

    impl Window {
        /// A window like Klipolo's (content view → [webview]), never ordered on screen.
        fn new(mtm: MainThreadMarker) -> Self {
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0));
            // SAFETY: NSWindow's designated initialiser; it is never closed, and not released when closed.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    // Tauri's default (`TitleBarStyle::Visible`): full size under an opaque title bar.
                    NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Resizable
                        | NSWindowStyleMask::FullSizeContentView,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            let parent = NSView::initWithFrame(NSView::alloc(mtm), frame);
            parent.setWantsLayer(true);
            window.setContentView(Some(&parent));
            let this = WebView::alloc(mtm).set_ivars(());
            // SAFETY: NSView's designated initialiser.
            let webview: Retained<WebView> =
                unsafe { msg_send![super(this), initWithFrame: frame] };
            webview.setWantsLayer(true);
            webview.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            parent.addSubview(&webview);
            let this = Self {
                window,
                parent,
                webview,
            };
            this.settle();
            this
        }

        /// Lets AppKit and Core Animation catch up: a display pass, a committed transaction, a turn of the run loop.
        fn settle(&self) {
            self.window.displayIfNeeded();
            // SAFETY: class methods of CATransaction, NSDate and NSRunLoop on the main thread.
            unsafe {
                let _: () = msg_send![class!(CATransaction), flush];
                let until: Retained<AnyObject> =
                    msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: 0.05f64];
                let run_loop: Retained<AnyObject> = msg_send![class!(NSRunLoop), currentRunLoop];
                let _: () = msg_send![&*run_loop, runUntilDate: &*until];
            }
        }

        fn scale(&self) -> f64 {
            self.window.backingScaleFactor()
        }

        /// The UI sends the hole (CSS px in the webview, top-left origin) and its devicePixelRatio.
        fn send(&self, hole: Option<[f64; 4]>, scale: f64) {
            let rect = hole.map(|[x, y, width, height]| ViewRect {
                x,
                y,
                width,
                height,
                scale,
                backdrop: None,
            });
            assert!(macos::set_rect(rect), "set_rect runs on the main thread");
            self.settle();
        }

        /// What the presenter does with a 1280×718 frame: the drawable takes the compositor's size.
        fn draw_at(&self, (width, height): (f64, f64)) {
            let subviews = self.parent.subviews();
            // SAFETY: the preview view is the content view's first subview (below the webview); its layer is our
            // CAMetalLayer.
            unsafe {
                let view: *mut NSView = msg_send![&*subviews, objectAtIndex: 0usize];
                let layer: *mut AnyObject = msg_send![view, layer];
                let _: () = msg_send![layer, setDrawableSize: NSSize::new(width, height)];
            }
        }
    }

    fn placed(step: &str, hole: [f64; 4], scale: f64) -> Inspection {
        let i = macos::inspect().expect("the preview is installed");
        assert!(i.problems.is_empty(), "{step}: {:?}\n{i:#?}", i.problems);
        assert_eq!(i.layer_frame, hole, "{step}: the layer is over the hole");
        assert_eq!(i.frame, hole, "{step}: the view is over the hole");
        assert_eq!(i.contents_scale, scale, "{step}: the window's scale");
        assert!(
            i.below_webview && !i.hidden && i.hit_test_nil,
            "{step}: {i:#?}"
        );
        i
    }

    pub fn run() {
        let mtm = MainThreadMarker::new().expect("harness = false runs on the main thread");
        // SAFETY: NSApplication/NSScreen class methods on the main thread. Prohibited: no Dock icon, no menu bar.
        let screen: Option<Retained<AnyObject>> = unsafe {
            let app: Retained<AnyObject> = msg_send![class!(NSApplication), sharedApplication];
            let _: bool = msg_send![&*app, setActivationPolicy: 2isize];
            msg_send![class!(NSScreen), mainScreen]
        };
        if screen.is_none() {
            eprintln!("native_view: no display, skipped");
            return;
        }
        let w = Window::new(mtm);
        let scale = w.scale();
        macos::install(Retained::as_ptr(&w.webview) as *mut c_void).expect("installed");
        w.settle();

        // A 717×403 pt hole at (370, 149.5) on a Retina display, a 1280×718 frame.
        let hole = [370.0, 149.5, 717.0, 403.0];
        w.send(Some(hole), scale);
        w.draw_at((1280.0, 718.0));
        let i = placed("first rect", hole, scale);
        assert_eq!(i.layer_size, [717.0, 403.0]);

        // A live resize: AppKit's autoresizing moves the view first, then the UI sends the hole's new place, and once
        // the resize ends, the same rect again.
        w.window.setContentSize(NSSize::new(1600.0, 1000.0));
        w.settle();
        let hole = [420.0, 160.0, 796.0, 448.0];
        w.send(Some(hole), scale);
        placed("after a resize", hole, scale);
        w.send(Some(hole), scale);
        placed("the same rect again", hole, scale);

        // The window moved to a display of another scale: the UI's devicePixelRatio changed, the layer takes the
        // window's backing scale.
        w.send(Some(hole), if scale == 2.0 { 1.0 } else { 2.0 });
        placed("a display change", hole, scale);

        // Hidden (no hole), then back.
        w.send(None, scale);
        let i = macos::inspect().expect("still installed");
        assert!(i.hidden && i.problems.is_empty(), "{i:#?}");
        w.send(Some(hole), scale);
        placed("shown again", hole, scale);

        macos::uninstall();
        println!("native_view: the layer sits over the hole (scale {scale}): ok");
    }

    #[link(name = "WebKit", kind = "framework")]
    unsafe extern "C" {}

    /// Runs the main run loop for `seconds`.
    fn spin(seconds: f64) {
        // SAFETY: NSDate and NSRunLoop class methods on the main thread.
        unsafe {
            let until: Retained<AnyObject> =
                msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: seconds];
            let run_loop: Retained<AnyObject> = msg_send![class!(NSRunLoop), currentRunLoop];
            let _: () = msg_send![&*run_loop, runUntilDate: &*until];
        }
    }

    /// Evaluates `js` in `web` and waits (up to 10 s) for its result as a string.
    fn eval(web: &NSView, js: &str) -> String {
        let out: Rc<RefCell<Option<String>>> = Rc::default();
        let sink = out.clone();
        let done = RcBlock::new(move |result: *mut AnyObject, error: *mut AnyObject| {
            let text = if result.is_null() {
                format!("error: {error:?}")
            } else {
                // SAFETY: the scripts below all return strings.
                unsafe { &*(result as *const NSString) }.to_string()
            };
            *sink.borrow_mut() = Some(text);
        });
        // SAFETY: -[WKWebView evaluateJavaScript:completionHandler:] on the main thread.
        unsafe {
            let _: () = msg_send![web, evaluateJavaScript: &*NSString::from_str(js), completionHandler: &*done];
        }
        for _ in 0..200 {
            if let Some(text) = out.borrow_mut().take() {
                return text;
            }
            spin(0.05);
        }
        panic!("the page didn't answer {js:?}");
    }

    /// Sends a left click at `point` (window coordinates) straight to `web`.
    fn click(window: &NSWindow, web: &NSView, point: NSPoint) {
        for (kind, down) in [(1usize, true), (2usize, false)] {
            // SAFETY: +[NSEvent mouseEventWithType:…] and -[NSResponder mouseDown:/mouseUp:] on the main thread.
            unsafe {
                let event: Retained<AnyObject> = msg_send![
                    class!(NSEvent),
                    mouseEventWithType: kind,
                    location: point,
                    modifierFlags: 0usize,
                    timestamp: 0.0f64,
                    windowNumber: window.windowNumber(),
                    context: None::<&AnyObject>,
                    eventNumber: 0isize,
                    clickCount: 1isize,
                    pressure: 1.0f32
                ];
                if down {
                    let _: () = msg_send![web, mouseDown: &*event];
                } else {
                    let _: () = msg_send![web, mouseUp: &*event];
                }
            }
        }
    }

    /// A real WKWebView in a window as Tauri makes Klipolo's (full size under an opaque title bar, wry's plain content
    /// view, the webview filling it), never ordered on screen. The page has a box at the hole's CSS rect; the UI's
    /// rect for it goes to `set_rect`; a click in the middle of the preview view must reach the page in the middle of
    /// that box, wherever WebKit put the page inside the webview.
    pub fn run_webkit() {
        let mtm = MainThreadMarker::new().expect("harness = false runs on the main thread");
        // SAFETY: NSScreen class method on the main thread.
        let screen: Option<Retained<AnyObject>> =
            unsafe { msg_send![class!(NSScreen), mainScreen] };
        if screen.is_none() {
            eprintln!("native_view: no display, WebKit part skipped");
            return;
        }
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0));
        // SAFETY: NSWindow's designated initialiser; it is never closed, and not released when closed.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        // As wry builds it: both views start empty, and making the parent the content view sizes them to the window.
        let parent = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
        parent.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        // SAFETY: WKWebViewConfiguration and WKWebView's designated initialiser; a WKWebView is an NSView.
        let web: Retained<NSView> = unsafe {
            let config: Retained<AnyObject> = msg_send![class!(WKWebViewConfiguration), new];
            let web: Retained<AnyObject> = msg_send![
                msg_send![class!(WKWebView), alloc],
                initWithFrame: NSRect::ZERO,
                configuration: &*config
            ];
            Retained::cast_unchecked(web)
        };
        web.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        parent.addSubview(&web);
        window.setContentView(Some(&parent));
        let html = "<!doctype html><html><body style='margin:0'>\
            <div style='position:absolute;left:50px;top:100px;width:200px;height:80px'></div></body></html>";
        // SAFETY: -[WKWebView loadHTMLString:baseURL:] and -[WKWebView isLoading] on the main thread.
        unsafe {
            let _: Option<Retained<AnyObject>> = msg_send![&*web, loadHTMLString: &*NSString::from_str(html), baseURL: None::<&AnyObject>];
        }
        for _ in 0..200 {
            spin(0.05);
            // SAFETY: as above.
            let loading: bool = unsafe { msg_send![&*web, isLoading] };
            if !loading {
                break;
            }
        }
        let page = eval(&web, "JSON.stringify([innerWidth, innerHeight])");
        // The premise: WebKit inset the page under the title bar (the page is shorter than the webview).
        let height = web.frame().size.height;
        println!("native_view: webview {height} pt tall, page [innerWidth, innerHeight] = {page}");
        let inner: Vec<f64> = page
            .trim_matches(['[', ']'])
            .split(',')
            .map(|v| v.trim().parse().expect("a number"))
            .collect();
        assert!(
            inner[1] < height,
            "the page is inset under the title bar: {page} in {height} pt"
        );

        let scale = window.backingScaleFactor();
        macos::install(Retained::as_ptr(&web) as *mut c_void).expect("installed");
        spin(0.05);
        let hole = [50.0, 100.0, 200.0, 80.0];
        let rect = ViewRect {
            x: hole[0],
            y: hole[1],
            width: hole[2],
            height: hole[3],
            scale,
            backdrop: None,
        };
        assert!(
            macos::set_rect(Some(rect)),
            "set_rect runs on the main thread"
        );
        spin(0.05);

        // Where the preview view is, and a click in its middle (window coordinates).
        let subviews = parent.subviews();
        // SAFETY: the preview view is the content view's first subview (below the webview).
        let view: Retained<NSView> = unsafe {
            let view: *mut NSView = msg_send![&*subviews, objectAtIndex: 0usize];
            Retained::retain(view).expect("the preview view")
        };
        assert!(
            !std::ptr::eq(Retained::as_ptr(&view), Retained::as_ptr(&web)),
            "the preview view sits below the webview"
        );
        let f = view.frame();
        let middle = NSPoint::new(
            f.origin.x + f.size.width / 2.0,
            f.origin.y + f.size.height / 2.0,
        );
        let at = parent.convertPoint_toView(middle, None);
        eval(
            &web,
            "window.__clicks = []; addEventListener('mousedown', e => __clicks.push([e.clientX, e.clientY]), true); 'ok'",
        );
        click(&window, &web, at);
        spin(0.3);
        let clicks = eval(&web, "JSON.stringify(__clicks)");
        assert_eq!(
            clicks, "[[150,140]]",
            "a click in the middle of the preview reaches the page in the middle of the hole (CSS 150, 140)"
        );
        let i = macos::inspect().expect("installed");
        assert!(i.problems.is_empty(), "{:?}\n{i:#?}", i.problems);
        assert_eq!(
            i.frame, hole,
            "the view is over the hole, in the page's coordinates"
        );
        assert_eq!(
            i.page_origin,
            [0.0, height - inner[1]],
            "the inset WebKit applied"
        );

        // The title bar's inset goes (as in full screen): the page grows, the UI sends the same hole again (its
        // `resize` handler), and the view follows the page.
        window.setStyleMask(NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable);
        spin(0.3);
        let page = eval(&web, "JSON.stringify(innerHeight)");
        let now = web.frame().size.height;
        println!(
            "native_view: without the full-size content: webview {now} pt tall, innerHeight = {page}"
        );
        assert_eq!(page, format!("{now}"), "no inset any more");
        assert!(
            macos::set_rect(Some(rect)),
            "set_rect runs on the main thread"
        );
        spin(0.05);
        let f = view.frame();
        let middle = NSPoint::new(
            f.origin.x + f.size.width / 2.0,
            f.origin.y + f.size.height / 2.0,
        );
        eval(&web, "__clicks = []; 'ok'");
        click(&window, &web, parent.convertPoint_toView(middle, None));
        spin(0.3);
        assert_eq!(
            eval(&web, "JSON.stringify(__clicks)"),
            "[[150,140]]",
            "after the inset changed, the view follows the page"
        );
        let i = macos::inspect().expect("installed");
        assert!(
            i.problems.is_empty() && i.page_origin == [0.0, 0.0],
            "{i:#?}"
        );
        macos::uninstall();
        println!(
            "native_view: the preview sits over the hole under a title bar inset, and without it: ok"
        );
    }
}
