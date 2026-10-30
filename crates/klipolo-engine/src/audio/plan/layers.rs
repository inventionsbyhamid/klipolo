//! The layers' sound, after the main track's (`audio_plan`): every audio item (at its volume, fades and effect) and every
//! overlay video's own sound (at its speed and volume), unless the layer is muted. A muted track adds nothing, and
//! neither does a hidden overlay track; while any track is soloed only soloed tracks add theirs (`Solo::sounds`). A
//! sound layer that ducks is lowered under `speech` (`Duck`), when anyone is known to speak.

use std::sync::Arc;

use klipolo_model::project::Project;
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::{ItemContent, Solo};

use super::{AudioSource, Crossfade, Duck, audible, chain, drift_rate, enabled};
use crate::registry::Registry;

/// Pushes the layers' sources onto `sources`. Solo is resolved once per plan (O(tracks)); each track then asks in O(1).
pub(super) fn layers(
    p: &Project,
    registry: &Registry,
    speech: &Arc<[RangeUs]>,
    sources: &mut Vec<AudioSource>,
) {
    let solo = Solo::of(&p.timeline.tracks);
    for track in &p.timeline.tracks {
        if !solo.sounds(track) {
            continue;
        }
        for item in &track.items {
            let (asset_id, source_in_us, (speed, rate), gain, fades, filter) = match &item.content {
                // A separated recording source plays at its run's drift-corrected rate, as it did in its clip.
                ItemContent::Audio {
                    asset_id,
                    source_in_us,
                    volume,
                    muted: false,
                    fade_in_us,
                    fade_out_us,
                    effect,
                    drift_ppm,
                    centre_from: _,
                    tie: _,
                    duck: _,
                } => (
                    asset_id,
                    *source_in_us,
                    (1.0, drift_rate(*drift_ppm)),
                    *volume,
                    (*fade_in_us, *fade_out_us),
                    chain(registry, effect.iter().chain(enabled(&item.effects))),
                ),
                ItemContent::Video {
                    asset_id,
                    source_in_us,
                    speed,
                    volume,
                    muted: false,
                    ..
                } => (
                    asset_id,
                    *source_in_us,
                    (*speed, 1.0),
                    *volume,
                    (0, 0),
                    None,
                ),
                _ => continue,
            };
            let Some(asset) = audible(p, asset_id) else {
                continue;
            };
            if gain <= 0.0 || item.duration_us <= 0 {
                continue;
            }
            // A separated one-sided source plays from the channel its clip played it from; a sound layer may duck.
            let (centre_from, duck) = match &item.content {
                ItemContent::Audio {
                    centre_from, duck, ..
                } => (*centre_from, *duck),
                _ => (None, None),
            };
            let duck = duck
                .filter(|&db| db < 0.0 && !speech.is_empty())
                .map(|db| Duck {
                    db: db as f32,
                    speech: speech.clone(),
                });
            sources.push(AudioSource {
                path: registry.resolve_path(&asset.path).into_owned(),
                start_us: item.start_us,
                end_us: item.end_us(),
                source_in_us: source_in_us.saturating_sub(asset.lead_us()),
                speed,
                rate,
                gain,
                fade_in_us: fades.0,
                fade_out_us: fades.1,
                crossfade: Crossfade::default(),
                filter,
                centre_from,
                duck,
            });
        }
    }
}
