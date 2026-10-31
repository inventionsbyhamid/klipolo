//! Drift fitting: an audio run's offset and clock drift from its anchors.
//!
//! PCM carries no timestamps, so once a second an audio run logs an `Anchor`: the session time of one of its frames.
//! An audio device's clock differs from the host's by ±10-100 ppm (100 ppm is 360 ms an hour), so the run's frames are
//! fitted to a line, `frame = rate × (1 + drift) × (t − offset)`, and the engine resamples by that ratio, so sources
//! sources stay within 10 ms of each other after an hour once corrected.
//!
//! The line itself (robust least squares, two rounds of rejection, exact integer means) moved to `klipolo_model::fit`
//! where Sync uses it too; this module re-exports it, so the recorder's code and tests are unchanged.

pub use klipolo_model::fit::{Anchor, Fit, MIN_ANCHORS, fit};
