//! Clip analysis for Meta's models: the 16 kHz mono WAV Voice Transcribe takes (`audio`), that WAV cut at silences
//! into requests of at most ten minutes (`chunks`), and the per-file cache that keeps what a clip's analysis found
//! (`cache`).

pub mod audio;
pub mod cache;
pub mod chunks;
