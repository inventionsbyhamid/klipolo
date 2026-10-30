//! The presenter seam: the native view the app made for the preview reaches the engine as an opaque
//! `NativeSurface`, so the session, the backend trait and the actor never name an OS type.
//! - macOS: a CAMetalLayer under the transparent WKWebView (`present::macos::install`);
//! - Windows (W: a DirectComposition visual or a child-HWND swap chain) and Linux (L: a Wayland subsurface): none
//!   yet, so the socket presents.

/// The view a native presenter draws into.
pub enum NativeSurface {
    #[cfg(target_os = "macos")]
    Metal(crate::present::macos::NativeLayer),
    /// No native presenter on this OS yet: never made.
    #[cfg(not(target_os = "macos"))]
    Unsupported(std::convert::Infallible),
}

/// After the native presenter failed: takes its view out from under the webview (macOS, on the main thread soon);
/// nothing to do elsewhere.
pub fn uninstall_soon() {
    #[cfg(target_os = "macos")]
    crate::present::macos::uninstall_soon();
}
