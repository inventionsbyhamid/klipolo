//! Preview audio: which sounds play where (`plan`); each file decoded once, streaming, into the PCM cache
//! (`decode`, `pcm`); the timeline mix with live speed changes and effects (`mix`, `stretch`; the effects' filter
//! chains come from `registry`); loudness
//! (`loudness`); and the cpal output whose clock the picture follows (`output`, `clock`).

pub mod clock;
pub mod decode;
pub mod limiter;
pub mod loudness;
pub mod mix;
pub mod output;
pub mod pcm;
#[cfg(test)]
mod perf;
pub mod plan;
mod stream;
pub mod stretch;

use klipolo_platform::audio_route as route;
