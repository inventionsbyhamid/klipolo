//! Helpers the capture tests share. Modules that use `synthetic` items (`take`, `harness` and `fake`) are declared
//! `#[cfg(feature = "synthetic")]`, so CI's featureless `cargo check --all-targets` still
//! compiles every test.
#![allow(dead_code)]

pub mod config;
pub mod decode;
#[cfg(feature = "synthetic")]
pub mod harness;
#[cfg(feature = "synthetic")]
pub mod replay;
#[cfg(feature = "synthetic")]
pub mod take;
// After `take`, not in order, so `harness` (a `SessionCore` driven in virtual time) stays beside `decode`.
#[cfg(feature = "synthetic")]
pub mod fake;

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// A cursor shape's image: `w` × `h` of one colour, its hotspot at the top left, 16 points high.
pub fn shape(w: u16, h: u16) -> klipolo_capture::cursor::ShapeImage {
    klipolo_capture::cursor::ShapeImage {
        px: (w, h),
        rgba: [40, 40, 40, 255].repeat(usize::from(w) * usize::from(h)),
        hotspot_px: (0.0, 0.0),
        pts: 16.0,
        sys: None,
    }
}

/// Everything `rx` gets within `ms` of real time.
pub fn received_within<T>(rx: &Receiver<T>, ms: u64) -> Vec<T> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    let mut got = vec![];
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(item) => got.push(item),
            Err(_) => break,
        }
    }
    got
}

/// `note` says that `source` didn't start, and why: `capture.note.didntStart` with the source's label and the
/// `capture.error.*` code `why`.
pub fn didnt_start(note: &klipolo_model::coded::CodedError, source: &str, why: &str) -> bool {
    use klipolo_model::coded::ErrorParam;
    let coded = |name: &str, code: &str| matches!(note.params.get(name), Some(ErrorParam::Coded(e)) if e.code == code);
    note.code == "capture.note.didntStart"
        && coded("source", &format!("capture.source.{source}"))
        && coded("why", why)
}
