//! The values a clip's Speed and Volume take, here once: the clip card and the video layer card offer them (the page
//! reads them from `apps/desktop/src/lib/clip-settings.json`, which `export_bindings_clip_settings` writes), and chat's
//! `setClip` takes exactly these, refusing any other.

/// The speeds a main clip or a video layer offers; its length follows (2 plays it in half the time).
pub const SPEEDS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
/// The quietest volume: silent.
pub const VOLUME_MIN: f64 = 0.0;
/// The loudest volume: twice as loud as recorded (1 is as recorded).
pub const VOLUME_MAX: f64 = 2.0;
/// A volume moves in hundredths, as the card's slider does.
pub const VOLUME_STEP: f64 = 0.01;

/// Whether `speed` is one of `SPEEDS`.
pub fn is_speed(speed: f64) -> bool {
    SPEEDS.contains(&speed)
}

/// Whether `volume` is one the card's slider can set: from `VOLUME_MIN` to `VOLUME_MAX`, on a `VOLUME_STEP`.
pub fn is_volume(volume: f64) -> bool {
    let steps = (volume / VOLUME_STEP).round();
    (VOLUME_MIN..=VOLUME_MAX).contains(&volume) && (steps * VOLUME_STEP - volume).abs() < 1e-9
}
