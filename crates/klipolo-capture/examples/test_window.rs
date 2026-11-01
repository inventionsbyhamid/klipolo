//! A window of Klipolo's own for the screen smoke run's app target: a regular app with one 960×540 window whose
//! large counter and background colour change 30 times a second, and no sound, for `seconds` (default 30). Recording it
//! as an app (`screen_smoke … app <this executable's path>`) records only this window, never another app's, and gives
//! the encoder changing content to measure its bitrate and latency on. With `--still` nothing in it ever changes: a fully
//! still picture, for the still-screen check. With `--top` it sits at the top of the main screen,
//! under where the camera bubble goes, for the bubble's moving-screen check. It captures nothing
//! itself.
//!
//! `cargo run -p klipolo-capture --features real-capture --example test_window -- [seconds] [--still] [--top]`

fn main() {
    #[cfg(target_os = "macos")]
    mac::run();
    #[cfg(not(target_os = "macos"))]
    println!("test_window: macOS only");
}

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    use cidre::dispatch;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFont,
        NSTextField, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    /// The window, its counter, and when it opened.
    type Face = (Retained<NSWindow>, Retained<NSTextField>, Instant);

    thread_local! {
        static FACE: RefCell<Option<Face>> = const { RefCell::new(None) };
    }

    pub fn run() {
        let seconds: u64 = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        let still = std::env::args().any(|a| a == "--still");
        let top = std::env::args().any(|a| a == "--top");
        let mtm = MainThreadMarker::new().expect("the main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        let frame = NSRect::new(NSPoint::new(120.0, 120.0), NSSize::new(960.0, 540.0));
        // SAFETY: a plain titled window, kept alive by FACE until the process ends.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                frame,
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Klipolo test window"));
        let label = NSTextField::labelWithString(&NSString::from_str("0"), mtm);
        label.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            160.0, 0.0,
        )));
        label.setFrame(NSRect::new(
            NSPoint::new(40.0, 160.0),
            NSSize::new(880.0, 220.0),
        ));
        if let Some(content) = window.contentView() {
            content.addSubview(&label);
        }
        if top && let Some(screen) = objc2_app_kit::NSScreen::mainScreen(mtm) {
            let visible = screen.visibleFrame();
            let left = visible.origin.x + (visible.size.width - 960.0) / 2.0;
            window.setFrameTopLeftPoint(NSPoint::new(left, visible.origin.y + visible.size.height));
        }
        // Shown without taking the keyboard from whatever the person is doing.
        window.orderFrontRegardless();
        // The window's number is its `CGWindowID`: `screen_smoke … window <id>` records it alone.
        println!(
            "test_window: pid {} window {} for {seconds} s, still {still}",
            std::process::id(),
            window.windowNumber()
        );
        FACE.with(|f| *f.borrow_mut() = Some((window, label, Instant::now())));
        std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < until {
                if !still {
                    dispatch::Queue::main().async_mut(tick);
                }
                std::thread::sleep(Duration::from_millis(33));
            }
            dispatch::Queue::main().async_mut(|| std::process::exit(0));
        });
        app.run();
    }

    /// A new counter and colour.
    fn tick() {
        FACE.with(|f| {
            if let Some((window, label, started)) = f.borrow().as_ref() {
                let ms = started.elapsed().as_millis();
                label.setStringValue(&NSString::from_str(&format!("{ms:>6}")));
                let hue = (ms % 4_000) as f64 / 4_000.0;
                window.setBackgroundColor(Some(
                    &NSColor::colorWithHue_saturation_brightness_alpha(hue, 0.6, 0.9, 1.0),
                ));
            }
        });
    }
}
