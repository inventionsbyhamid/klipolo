//! Meta's list prices in US dollars, and what one call costs.

use crate::Usage;

/// Muse Spark input, per token ($1.25 per 1M).
pub const SPARK_INPUT_USD: f64 = 1.25 / 1_000_000.0;
/// Muse Spark input served from cache, per token ($0.15 per 1M).
pub const SPARK_CACHED_INPUT_USD: f64 = 0.15 / 1_000_000.0;
/// Muse Spark output (reasoning included), per token ($4.25 per 1M).
pub const SPARK_OUTPUT_USD: f64 = 4.25 / 1_000_000.0;
/// Voice Transcribe, per hour of audio.
pub const ASR_USD_PER_HOUR: f64 = 0.18;
/// SAM, per video frame ($0.20 per 1,000).
pub const SAM_FRAME_USD: f64 = 0.20 / 1000.0;
/// Meta bills at least this many frames per SAM video request.
pub const SAM_MIN_FRAMES: u64 = 50;
/// Muse Image, per image returned.
pub const IMAGE_USD: f64 = 0.01;
/// Spark input tokens per sampled video frame at 640x360 (measured).
pub const VIDEO_TOKENS_PER_FRAME: u64 = 131;
/// Spark input tokens per second of a video's sound (measured).
pub const AUDIO_TOKENS_PER_SECOND: u64 = 10;

/// The cost of a Muse Spark call. Cached input tokens are part of
/// `input_tokens` and cost the cached price instead.
pub fn spark_usd(usage: Usage) -> f64 {
    let cached = usage.cached_tokens.min(usage.input_tokens);
    (usage.input_tokens - cached) as f64 * SPARK_INPUT_USD
        + cached as f64 * SPARK_CACHED_INPUT_USD
        + usage.output_tokens as f64 * SPARK_OUTPUT_USD
}

/// The cost of transcribing `duration_ms` of audio.
pub fn asr_usd(duration_ms: u64) -> f64 {
    duration_ms as f64 / 3_600_000.0 * ASR_USD_PER_HOUR
}

/// The cost of a SAM video call: its frames, at least [`SAM_MIN_FRAMES`].
pub fn sam_usd(usage: Usage) -> f64 {
    let frames = match usage.video_frames_processed {
        0 => 0,
        n => n.max(SAM_MIN_FRAMES),
    };
    frames as f64 * SAM_FRAME_USD
}

/// The cost of a Muse Image answer with `images` pictures.
pub fn image_usd(images: usize) -> f64 {
    images as f64 * IMAGE_USD
}

/// Spark input tokens for `seconds` of 640x360 video sampled at `fps`
/// (sound included), without the prompt. For estimates before a call.
pub fn spark_video_tokens(seconds: f64, fps: f64) -> u64 {
    if !(seconds.is_finite() && fps.is_finite() && seconds > 0.0 && fps > 0.0) {
        return 0;
    }
    let frames = (seconds * fps).ceil() as u64;
    let audio = (seconds * AUDIO_TOKENS_PER_SECOND as f64).ceil() as u64;
    frames * VIDEO_TOKENS_PER_FRAME + audio
}
