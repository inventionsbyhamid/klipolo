//! A take's cursor facts, on its recording clock: where the cursor rests (`still`), how hidden it is while
//! resting or hidden by an app (`idle_opacity`), where it paused after travelling (`dwell`), and span lists combined
//! into idle stretches (`spans`). Each is pure and computed once per take; the timeline mapping is `crate::timeline`.
//!
//! Until the recorder's writer marks a departure, a still run ends at its last kept position, which can be up to
//! one heartbeat (500 ms) before the cursor really leaves: the raw path drifts over that gap, and these facts follow
//! the raw path.

pub mod dwell;
pub mod spans;
pub mod still;

pub use dwell::{DWELL_MIN_US, DWELL_TRAVEL, Dwell, dwells};
pub use still::{
    FADE_IN_LEAD_US, FADE_OUT_US, HIDDEN_FADE_US, MIN_RUN_US, STILL_EPS, StillRuns, idle_opacity,
};
