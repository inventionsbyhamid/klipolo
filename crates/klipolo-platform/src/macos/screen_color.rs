//! The eyedropper on macOS (`screen_color`): `NSColorSampler`, the system's own loupe. AppKit draws it
//! outside the app and hands back only the colour the person clicked, so nothing here reads the screen, and it needs
//! no Screen Recording permission. It shows only on the person's press.

use std::cell::{Cell, RefCell};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSColorSampler, NSColorSpace};

thread_local! {
    /// The sampler showing now (the main thread's): a second press while it shows picks nothing.
    static ACTIVE: RefCell<Option<Retained<NSColorSampler>>> = const { RefCell::new(None) };
}

pub(crate) fn screen_color_supported() -> bool {
    true
}

pub(crate) fn sample_screen_color(done: Box<dyn FnOnce(Option<String>) + Send>) {
    // Off the main thread, or a sampler already showing: nothing is picked (`done` still runs, once).
    if MainThreadMarker::new().is_none() || ACTIVE.with_borrow(Option::is_some) {
        done(None);
        return;
    }
    let sampler = NSColorSampler::new();
    let done = Cell::new(Some(done));
    let handler = RcBlock::new(move |color: *mut NSColor| {
        // SAFETY: AppKit passes a valid NSColor, or nil when the person cancelled, on the main thread.
        let color = unsafe { color.as_ref() };
        let hex = color
            .and_then(|c| c.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace()))
            .map(|c| {
                crate::screen_color::hex(c.redComponent(), c.greenComponent(), c.blueComponent())
            });
        // Let go of the sampler when this event's autorelease pool drains, never inside its own handler.
        if let Some(sampler) = ACTIVE.take() {
            let _ = Retained::autorelease_ptr(sampler);
        }
        if let Some(done) = done.take() {
            done(hex);
        }
    });
    // Held before it shows, so a handler AppKit runs at once still finds it, and a press meanwhile picks nothing.
    ACTIVE.set(Some(sampler.clone()));
    // SAFETY: on the main thread; AppKit retains the sampler until its session ends, and ACTIVE holds it too.
    unsafe { sampler.showSamplerWithSelectionHandler(&handler) };
}
