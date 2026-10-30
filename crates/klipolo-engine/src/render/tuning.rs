//! The decoders' tunings: the preview's (warm, preview-sized, Cap's fixed deadline) and export's (full size, waiting
//! while the decoder makes progress).

use std::time::Duration;

use crate::decode_wait::{DecodeWait, EXPORT_WAIT};
use crate::vendor::cap::rendering::decoder::DecoderTuning;

/// Preview decoders stay warm for 5 s between requests while the editor is visible; hiding the window drops them.
/// This is the main footage's budget; the pool gives the camera and overlays less (`DecoderPool::tuning_for`).
pub const PREVIEW_TUNING: DecoderTuning = DecoderTuning {
    idle_timeout: Duration::from_secs(5),
    max_cache_bytes: 96 << 20,
    // Any box: the pool picks each role's preview size (`DecoderPool::tuning_for`).
    max_output: Some((1920, 1080)),
    // Real time: Cap's fixed deadline, and the pool recovers a frame that misses it.
    wait: DecodeWait::Fixed,
};

/// Export's decoders: full-size frames (no preview box) and a smaller cache, since export reads forward.
pub const EXPORT_TUNING: DecoderTuning = DecoderTuning {
    idle_timeout: Duration::from_secs(2),
    max_cache_bytes: 64 << 20,
    max_output: None,
    // Not real time: as long as the decoder makes progress (a loaded computer holds one for seconds).
    wait: EXPORT_WAIT,
};

/// `EXPORT_TUNING`, with a test's shorter waits in test and trial builds (`decode_wait::hooks::set_export_wait`).
pub fn export_tuning() -> DecoderTuning {
    #[cfg(any(test, feature = "trial"))]
    return DecoderTuning {
        wait: crate::decode_wait::hooks::export_wait().unwrap_or(EXPORT_TUNING.wait),
        ..EXPORT_TUNING
    };
    #[cfg(not(any(test, feature = "trial")))]
    EXPORT_TUNING
}
