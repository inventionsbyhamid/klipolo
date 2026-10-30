//! Quiet sound from the cache's 100 ms energy windows, for the cursor's idle stretches and
//! level is its K-weighted mean square in dB (`10·log10`), the definition the transcript's pauses use
//! (`klipolo_model::speech::silence`, `10·log10(power / 2)` of the same windows). This is a fixed threshold, a second
//! `speech::silence::pauses` (with its threshold that adapts to each file) is on this branch, `quiet_spans` calls it
//! instead of keeping a second definition of a pause.

use klipolo_model::time::Us;

use super::{PcmKey, PcmStore, WINDOW};
use crate::audio::mix::RATE;

/// The windows' length.
const WINDOW_US: Us = 100_000;

/// The spans of `path`'s sound whose every channel stays below `db` (dBFS) for at least `min_us`, from its 100 ms
/// energy windows, sorted and apart, in the file's own time; None until the sound cache has them. O(windows).
pub fn quiet_spans(cache: &PcmStore, path: &str, db: f32, min_us: Us) -> Option<Vec<(Us, Us)>> {
    let key = PcmKey {
        path: path.to_owned(),
    };
    let pcm = cache.ready(&key)?;
    let energies = cache.energies(&key, &pcm)?;
    let channels = pcm.channels() as u8;
    // A window is quiet when its loudest channel's mean square is below the threshold's. `mean` over exactly one window
    // (the last one's own frames) is its value, with a channel counted on both speakers: its own mean square is half.
    let below = 10f64.powf(f64::from(db) / 10.0);
    let frames = energies.frames() as usize;
    let windows = frames.div_ceil(WINDOW);
    let quiet = |w: usize| {
        let (a, b) = (w * WINDOW, ((w + 1) * WINDOW).min(frames));
        (0..channels).all(|c| energies.mean(a as f64, b as f64, Some(c)) / 2.0 < below)
    };
    let end = (frames as u64 * 1_000_000 / u64::from(RATE)) as Us;
    let mut out = Vec::new();
    let mut from = None;
    for w in 0..=windows {
        match (w < windows && quiet(w), from) {
            (true, None) => from = Some(w as Us * WINDOW_US),
            (false, Some(f)) => {
                let to = (w as Us * WINDOW_US).min(end);
                if to - f >= min_us {
                    out.push((f, to));
                }
                from = None;
            }
            _ => {}
        }
    }
    Some(out)
}
