//! The sizes the worker draws at: a format's preview size (its canvas within 0.92 MP: 1280 × 720 landscape) and
//! its export sizes (1080p, 2160p), and a frame's size for a long edge.

use klipolo_model::project::FormatKind;

use super::protocol::SizeName;

/// The format's preview pixels within 0.92 MP.
pub fn preview_size(format: FormatKind) -> (u32, u32) {
    match format {
        FormatKind::Landscape => (1280, 720),
        FormatKind::Portrait => (720, 1280),
        FormatKind::Square => (960, 960),
    }
}

pub fn export_size(format: FormatKind, size: SizeName) -> (u32, u32) {
    let (w, h) = format.size();
    match size {
        SizeName::Preview => preview_size(format),
        SizeName::P1080 => (w, h),
        SizeName::P2160 => (w * 2, h * 2),
    }
}

/// The format's canvas with its longer side `long_edge` (each side at least 2).
pub fn frame_size(format: FormatKind, long_edge: u32) -> (u32, u32) {
    let (w, h) = format.size();
    let scale = f64::from(long_edge) / f64::from(w.max(h));
    let side = |n: u32| ((f64::from(n) * scale).round() as u32).max(2);
    (side(w), side(h))
}
