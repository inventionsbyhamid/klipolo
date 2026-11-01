//! Clip analysis for Meta's models: the 16 kHz mono WAV Voice Transcribe takes (`audio`), that WAV cut at silences
//! into requests of at most ten minutes (`chunks`), the per-file cache that keeps what a clip's analysis found
//! (`cache`), the clip's timed words (`transcript`, with `fillers`), and its scene log (`scenes`), made from a small copy
//! of the clip (`proxy`) and snapped to its hard cuts (`cuts`), and the speaker's path for a vertical or square crop
//! (`subject`). The services sit behind interfaces this crate owns (`providers`), with Meta as one adapter.

pub mod audio;
pub mod cache;
pub mod chunks;
pub mod cuts;
pub mod error;
pub mod fillers;
pub mod providers;
pub mod proxy;
pub mod scenes;
pub mod subject;
pub mod transcript;

pub use error::{AnalysisError, VideoError};
