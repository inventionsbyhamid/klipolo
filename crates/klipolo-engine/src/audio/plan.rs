//! Pure: which audio plays where on the timeline: the main track's clips, then audio items and the sound of
//! overlay video layers; and which edits can't change that (`may_change_sound`).

mod changes;
mod fill;
mod layers;
mod speech;

use std::sync::Arc;

use klipolo_model::ContribId;
use klipolo_model::project::{Asset, Loudness, Project};
use klipolo_model::query::{RunInClip, clip_starts, run_in_clip, run_within, sound_sources};
use klipolo_model::time::{RangeUs, Us, unscaled};
use klipolo_model::timeline::{ClipSource, EffectInstance};
use klipolo_model::transitions::split;

use crate::registry::Registry;
pub use changes::may_change_sound;
pub use speech::{JOIN_US, ducking_speech, ducking_speech_after, ducks, speech_ranges};

#[derive(Clone, Debug, PartialEq)]
pub struct AudioPlan {
    pub sources: Vec<AudioSource>,
    pub loudness: Loudness,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSource {
    pub path: String,
    /// Timeline span `[start_us, end_us)`.
    pub start_us: Us,
    pub end_us: Us,
    /// The file's sound time heard at `start_us`: the file time its picture shows there less its sound lead
    /// (`Asset::lead_us`; 0 for a sound file); negative when the sound starts later (silence until then).
    pub source_in_us: Us,
    pub speed: f64,
    /// The file's own time per µs of the recording clock: `1 + drift_ppm·10⁻⁶` for a recording's run, or a separated
    /// layer of one, whose device clock ran fast or slow; 1 for everything else. It plays at
    /// `speed × rate` source frames per timeline frame (`step`).
    pub rate: f64,
    pub gain: f64,
    pub fade_in_us: Us,
    pub fade_out_us: Us,
    /// Which of the fades are a transition's crossfade (equal power) rather than a fade to silence (linear).
    pub crossfade: Crossfade,
    /// The audio effects' FFmpeg filter chain, in order (each one's `Registry::audio_filter`, joined): a sound layer's
    /// effect and then its own effect stack, or a recording source's mix effects. None for no effect, or none this
    /// build has (the sound plays dry).
    pub filter: Option<Arc<str>>,
    /// The one channel of the file it plays, on both sides at the level a one-channel file plays at: a one-sided
    /// recording source played centred (`SourceMix::centre_from`), or a separated layer of one. None plays the file as
    /// recorded; a one-channel file plays centred either way.
    pub centre_from: Option<u8>,
    /// Lowered under speech (a sound layer's `ItemContent::Audio::duck`, when anyone is known to speak); None plays at
    /// `gain` throughout.
    pub duck: Option<Duck>,
    /// Where it plays on from the piece before it, or into the one after it, across a loop's seam (`plan::layers`):
    /// the mixer ramps neither edge there.
    pub seam: Seam,
}

/// A loop's seam at a source's edges: a piece of music that plays its file to the end, followed on its track by the
/// next piece of the same file from its start, with no fade between (a song laid back to back under a longer video).
/// The file loops cleanly there, so the mixer's 3 ms edge ramps, which would dip the music to silence at every seam,
/// are left out on both sides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seam {
    /// It starts the file again where the piece before it played the file to its end.
    pub start: bool,
    /// It plays the file to its end where the piece after it starts the file again.
    pub end: bool,
}

/// How a source ducks under speech: `db` lower (negative) inside `speech`, easing down before each range and back up
/// after it (the mixer's envelope). `speech` is in timeline µs, sorted and apart (`speech_ranges`), and shared by every
/// ducking source of a plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Duck {
    pub db: f32,
    pub speech: Arc<[RangeUs]>,
}

impl AudioSource {
    /// Source frames played per timeline frame: the speed change and the drift correction together.
    pub fn step(&self) -> f64 {
        self.speed * self.rate
    }
}

/// The chain of the effects this build has, in order (`AudioSource::filter`); one allocation per effected source, at plan
/// time only.
fn chain<'a>(
    registry: &Registry,
    effects: impl IntoIterator<Item = &'a ContribId>,
) -> Option<Arc<str>> {
    let mut joined: Option<String> = None;
    for f in effects
        .into_iter()
        .filter_map(|id| registry.audio_filter(id))
    {
        match &mut joined {
            Some(s) => {
                s.push(',');
                s.push_str(f);
            }
            None => joined = Some(f.to_owned()),
        }
    }
    joined.map(Arc::from)
}

/// The effects of a stack that are on.
fn enabled(stack: &[EffectInstance]) -> impl Iterator<Item = &ContribId> {
    stack.iter().filter(|e| e.enabled).map(|e| &e.effect)
}

/// `1 + ppm·10⁻⁶`: a device clock `ppm` fast plays that much more of its file per µs (`SourceRun::local_us`).
fn drift_rate(ppm: f64) -> f64 {
    1.0 + ppm * 1e-6
}

/// Which of a source's fades cross into another clip's sound across a transition. Those are equal power: the
/// outgoing sound falls as cos and the incoming one rises as sin over the window, so two unrelated
/// sounds keep their summed power level instead of dipping about 3 dB in the middle. Every other fade goes to silence
/// in a straight line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Crossfade {
    pub fade_in: bool,
    pub fade_out: bool,
}

/// `id`'s file when it has sound.
fn audible<'a>(p: &'a Project, id: &str) -> Option<&'a Asset> {
    p.asset(id).filter(|a| a.has_audio)
}

/// The main track's sound: each clip's own audio (imported clips) or its recording's sound sources and any picture source
/// whose files carry sound (`query::sound_sources`; each run of them the
/// clip plays, at the source's mix in that clip: `Clip::mix_for`, centred from one channel where it says so, and at the
/// run's drift-corrected `rate`; a gap between runs plays nothing, but where no sound source recorded anything a muted
/// camera sound that fills gaps plays: `fill`), at the clip's volume with its fades (to silence). A transition
/// crossfades the two clips' sound over its window instead: the outgoing clip plays on past its cut and fades out,
/// the incoming one starts before it and fades in. Muted and silent clips add nothing. Then every audio item (at its
/// volume, fades and effect) and every overlay video's own sound (at its speed and volume), unless the layer is
/// muted. A muted track adds nothing, and neither does a hidden overlay track (audio tracks have no hide control).
/// While any track is soloed only soloed tracks add theirs, and mute still wins (`Solo::sounds`); the main track
/// always plays. Export plans its sound here too, so what plays is what exports.
///
/// A file's time is its picture's (each stream counts from its own start), so a picture file's own sound reads its
/// file its lead (`Asset::lead_us`) before the time its picture shows: an imported clip's, a
/// recording's picture source's runs and an overlay video's sound play in step with their pictures. A sound file has no
/// lead.
///
/// `registry` is the plan context's (`PlanContext::registry`): it maps each audio effect to its filter chain and pack
/// paths to files.
///
/// `speech` is where someone speaks on `p`'s timeline (`ducking_speech`, worked out once per revision or export job):
/// a sound layer that ducks is lowered under it. Empty, nothing ducks.
pub fn audio_plan(p: &Project, registry: &Registry, speech: Arc<[RangeUs]>) -> AudioPlan {
    let mut sources = Vec::new();
    let main = &p.timeline.main;
    for (i, (clip, start)) in main.iter().zip(clip_starts(main)).enumerate() {
        if clip.muted || clip.volume <= 0.0 {
            continue;
        }
        // A transition crossfades the two clips' sound over its window (equal power; it replaces the fades).
        let incoming = i
            .checked_sub(1)
            .and_then(|k| main[k].transition_out.as_ref())
            .map(|t| split(t.duration_us));
        let outgoing = main
            .get(i + 1)
            .and(clip.transition_out.as_ref())
            .map(|t| split(t.duration_us));
        let (pre, fade_in) = incoming.map_or((0, clip.fade_in_us), |(before, after)| {
            (before, before + after)
        });
        let (post, fade_out) = outgoing.map_or((0, clip.fade_out_us), |(before, after)| {
            (after, before + after)
        });
        let end = start + clip.duration_us();
        // One source per stretch of a file inside the clip: the whole clip for an imported file, else each run of a
        // recording's sound sources that the clip plays, at its drift-corrected rate. Only a stretch at the clip's edge
        // takes its fade or crossfade; its pre-roll into a transition is that much of the file's own time.
        let (first, mut fills) = (sources.len(), Vec::new()); // the camera's fills, from the clip's first source
        let mut push = |asset: &Asset, span: RunInClip, gain: f64, rate: f64, filter, centre| {
            let (at_start, at_end) = (span.start_us == start, span.end_us == end);
            sources.push(AudioSource {
                path: registry.resolve_path(&asset.path).into_owned(),
                start_us: if at_start { start - pre } else { span.start_us },
                end_us: if at_end { end + post } else { span.end_us },
                source_in_us: span.run_in_us.saturating_sub(asset.lead_us())
                    - if at_start {
                        (unscaled(pre, clip.speed) as f64 * rate).round() as Us
                    } else {
                        0
                    },
                speed: clip.speed,
                rate,
                gain: clip.volume * gain,
                fade_in_us: if at_start { fade_in } else { 0 },
                fade_out_us: if at_end { fade_out } else { 0 },
                crossfade: Crossfade {
                    fade_in: at_start && incoming.is_some(),
                    fade_out: at_end && outgoing.is_some(),
                },
                filter,
                centre_from: centre,
                duck: None,
                seam: Seam::default(),
            });
            sources.len() - 1
        };
        match &clip.source {
            ClipSource::Asset { id } => {
                if let Some(asset) = audible(p, id) {
                    let whole = RunInClip {
                        start_us: start,
                        end_us: end,
                        run_in_us: clip.source_in_us,
                    };
                    push(asset, whole, 1.0, 1.0, None, None);
                }
            }
            ClipSource::Recording { id } => {
                let Some(rec) = p.recording(id) else { continue };
                // Its sound sources, and a picture source whose files carry sound (an imported camera's own
                // sound; Klipolo's picture files have none, and `audible` skips a file without sound). A muted camera
                // sound that fills gaps plays over the clip's true gaps only.
                let mut gaps = fill::Gaps::of(rec, clip);
                for source in sound_sources(p, rec) {
                    let mix = clip.mix_for(source);
                    let within = match gaps.heard(source, mix) {
                        fill::Heard::Nowhere => continue,
                        fill::Heard::Everywhere => None,
                        fill::Heard::Gaps(found) => Some(found),
                    };
                    // The source's audio effects in this clip (its mix's) and the channel it plays centred from, as a
                    // separated layer of it plays them.
                    let (filter, centre) =
                        (chain(registry, enabled(&mix.effects)), mix.centre_from);
                    for run in &source.runs {
                        let Some(asset) = audible(p, &run.asset_id) else {
                            continue;
                        };
                        let rate = drift_rate(run.drift_ppm);
                        let whole = within
                            .is_none()
                            .then(|| run_in_clip(clip, start, source, run));
                        let gaps_in = within.unwrap_or_default().iter();
                        let filled = gaps_in.map(|&gap| run_within(clip, start, source, run, gap));
                        for span in whole.into_iter().chain(filled).flatten() {
                            let at = push(asset, span, mix.volume, rate, filter.clone(), centre);
                            if within.is_some() {
                                fills.push(at - first);
                            }
                        }
                    }
                }
            }
        }
        fill::cross(&mut sources[first..], &fills, (start, end), clip.speed);
    }
    layers::layers(p, registry, &speech, &mut sources);
    AudioPlan {
        sources,
        loudness: p.audio.loudness,
    }
}
