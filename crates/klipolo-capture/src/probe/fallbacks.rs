//! The encoders a take's video tries, in order (`VideoSpec::encoders`, tried by `encoder_pick`): the probe's choice for
//! the preset first, then every other hardware encoder, those whose measured capacity holds the preset (with the
//! native rule's margin) before those that don't, H.264 before HEVC and a dedicated engine first among each. One the
//! probe saw refuse (0 fps) comes last but is still tried: the probe may have run at a bad moment. So a take records
//! video while any encoder works.
//!
//! The camera's list (`camera_encoders_for`) follows the same rule for the camera preset, given that
//! a shared encoder is one physical engine: an encoder on the screen's engine holds the camera only if it holds both
//! together (`keeps_both`, as `decide` fits them), and one on another engine comes first, as `decide` prefers.

use super::{CameraChoice, HEADROOM, ScreenChoice, capacity, keeps_both, result_of};
use crate::backend::{EncoderPick, EncoderResult, ProbeReport};
use crate::media::Codec;
use crate::preset::{Preset, native_preset, preset};

/// `choice` (a `Choices::screen` or `native`) and its fallbacks from `report`; `display` sizes a native preset (None:
/// 1920×1080).
pub fn encoders_for(
    report: &ProbeReport,
    choice: &ScreenChoice,
    display: Option<(u32, u32)>,
) -> Vec<EncoderPick> {
    let size = display.unwrap_or((1920, 1080));
    let holds = |e: &EncoderResult| holds_alone(e, &native_preset(choice.preset, e.codec, size));
    ordered(report, pick(choice.codec, &choice.encoder), |e| {
        (!holds(e), refused(e), e.codec != Codec::H264, !e.dedicated)
    })
}

/// The camera's `choice` (`Choices::camera`) and its fallbacks from `report`, beside the take's `screen` choice on a
/// display of `display` (None: 1920×1080); `screen` None for a camera-only take.
pub fn camera_encoders_for(
    report: &ProbeReport,
    choice: &CameraChoice,
    screen: Option<&ScreenChoice>,
    display: Option<(u32, u32)>,
) -> Vec<EncoderPick> {
    let size = display.unwrap_or((1920, 1080));
    let beside = screen.and_then(|s| Some((result_of(report, &s.encoder)?, s.preset)));
    let shares = |e: &EncoderResult| beside.is_some_and(|(s, _)| s.engine == e.engine);
    let holds = |e: &EncoderResult| {
        let camera = preset(choice.preset, e.codec);
        match beside.filter(|_| shares(e)) {
            Some((s, id)) => keeps_both(e, &native_preset(id, s.codec, size), &camera),
            None => holds_alone(e, &camera),
        }
    };
    ordered(report, pick(choice.codec, &choice.encoder), |e| {
        (
            !holds(e),
            refused(e),
            shares(e),
            e.codec != Codec::H264,
            !e.dedicated,
        )
    })
}

/// `first`, then the report's other hardware encoders by `key` (stable: the report's order breaks ties).
fn ordered<K: Ord>(
    report: &ProbeReport,
    first: EncoderPick,
    key: impl Fn(&EncoderResult) -> K,
) -> Vec<EncoderPick> {
    let mut rest: Vec<&EncoderResult> = (report.encoders.iter())
        .filter(|e| e.hardware && e.id != first.id)
        .collect();
    rest.sort_by_key(|e| key(e));
    std::iter::once(first)
        .chain(rest.into_iter().map(|e| pick(e.codec, &e.id)))
        .collect()
}

/// Whether `e` alone keeps `p`'s pixel rate, with the margin.
fn holds_alone(e: &EncoderResult, p: &Preset) -> bool {
    let rate = f64::from(p.width) * f64::from(p.height) * f64::from(p.fps);
    capacity(e) >= HEADROOM * rate
}

/// The probe saw it refuse (0 fps).
fn refused(e: &EncoderResult) -> bool {
    capacity(e) <= 0.0
}

fn pick(codec: Codec, id: &str) -> EncoderPick {
    EncoderPick {
        codec,
        id: id.to_owned(),
    }
}
