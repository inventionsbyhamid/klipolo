//! Clip analysis for Meta's models: the 16 kHz mono WAV Voice Transcribe takes (`audio`), and that WAV cut at
//! silences into requests of at most ten minutes (`chunks`).

pub mod audio;
pub mod chunks;
