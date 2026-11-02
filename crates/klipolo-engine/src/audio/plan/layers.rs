//! The layers' sound, after the main track's (`audio_plan`): every audio item (at its volume, fades and effect) and every
//! overlay video's own sound (at its speed and volume), unless the layer is muted. A muted track adds nothing, and
//! neither does a hidden overlay track; while any track is soloed only soloed tracks add theirs (`Solo::sounds`). A
//! sound layer that ducks is lowered under `speech` (`Duck`), when anyone is known to speak. Where one sound layer
//! plays its file to the end and the next on its track starts the same file again at once, as music laid back to back
//! does, both are marked as a loop's seam (`Seam`), which the mixer plays through without its edge ramps.

use std::sync::Arc;

use klipolo_model::project::Project;
use klipolo_model::time::{OVERLAP_TOLERANCE_US, RangeUs};
use klipolo_model::timeline::{Item, ItemContent, Solo};
use klipolo_model::volume;

use super::{AudioSource, Crossfade, Duck, Seam, audible, chain, drift_rate, enabled};
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
        // The track's last item that made a source, and that source's index, while the next item follows it.
        let mut before: Option<(&Item, usize)> = None;
        for item in &track.items {
            let previous = before.take();
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
                    volume_keys: _,
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
            let keys = item.content.volume_keys();
            if volume::silent(keys, gain) || item.duration_us <= 0 {
                continue;
            }
            let (gain, line) = super::level(gain, keys, item.start_us);
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
                line,
                seam: Seam::default(),
            });
            let here = sources.len() - 1;
            if let Some((was, at)) = previous
                && loops_on(p, was, &sources[at], item, &sources[here])
            {
                sources[at].seam.end = true;
                sources[here].seam.start = true;
            }
            before = Some((item, here));
        }
    }
}

/// Whether sound layer `next` (playing as `b`) starts its file again where layer `was` (playing as `a`), just before it
/// on its track, played the same file to its end: back to back, no fade out of `a` or into `b`, and the two the same
/// sound otherwise (the level where they meet, effect, rate, channel and ducking), so nothing changes but where the file is
/// read. A loop's
/// seam: the file joins itself there, so neither edge is ramped.
fn loops_on(p: &Project, was: &Item, a: &AudioSource, next: &Item, b: &AudioSource) -> bool {
    let (
        ItemContent::Audio {
            asset_id: file,
            source_in_us: from,
            duck: was_duck,
            ..
        },
        ItemContent::Audio {
            asset_id: next_file,
            source_in_us: next_from,
            duck: next_duck,
            ..
        },
    ) = (&was.content, &next.content)
    else {
        return false;
    };
    let Some(asset) = p.asset(file) else {
        return false;
    };
    file == next_file
        && *next_from == 0
        && from.saturating_add(was.duration_us) >= asset.duration_us
        && next.start_us.abs_diff(was.end_us()) <= OVERLAP_TOLERANCE_US.unsigned_abs()
        && a.fade_out_us == 0
        && b.fade_in_us == 0
        && a.level_at(a.end_us) == b.level_at(b.start_us)
        && a.rate == b.rate
        && a.speed == b.speed
        && a.filter == b.filter
        && a.centre_from == b.centre_from
        && was_duck == next_duck
}
