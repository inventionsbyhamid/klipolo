//! Each display's choices and the offers at each quality there (`preflight`'s "Per display" and "Offers").

use klipolo_model::coded::CodedError;

use super::{Fitted, PresetOffer};
use crate::backend::{DisplayInfo, PresetId, ProbeReport};
use crate::disk::outlook;
use crate::english::msg;
use crate::preset::{Preset, bytes_per_s, native_preset, preset};
use crate::probe::{CameraChoice, Choices, ScreenChoice, decide, keeps_both, result_of};

/// What a take records beside the screen and the camera, and the space it has.
pub(super) struct Load {
    pub(super) mono: u32,
    pub(super) free_bytes: Option<u64>,
}

/// One display's choices (or, without one, those without a native size) with the camera on or off, and its offers. A
/// display without a size gets no native choice (never `Some((0, 0))`: every encoder would keep a 2×2 Native60).
pub(super) fn fit(
    report: &ProbeReport,
    display: Option<&DisplayInfo>,
    load: &Load,
    camera_on: bool,
) -> Fitted {
    let hz = display.map_or(60, |d| d.refresh_hz);
    let size = display
        .filter(|d| d.width > 0 && d.height > 0)
        .map(|d| (d.width, d.height));
    let choices = decide(report, hz, size, camera_on);
    // The screen alone, for the encoder each quality records on: fitting the camera steps presets down or drops the
    // native choice, but never moves a choice to another encoder.
    let alone = decide(report, hz, size, false);
    let offers = choices
        .available
        .iter()
        .filter_map(|&id| offer(report, &choices, &alone, id, size, load))
        .collect();
    Fitted { choices, offers }
}

fn offer(
    report: &ProbeReport,
    choices: &Choices,
    alone: &Choices,
    id: PresetId,
    size: Option<(u32, u32)>,
    load: &Load,
) -> Option<PresetOffer> {
    let native = matches!(id, PresetId::Native30 | PresetId::Native60);
    let on = if native { &alone.native } else { &alone.screen };
    let on = on.as_ref()?;
    let screen = native_preset(id, on.codec, size.unwrap_or((1920, 1080)));
    let (camera, camera_note) = beside(report, choices, on, &screen);
    let camera_preset = camera.as_ref().map(|c| preset(c.preset, c.codec));
    let bytes = bytes_per_s(Some(screen), camera_preset, load.mono, 1);
    let best = |c: &Option<ScreenChoice>| c.as_ref().is_some_and(|c| c.preset == id);
    let tutorials = if choices.native.is_some() {
        &choices.native
    } else {
        &choices.screen
    };
    Some(PresetOffer {
        id,
        width: screen.width,
        height: screen.height,
        fps: screen.fps,
        gb_per_hour: (bytes * 3_600.0 / 1e9) as f32,
        minutes_left: load.free_bytes.map(|f| outlook(f, bytes).minutes_left),
        best_native: best(tutorials),
        best_game: best(&choices.screen),
        camera,
        camera_note,
    })
}

/// The defaults' camera beside `screen` recorded on `on`'s encoder: as it is on another engine; on the same one (any of
/// its ids), while `keeps_both` holds for that engine, else at Camera720, else left out with the note.
fn beside(
    report: &ProbeReport,
    choices: &Choices,
    on: &ScreenChoice,
    screen: &Preset,
) -> (Option<CameraChoice>, Option<CodedError>) {
    let Some(mut camera) = choices.camera.clone() else {
        return (None, choices.camera_note.clone());
    };
    let (Some(e), Some(cam)) = (
        result_of(report, &on.encoder),
        result_of(report, &camera.encoder),
    ) else {
        return (Some(camera), None);
    };
    if cam.engine != e.engine {
        return (Some(camera), None);
    }
    loop {
        if keeps_both(e, screen, &preset(camera.preset, camera.codec)) {
            return (Some(camera), None);
        }
        if camera.preset != PresetId::Camera1080 {
            return (None, Some(msg!("capture.preflight.cameraWithScreen")));
        }
        camera.preset = PresetId::Camera720;
    }
}
