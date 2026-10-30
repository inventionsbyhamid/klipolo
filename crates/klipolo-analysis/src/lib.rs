//! Clip analysis for Meta's models: the 16 kHz mono WAV Voice Transcribe takes (`audio`), that WAV cut at silences
//! into requests of at most ten minutes (`chunks`), the per-file cache that keeps what a clip's analysis found
//! (`cache`), and which of a transcript's words are filler sounds (`fillers`).

pub mod audio;
pub mod cache;
pub mod chunks;
pub mod fillers;
