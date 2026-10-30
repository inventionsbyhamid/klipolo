//! Generator layers' time (effect ABI v1.1). A generator's passes, its own and its layer's
//! filters', see its piece's place in the contribution's time, so the second piece of a split layer continues the
//! first; a generator with `fps` holds its time on `1/fps` steps; and one that reads no time sees none. A generator
//! whose picture is a previous frame's (static, or within one step) gets a cache key, so the compositor draws it once
//! (`render::effects::statics`).

use klipolo_model::time::{Us, us_to_secs};
use klipolo_model::timeline::PieceTime;

use super::{EffectPass, frame_index, frame_start_us};
use crate::registry::{MAX_STEP_FPS, fnv1a64, fnv1a64_on};

/// How far past a step or frame boundary local time may fall short and still count as on it. Local time is the
/// difference of two µs-rounded frame starts (the frame drawn and the layer's start), so it can land 1 µs before a
/// boundary they share; boundaries they don't share are at least 1/3600 s (277 µs) away at rates up to 60 fps, so the
/// slack only settles those ties.
const SLACK_US: Us = 2;

/// The time, progress and frame a generator layer's passes see: its piece's place in the contribution's time, held
/// on `fps` steps, and all zero for a generator that never reads them (so its output can be cached).
///
/// Steps follow the engine's frame grid at `fps` (`frame_index`: the step changes when `floor(time × fps)` does, with
/// each step's start rounded to the µs, as frames' are). The frame is the piece time's frame at `grain_fps`, the grain
/// clock every layer's `kl.frame` counts at (`plan::GRAIN_FPS`, 30), so `kl_rand` grain refreshes at 30 Hz on every
/// layer, continues across a split and holds within a step.
pub fn generator_time(
    piece: PieceTime,
    local_us: Us,
    fps: Option<u32>,
    animates: bool,
    grain_fps: f64,
) -> (f32, f32, u32) {
    if !animates {
        return (0.0, 0.0, 0);
    }
    let mut t = piece.offset_us.saturating_add(local_us.max(0));
    if let Some(fps) = fps.filter(|f| (1..=MAX_STEP_FPS).contains(f)) {
        t = frame_start_us(frame_index(t.saturating_add(SLACK_US), fps), fps);
    }
    // NaN and anything under 1 read as 1 frame a second; the cast saturates.
    let rate = grain_fps.max(1.0).round() as u32;
    let frame = u32::try_from(frame_index(t.saturating_add(SLACK_US), rate)).unwrap_or(u32::MAX);
    let progress = (t as f64 / piece.span_us.max(1) as f64).clamp(0.0, 1.0);
    (us_to_secs(t) as f32, progress as f32, frame)
}

/// `pass` (a generator's own, or one of its layer's filters) re-timed to the layer's `piece` at `local_us` into the
/// layer: its time, progress and frame by [`generator_time`] with its shader's `fps` and `animates` (the frame at
/// `grain_fps`), `kl.duration` the piece's whole span and `kl.step_fps` its step rate (`kl.fps`, the project's rate,
/// stays as `effects::pass` set it).
pub(super) fn follow_piece(pass: &mut EffectPass, piece: PieceTime, local_us: Us, grain_fps: f64) {
    let shader = &pass.shader;
    let (time_s, progress, frame) =
        generator_time(piece, local_us, shader.fps, shader.animates, grain_fps);
    pass.time_s = time_s;
    pass.progress = progress;
    pass.frame = Some(frame);
    pass.duration_s = us_to_secs(piece.span_us.max(0)) as f32;
    pass.step_fps = shader.fps.map_or(0.0, |f| f as f32);
}

/// The cache key of a generator's own pass, re-timed by [`follow_piece`]: Some when its picture is the same as a
/// previous frame's with the same key (it reads no time, or its time holds on `fps` steps), else None. Over what its
/// pass reads that the plan decides: the shader, its settings, its time, progress and frame, `kl.duration`, `kl.fps`
/// and `kl.step_fps`. The compositor adds what it decides (size, `kl.px`, `kl.content`) before it reuses a picture.
pub(super) fn cache_key(pass: &EffectPass) -> Option<u64> {
    let shader = &pass.shader;
    if shader.animates && shader.fps.is_none() {
        return None;
    }
    let h = fnv1a64(&shader.key.to_le_bytes());
    let h = fnv1a64_on(h, pass.params.bytes());
    let timing = [
        pass.time_s.to_bits(),
        pass.progress.to_bits(),
        pass.frame.unwrap_or(0),
        pass.duration_s.to_bits(),
        pass.fps.to_bits(),
        pass.step_fps.to_bits(),
        pass.seed,
    ];
    Some(
        timing
            .iter()
            .fold(h, |h, v| fnv1a64_on(h, &v.to_le_bytes())),
    )
}
