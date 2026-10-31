//! Screen Recording's own request, the one permission request Klipolo makes itself. It runs only
//! on the person's press of "Allow screen recording" in the recorder (the consent rule: a permission request begins only after
//! the person's explicit press), never while listing, in preflight or in a test (the consent guard).
//!
//! `CGRequestScreenCaptureAccess` adds Klipolo to Privacy & Security › Screen Recording and shows macOS's own prompt the
//! first time. macOS applies a grant only to a relaunched app, so the recorder then asks the person to reopen Klipolo.

use objc2_core_graphics::CGRequestScreenCaptureAccess;

/// Asks macOS for Screen Recording. Its answer (whether Klipolo may record now) is only logged: a grant made now applies
/// once Klipolo reopens, which the recorder's step says.
pub(super) fn request() {
    crate::guard::assert_real_capture_allowed("the Screen Recording request");
    let granted = CGRequestScreenCaptureAccess();
    tracing::info!(granted, "recorder: Screen Recording requested");
}
