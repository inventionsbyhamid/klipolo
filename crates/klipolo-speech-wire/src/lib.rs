//! What every Klipolo speech worker shares, whichever engine it runs: whisper.cpp in `klipolo --speech-worker`,
//! transcribe.cpp in `klipolo-asr --speech-worker`. Pure Rust with no ggml, so the two workers share it and never a
//! native library:
//! - `protocol`: the wire (requests on stdin with their samples, one JSON reply line each on stdout);
//! - `engine`: `SpeechEngine`, the one thing an engine implements, and `serve`, the request loop around it (`ready`
//!   after lowering, aborts, progress lines, compute times);
//! - `report`: the request in flight's abort and progress state, one per `serve`;
//! - `words`: a word as heard, and `snap_to_speech`, which every engine's words go through;
//! - `lower`: the worker's priority below normal, through the OS's own calls.

pub mod engine;
pub mod lower;
pub mod protocol;
pub mod report;
pub mod timing;
pub mod words;
