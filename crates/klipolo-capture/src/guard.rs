//! Capture begins only after the person starts it, never in tests. Every real backend and pipe constructor
//! calls this first; the test commands and CI all set `KLIPOLO_TEST`.

/// Panics when `KLIPOLO_TEST` is set: `what` names the capture that a test or CI run was about to start.
pub fn assert_real_capture_allowed(what: &str) {
    if std::env::var_os("KLIPOLO_TEST").is_some() {
        panic!("real capture in a test: {what} (use the synthetic backend)");
    }
}
