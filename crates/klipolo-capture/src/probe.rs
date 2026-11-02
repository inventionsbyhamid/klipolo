//! What this computer records by default, from one rule over its probe. It is
//! data, never `cfg`: the Mac's T2, Apple silicon, an NVIDIA PC and a VA-API laptop all go through `decide`. Nothing
//! here captures anything: the probe already ran on generated frames, and this only reads its numbers.
//!
//! - **Games** (`screen`): H.264 1080p60 on a *dedicated* engine that held 60 fps, so the encoder doesn't share
//!   the game's GPU; else HEVC 1080p60 on a dedicated engine (the T2's AVE); else 1080p30 on any hardware encoder,
//!   H.264 first; else no video recording (`capture.preflight.noEncoder`).
//! - **Whole-screen and window recordings** (`native`): the display's own size, 60 fps where
//!   the encoder keeps up, preferring a dedicated HEVC engine ("HEVC on T2 and Apple Silicon Macs, H.264 otherwise").
//! - **The camera**, when the take records one (`camera_on`, known at Record), stays off the screen's engine where there
//!   is an H.264 encoder on another; else it shares the screen's encoder, in its codec (an HEVC-only computer too). On
//!   a shared engine (`EncoderResult::engine`: an H.264 camera beside an HEVC native recording on Apple silicon's one
//!   media engine counts) the two must fit together (`keeps_both`): the camera steps down first, then the screen, and if
//!   nothing fits the camera goes with a note. Nothing is ever left to overload the encoder silently. A take without
//!   the camera keeps the defaults the screen alone can hold.
//!
//! The FFmpeg encoder ladders Windows and Linux try are in `ladder` (Cap's Windows tables; Klipolo's own Linux one), and
//! the encoders a take falls back to when its choice won't start in `fallbacks`.

mod fallbacks;
mod ladder;

pub use fallbacks::{camera_encoders_for, encoders_for};
pub use ladder::{GpuVendor, ladder, live_options};

use klipolo_model::coded::CodedError;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::backend::{EncoderResult, PresetId, ProbeReport};
use crate::english::msg;
use crate::media::Codec;
use crate::preset::{Preset, native_preset, native_size};

/// The probe's choices. The recorder offers `native` first for tutorials and `screen` for games.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Choices {
    /// Games. None: this computer can't record video.
    pub screen: Option<ScreenChoice>,
    /// Whole-screen and window recordings (the display's own size, 60 fps where the encoder keeps up). None:
    /// the recorder uses `screen`.
    pub native: Option<ScreenChoice>,
    /// The screen presets this computer can sustain, in the Quality list's order: the native ones first, then Light,
    /// Smooth, Sharp and Slow motion. They are for the screen alone: with the camera on the same encoder, a preset the
    /// person picks is checked with `keeps_both`.
    pub available: Vec<PresetId>,
    /// Already fitted beside `screen` and `native` where it shares their encoder. None without `camera_on`.
    pub camera: Option<CameraChoice>,
    /// Why there is no camera although an encoder could record one: `capture.preflight.cameraWithScreen`, the encoder
    /// it would share can't take it beside the screen even at the lowest presets.
    pub camera_note: Option<CodedError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScreenChoice {
    pub preset: PresetId,
    pub codec: Codec,
    /// `EncoderResult::id`.
    pub encoder: String,
}

/// The camera at Camera720 or Camera1080: H.264 on an encoder of its own, else the screen's encoder in its codec.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CameraChoice {
    pub preset: PresetId,
    pub codec: Codec,
    pub encoder: String,
}

/// Paced results at or above this held the rate (60 fps pacing).
const HELD_60: f32 = 59.0;
const HELD_30: f32 = 30.0;
const HELD_120: f32 = 118.0;
/// The margin an encoder keeps over the pixel rate it is given: the native rule's, and the screen and camera together.
const HEADROOM: f64 = 1.05;
/// `max_fps_1440p` is measured on 2560×1440 frames.
const PIXELS_1440P: f64 = 2560.0 * 1440.0;
/// The paced runs `fps_1080p60` and `fps_1080p120` use 1920×1080 frames.
const PIXELS_1080P: f64 = 1920.0 * 1080.0;

/// `display_hz`: the main display's refresh rate (Slow motion needs 120 Hz). `display`: its pixel size, or None when
/// unknown (no native choice then). `camera_on`: the take records the camera; only then is a camera chosen and fitted
/// beside the screen, so a take without one keeps what the screen alone can hold (Smooth, Native60).
pub fn decide(
    report: &ProbeReport,
    display_hz: u32,
    display: Option<(u32, u32)>,
    camera_on: bool,
) -> Choices {
    let hw = || report.encoders.iter().filter(|e| e.hardware);
    let dedicated = |codec| hw().filter(move |e| e.dedicated && e.codec == codec);
    let any = |codec| hw().filter(move |e| e.codec == codec);
    let held = |min: f32| move |e: &&EncoderResult| e.fps_1080p60 >= min;

    let smooth = best(dedicated(Codec::H264).filter(held(HELD_60)))
        .or_else(|| best(dedicated(Codec::Hevc).filter(held(HELD_60))));
    let light = || {
        best(any(Codec::H264).filter(held(HELD_30)))
            .or_else(|| best(any(Codec::Hevc).filter(held(HELD_30))))
    };
    let screen = match smooth {
        Some(e) => Some((PresetId::Smooth, e)),
        None => light().map(|e| (PresetId::Light, e)),
    };

    // The first encoder in that order whose flat-out pixel rate keeps the display's own size at 60, else at 30.
    let native = display.and_then(|size| {
        let order = || {
            dedicated(Codec::Hevc)
                .chain(dedicated(Codec::H264))
                .chain(any(Codec::H264).filter(|e| !e.dedicated))
                .chain(any(Codec::Hevc).filter(|e| !e.dedicated))
        };
        let (w, h) = native_size(size);
        let pixels = f64::from(w) * f64::from(h);
        let keeps = |fps: f64| {
            move |e: &&EncoderResult| {
                f64::from(e.max_fps_1440p) * PIXELS_1440P >= HEADROOM * pixels * fps
            }
        };
        order()
            .find(keeps(60.0))
            .map(|e| (PresetId::Native60, e))
            .or_else(|| order().find(keeps(30.0)).map(|e| (PresetId::Native30, e)))
    });

    let mut available = vec![];
    if let Some((id, _)) = native {
        available.push(id);
    }
    if let Some((id, e)) = screen {
        available.push(PresetId::Light);
        if id == PresetId::Smooth {
            available.push(PresetId::Smooth);
        }
        if e.dedicated && e.fps_1440p60 >= HELD_60 {
            available.push(PresetId::Sharp);
        }
        if e.dedicated && e.fps_1080p120 >= HELD_120 && display_hz >= 120 {
            available.push(PresetId::SlowMo);
        }
    }

    // H.264 on another engine than the screen's (a dedicated one first, then the report's order), else the screen's
    // own encoder (or, with no screen choice, the native one's) in its codec.
    let screen_engine = screen.map(|(_, e)| e.engine.as_str());
    let camera = any(Codec::H264)
        .filter(|_| camera_on)
        .filter(|e| Some(e.engine.as_str()) != screen_engine)
        .min_by_key(|e| !e.dedicated)
        .or_else(|| screen.or(native).filter(|_| camera_on).map(|(_, e)| e))
        .map(|e| {
            let preset = if e.dedicated {
                PresetId::Camera1080
            } else {
                PresetId::Camera720
            };
            (preset, e)
        });
    let size = display.unwrap_or((1920, 1080));
    let (screen, native, camera, camera_note) = fit_camera(screen, native, camera, size);
    // A native preset stepped down for the camera is offered after the one the screen alone keeps.
    if let Some((id, _)) = native
        && !available.contains(&id)
    {
        let natives = available
            .iter()
            .take_while(|p| matches!(p, PresetId::Native60 | PresetId::Native30))
            .count();
        available.insert(natives, id);
    }

    let choice = |(preset, e): (PresetId, &EncoderResult)| ScreenChoice {
        preset,
        codec: e.codec,
        encoder: e.id.clone(),
    };
    Choices {
        screen: screen.map(choice),
        native: native.map(choice),
        available,
        camera: camera.map(|(preset, e)| CameraChoice {
            preset,
            codec: e.codec,
            encoder: e.id.clone(),
        }),
        camera_note,
    }
}

type Pick<'a> = Option<(PresetId, &'a EncoderResult)>;

/// The camera beside the choices whose engine it shares, whichever of the engine's ids each records on. While either is over that encoder's capacity,
/// the camera steps down first (Camera1080 to Camera720), then that choice: Smooth to Light; Native60 to Native30, then
/// no native choice (tutorials record at the screen's). When the screen choice can't step (or there is no screen choice
/// to fall back on), the camera goes, with the `cameraWithScreen` note: video always comes before the camera.
fn fit_camera<'a>(
    mut screen: Pick<'a>,
    mut native: Pick<'a>,
    mut camera: Pick<'a>,
    display: (u32, u32),
) -> (Pick<'a>, Pick<'a>, Pick<'a>, Option<CodedError>) {
    let sized = |id, codec| native_preset(id, codec, display);
    let gone = Some(msg!("capture.preflight.cameraWithScreen"));
    while let Some((cam, ce)) = camera {
        let over = |pick: Pick<'_>| {
            pick.is_some_and(|(id, e)| {
                e.engine == ce.engine && !keeps_both(e, &sized(id, e.codec), &sized(cam, ce.codec))
            })
        };
        if cam == PresetId::Camera1080 && (over(screen) || over(native)) {
            camera = Some((PresetId::Camera720, ce));
        } else if over(screen) {
            match screen {
                Some((PresetId::Smooth, e)) => screen = Some((PresetId::Light, e)),
                _ => return (screen, native, None, gone),
            }
        } else if over(native) {
            match native {
                Some((PresetId::Native60, e)) => native = Some((PresetId::Native30, e)),
                _ if screen.is_some() => native = None,
                _ => return (screen, native, None, gone),
            }
        } else {
            break;
        }
    }
    (screen, native, camera, None)
}

/// The camera recorded on its own ("Record yourself", a Camera take): no screen shares an encoder with it, so it
/// records at Camera1080 on the first hardware encoder that holds 1080p30 alone (with the native rule's 5 % margin),
/// else at Camera720 on the first that holds that, else at Camera720 on the first there is (the probe may have run at a
/// bad moment). H.264 before HEVC, a dedicated engine first among each, then the report's order. None without a
/// hardware encoder.
pub fn camera_alone(report: &ProbeReport) -> Option<CameraChoice> {
    let mut hw: Vec<&EncoderResult> = (report.encoders.iter()).filter(|e| e.hardware).collect();
    hw.sort_by_key(|e| (e.codec != Codec::H264, !e.dedicated));
    let holds = |id: PresetId| {
        move |e: &&&EncoderResult| {
            let p = crate::preset::preset(id, e.codec);
            capacity(e) >= HEADROOM * f64::from(p.width) * f64::from(p.height) * f64::from(p.fps)
        }
    };
    let (preset, e) = (hw.iter().find(holds(PresetId::Camera1080)))
        .map(|e| (PresetId::Camera1080, *e))
        .or_else(|| {
            let e = hw.iter().find(holds(PresetId::Camera720)).or(hw.first())?;
            Some((PresetId::Camera720, *e))
        })?;
    Some(CameraChoice {
        preset,
        codec: e.codec,
        encoder: e.id.clone(),
    })
}

/// Whether `encoder` keeps `screen` and `camera` together, with a 5 % margin: the camera on the screen's engine (one of
/// its ids, or another). `decide` fits its defaults with it; the preflight asks it again for a preset the person picks
/// while the camera is on.
pub fn keeps_both(encoder: &EncoderResult, screen: &Preset, camera: &Preset) -> bool {
    let rate = |p: &Preset| f64::from(p.width) * f64::from(p.height) * f64::from(p.fps);
    capacity(encoder) >= HEADROOM * (rate(screen) + rate(camera))
}

/// The probe's result for `encoder` (an `EncoderResult::id`).
pub fn result_of<'a>(report: &'a ProbeReport, encoder: &str) -> Option<&'a EncoderResult> {
    report.encoders.iter().find(|e| e.id == encoder)
}

/// What the probe measured `e` encoding per second, in pixels. A paced 1080p60 run that fell short of 60 found the
/// encoder's limit; one that held 60 shows only the pacing, so its flat-out 1440p run says how far past it goes.
fn capacity(e: &EncoderResult) -> f64 {
    let paced = f64::from(e.fps_1080p60) * PIXELS_1080P;
    if e.fps_1080p60 < HELD_60 {
        paced
    } else {
        paced.max(f64::from(e.max_fps_1440p) * PIXELS_1440P)
    }
}

/// Of the encoders that fit one rule, the one with the most headroom (1440p flat out); the report's order breaks ties.
fn best<'a>(fits: impl Iterator<Item = &'a EncoderResult>) -> Option<&'a EncoderResult> {
    fits.fold(None, |kept: Option<&EncoderResult>, e| match kept {
        Some(k) if k.max_fps_1440p >= e.max_fps_1440p => Some(k),
        _ => Some(e),
    })
}
