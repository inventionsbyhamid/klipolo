//! The recorded sound tools: a sound's mute and volume (`muteSource`, `setSourceVolume`), everywhere or in some clips,
//! and the sound put on a track of its own (`separateSound`).
//!
//! A sound is named by its kind ("the mic", "the computer sound"); the camera's own sound is a picture source whose
//! files carry sound. The sources acted on are those of the recordings the clips named play, else every recording that
//! has one; in each recording every source of the kind, or the one `which` names: its place among them ("2",
//! "second", as the timeline numbers "Mic 2") or its name. A take's own mic that Sync replaced with a recorder's file
//! (muted, beside a synced mic) isn't one of every mic, but `which` still reaches it.
//!
//! A mix changes from what each plays now (`Clip::mix_for`): with no clips named, the source's default, and every clip
//! of its recording with a mix of its own ("everywhere" means everywhere); with clips, only theirs. A clip's mix that
//! ends up the same as the default is dropped, so the clip follows the default again. A mute is the person's silence
//! (`SourceMix::set_muted`). Where the sound plays from a track of its own (`separateSound`), its layers are what is
//! heard: the clip's mix is left as it is, and a note says so.
//!
//! Costs: O(clips + recordings × sources) to find the sources, then O(clips + layers) per source, with a binary search
//! per clip into the source's own layers.

use std::collections::HashSet;

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::coded::{CodedError, english};
use klipolo_model::ids::Id;
use klipolo_model::messages::MESSAGES;
use klipolo_model::ops::{Op, SourcePatch};
use klipolo_model::project::{Recording, RecordingSource, SourceMix, SourceRole};
use klipolo_model::query::{clip_starts, makes_sound};
use klipolo_model::time::{Us, clock};
use klipolo_model::timeline::{ClipSource, ItemContent};

use super::recorded::{Noted, named, noted};
use super::{Compiler, Edit, Refused, label, to_model, to_person};
use crate::tools::Sound;
use crate::tools::recorded::SOUNDS;

/// Said when separating the camera's own sound, which stays with its picture.
pub const VIDEO_SOUND_STAYS: &str =
    "The camera's sound stays with its picture. You can mute it or change its volume instead.";

/// Whether `s` is a source of `sound`'s kind: its role, or for the camera's sound, a picture source whose files carry
/// sound (`query::makes_sound`).
fn is_kind(p: &Project, s: &RecordingSource, sound: Sound) -> bool {
    match sound {
        Sound::Mic => s.role == SourceRole::Mic,
        Sound::ComputerSound => s.role == SourceRole::SystemAudio,
        Sound::GameSound => s.role == SourceRole::GameAudio,
        Sound::VideoSound => s.role.is_video() && makes_sound(p, s),
    }
}

/// The kind of sound the tools name `s` by, if they name it at all (a voice chat or an app's sound they don't).
pub(crate) fn kind_of(p: &Project, s: &RecordingSource) -> Option<Sound> {
    SOUNDS.into_iter().find(|&sound| is_kind(p, s, sound))
}

/// Recording `r`'s sources of `sound`'s kind, in order.
fn of_kind<'r>(p: &Project, r: &'r Recording, sound: Sound) -> Vec<&'r RecordingSource> {
    (r.sources.iter())
        .filter(|s| is_kind(p, s, sound))
        .collect()
}

/// The source `which` names among `sources` (one recording's of `sound`'s kind): a place, 1-based ("2", "second",
/// "2nd", or "mic 2" as the context names it), or its name, a file's extension optional, any case.
fn which_one<'r>(
    sources: &[&'r RecordingSource],
    sound: Sound,
    which: &str,
) -> Option<&'r RecordingSource> {
    let w = which.trim().to_lowercase();
    let kind = format!("{} ", sound.name());
    let place = match w.strip_prefix(kind.as_str()).unwrap_or(w.as_str()) {
        "first" | "1st" => Some(1),
        "second" | "2nd" => Some(2),
        "third" | "3rd" => Some(3),
        "fourth" | "4th" => Some(4),
        other => other.parse::<usize>().ok(),
    };
    if let Some(n) = place {
        return n.checked_sub(1).and_then(|k| sources.get(k)).copied();
    }
    sources.iter().copied().find(|s| {
        let label = s.label.to_lowercase();
        let file = label.rsplit(['/', '\\']).next().unwrap_or(&label);
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        label == w || file == w || stem == w
    })
}

/// A take's own mic that Sync replaced with a recorder's file (muted, unsynced, beside a synced mic): "my mic" is the
/// recorder's then, so it isn't one of every mic. Naming it with `which` still reaches it.
fn replaced(r: &Recording, s: &RecordingSource) -> bool {
    let synced_mic = (r.sources.iter()).any(|x| x.role == SourceRole::Mic && x.synced.is_some());
    s.role == SourceRole::Mic && s.synced.is_none() && s.mix.muted && synced_mic
}

/// Recording `r`'s sources of `sound`'s kind a tool acts on, by id: every one (but a mic Sync replaced), or the one
/// `which` names. The flag is true when `which` named none of them (and there are some).
fn chosen(p: &Project, r: &Recording, sound: Sound, which: Option<&str>) -> (Vec<Id>, bool) {
    let sources = of_kind(p, r, sound);
    match which {
        None => (
            (sources.iter())
                .filter(|s| !replaced(r, s))
                .map(|s| s.id.clone())
                .collect(),
            false,
        ),
        Some(w) => match which_one(&sources, sound, w) {
            Some(s) => (vec![s.id.clone()], false),
            None => (Vec::new(), !sources.is_empty()),
        },
    }
}

/// "Which mic: 1, 2?": the sources of the kind in `recordings`, by the person's name for one they named, else by its
/// place ("1", "2"), as `which` takes them. A question for the person.
fn ask_which(p: &Project, sound: Sound, recordings: &[&str]) -> Refused {
    let mut names: Vec<String> = Vec::new();
    for r in recordings.iter().filter_map(|id| p.recording(id)) {
        for (n, s) in of_kind(p, r, sound).iter().enumerate() {
            let name = if s.label.starts_with("capture.source.") {
                (n + 1).to_string()
            } else {
                s.label.clone()
            };
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    to_person(format!("Which {}: {}?", sound.name(), names.join(", ")))
}

/// One recording's source of the kind asked for, and the clips named that play it (indexes into the working copy's
/// main track; empty when no clips were named).
struct Found {
    recording: Id,
    source: Id,
    clips: Vec<usize>,
}

/// The sources of `sound`'s kind a tool acts on: those of the recordings the clips `named` play (with those clips), or
/// with none named, of every recording that has one; in each recording every source of the kind, or the one `which`
/// names. Said to the person when there is none, or when the clips named are imported files; when `which` names none of
/// the sources there are, the person is asked which, unless each recording has one: then `which` was a misreading
/// ("the mic in the second recording"), and `tool`'s call goes back to the model.
fn find(
    work: &Project,
    tool: &str,
    sound: Sound,
    which: Option<&str>,
    named: Option<&HashSet<Id>>,
) -> Result<Vec<Found>, Refused> {
    let mut missed: Vec<&str> = Vec::new();
    let mut found: Vec<Found> = Vec::new();
    let mut file_clip = false;
    match named {
        Some(named) => {
            for (i, clip) in work.timeline.main.iter().enumerate() {
                if !named.contains(&clip.id) {
                    continue;
                }
                let ClipSource::Recording { id: rec } = &clip.source else {
                    file_clip = true;
                    continue;
                };
                let Some(r) = work.recording(rec) else {
                    continue;
                };
                let (sources, miss) = chosen(work, r, sound, which);
                if miss && !missed.contains(&r.id.as_str()) {
                    missed.push(&r.id);
                }
                for source in sources {
                    match (found.iter_mut()).find(|f| f.recording == r.id && f.source == source) {
                        Some(f) => f.clips.push(i),
                        None => found.push(Found {
                            recording: r.id.clone(),
                            source,
                            clips: vec![i],
                        }),
                    }
                }
            }
        }
        None => {
            for r in &work.recordings {
                let (sources, miss) = chosen(work, r, sound, which);
                if miss {
                    missed.push(&r.id);
                }
                found.extend(sources.into_iter().map(|source| Found {
                    recording: r.id.clone(),
                    source,
                    clips: Vec::new(),
                }));
            }
        }
    }
    if found.is_empty() {
        let name = sound.name();
        let one_each = (missed.iter())
            .filter_map(|id| work.recording(id))
            .all(|r| of_kind(work, r, sound).len() == 1);
        return Err(if !missed.is_empty() && one_each {
            to_model(format!(
                "{tool}: each recording has one {name}, so which names none. Leave which null and name that \
                 recording's clips."
            ))
        } else if !missed.is_empty() {
            ask_which(work, sound, &missed)
        } else if file_clip {
            to_person(format!(
                "That clip is an imported file, not a recording, so it has no {name}."
            ))
        } else {
            to_person(format!("Your video has no {name}."))
        });
    }
    Ok(found)
}

/// For a tool that acts on one source per recording: asks which when a recording found has several of the kind.
fn one_each(work: &Project, sound: Sound, found: &[Found]) -> Result<(), Refused> {
    let several: Vec<&str> = (work.recordings.iter())
        .filter(|r| found.iter().filter(|f| f.recording == r.id).count() > 1)
        .map(|r| r.id.as_str())
        .collect();
    if several.is_empty() {
        Ok(())
    } else {
        Err(ask_which(work, sound, &several))
    }
}

/// The source `f` found.
fn source_of<'p>(work: &'p Project, f: &Found) -> Option<&'p RecordingSource> {
    work.recording(&f.recording)?.source(&f.source)
}

/// Where a source plays from layers of its own: each video or sound layer of its files, sorted by start, each with
/// the furthest end reached so far.
pub(crate) struct OwnTrack(Vec<(Us, Us)>);

impl OwnTrack {
    /// O(layers + own layers × log).
    pub(crate) fn of(p: &Project, source: &RecordingSource) -> Self {
        let files: HashSet<&str> = source.runs.iter().map(|r| r.asset_id.as_str()).collect();
        let mut spans: Vec<(Us, Us)> = (p.timeline.tracks.iter())
            .flat_map(|t| &t.items)
            .filter(|i| match &i.content {
                ItemContent::Audio { asset_id, .. } | ItemContent::Video { asset_id, .. } => {
                    files.contains(asset_id.as_str())
                }
                _ => false,
            })
            .map(|i| (i.start_us, i.end_us()))
            .collect();
        spans.sort_unstable();
        for k in 1..spans.len() {
            spans[k].1 = spans[k].1.max(spans[k - 1].1);
        }
        Self(spans)
    }

    /// Whether one of the layers plays somewhere in `[start, end)`: O(log layers).
    pub(crate) fn over(&self, start: Us, end: Us) -> bool {
        let before_end = self.0.partition_point(|s| s.0 < end);
        (before_end.checked_sub(1)).is_some_and(|k| self.0[k].1 > start)
    }
}

/// Each found sound's mix changed by `change`: with no clips named, the source's default (first, unless it stays as
/// it is) and each clip of its recording with a mix of its own; with clips, only theirs. Ops only for what changes;
/// the flag is true when a clip was left as it is because the sound plays from its own track there.
fn change_mix(work: &Project, found: &[Found], change: impl Fn(&mut SourceMix)) -> (Vec<Op>, bool) {
    let main = &work.timeline.main;
    let starts = clip_starts(main);
    let mut ops = Vec::new();
    let mut own_track = false;
    for f in found {
        let Some(source) = source_of(work, f) else {
            continue;
        };
        let layers = OwnTrack::of(work, source);
        let mut default = source.mix.clone();
        let everywhere = f.clips.is_empty();
        let clips: Vec<usize> = if everywhere {
            change(&mut default);
            let own = |i: &usize| {
                let x = &main[*i];
                matches!(&x.source, ClipSource::Recording { id } if *id == f.recording)
                    && x.source_mix.contains_key(&f.source)
            };
            (0..main.len()).filter(own).collect()
        } else {
            f.clips.clone()
        };
        if everywhere && default != source.mix {
            ops.push(Op::UpdateRecordingSource {
                recording_id: f.recording.clone(),
                source_id: f.source.clone(),
                patch: SourcePatch {
                    mix: Some(default.clone()),
                    ..SourcePatch::default()
                },
            });
        }
        for i in clips {
            let clip = &main[i];
            if layers.over(starts[i], starts[i].saturating_add(clip.duration_us())) {
                own_track = true;
                continue;
            }
            let mut mix = clip.mix_for(source).clone();
            change(&mut mix);
            let mix = (mix != default).then_some(mix);
            if clip.source_mix.get(&f.source) != mix.as_ref() {
                ops.push(Op::SetClipSourceMix {
                    clip_id: clip.id.clone(),
                    source_id: f.source.clone(),
                    mix,
                });
            }
        }
    }
    (ops, own_track)
}

/// `change_mix` of `sound` as a call `tool` asked, with its label's values.
fn mixed(
    c: &Compiler,
    work: &Project,
    (tool, values): (&str, Vec<(&str, ErrorParam)>),
    (sound, which, clips): (Sound, Option<&str>, Option<&[String]>),
    change: impl Fn(&mut SourceMix),
) -> Result<Noted, Refused> {
    let named = named(c, work, tool, clips)?;
    let found = find(work, tool, sound, which, named.as_ref())?;
    let (ops, own_track) = change_mix(work, &found, change);
    let notes = if own_track {
        vec![format!(
            "Where the {} is on its own track, its layers stay as they are.",
            sound.name()
        )]
    } else {
        Vec::new()
    };
    let mut params = vec![("source", ErrorParam::from(sound.key()))];
    params.extend(values);
    noted(
        Edit {
            ops,
            label: label(tool, params),
        },
        notes,
    )
}

pub(super) fn mute(
    c: &Compiler,
    work: &Project,
    sound: Sound,
    muted: bool,
    (which, clips): (Option<&str>, Option<&[String]>),
) -> Result<Noted, Refused> {
    let mute = ErrorParam::from(if muted { "on" } else { "off" });
    // A mute means silence: muting the camera's own sound also stops it filling the gaps.
    mixed(
        c,
        work,
        ("muteSource", vec![("mute", mute)]),
        (sound, which, clips),
        |mix| mix.set_muted(muted),
    )
}

pub(super) fn set_volume(
    c: &Compiler,
    work: &Project,
    sound: Sound,
    volume: f64,
    (which, clips): (Option<&str>, Option<&[String]>),
) -> Result<Noted, Refused> {
    let percent = ErrorParam::Number((volume * 100.0).round());
    mixed(
        c,
        work,
        ("setSourceVolume", vec![("percent", percent)]),
        (sound, which, clips),
        |mix| mix.volume = volume,
    )
}

/// One `DetachSource` per clip that plays the sound: the clips named, or every clip of the recordings with it. A clip
/// where it already has its own track is left as it is (a note says so, or the refusal when that is every clip), and so
/// are a sped-up clip (separated sound plays at normal speed only; a note says where) and one where the source recorded
/// nothing. A recording with several of the kind (two mics) asks which, unless `which` says; the camera's own sound
/// stays with its picture.
pub(super) fn separate(
    c: &Compiler,
    work: &Project,
    sound: Sound,
    (which, clips): (Option<&str>, Option<&[String]>),
) -> Result<Noted, Refused> {
    const TOOL: &str = "separateSound";
    if sound == Sound::VideoSound {
        return Err(to_person(VIDEO_SOUND_STAYS.to_owned()));
    }
    let named = named(c, work, TOOL, clips)?;
    let found = find(work, TOOL, sound, which, named.as_ref())?;
    if which.is_none() {
        one_each(work, sound, &found)?;
    }
    let main = &work.timeline.main;
    let starts = clip_starts(main);
    let (mut ops, mut already, mut sped_up, mut any) = (Vec::new(), false, Vec::new(), false);
    for f in &found {
        let Some(source) = source_of(work, f) else {
            continue;
        };
        let layers = OwnTrack::of(work, source);
        let clips: Vec<usize> = if f.clips.is_empty() {
            (0..main.len())
                .filter(|&i| matches!(&main[i].source, ClipSource::Recording { id } if *id == f.recording))
                .collect()
        } else {
            f.clips.clone()
        };
        for i in clips {
            any = true;
            let clip = &main[i];
            if layers.over(starts[i], starts[i].saturating_add(clip.duration_us())) {
                already = true;
                continue;
            }
            let recorded = source.runs.iter().any(|r| {
                let (from, to) = r.rec_span(source.nudge_us);
                from < clip.source_out_us && to > clip.source_in_us
            });
            if !recorded {
                continue;
            }
            if clip.speed != 1.0 {
                sped_up.push(starts[i]);
                continue;
            }
            ops.push(Op::DetachSource {
                clip_id: clip.id.clone(),
                source_id: f.source.clone(),
            });
        }
    }
    let name = sound.name();
    if ops.is_empty() {
        return Err(to_person(if !sped_up.is_empty() {
            // The model's own sentence: separated sound plays at normal speed only.
            english(&[MESSAGES], &CodedError::new("model.source.detachSpeed"))
        } else if already {
            format!("The {name} is already on its own track there.")
        } else if !any {
            format!("No clip in your video plays the {name}.")
        } else {
            format!("The {name} recorded nothing during that clip.")
        }));
    }
    let mut notes = Vec::new();
    if already {
        notes.push(format!("The {name} is already on its own track there."));
    }
    match sped_up[..] {
        [] => {}
        [at] => notes.push(format!(
            "The sped-up clip at {} was left as it is: separated sound plays at normal speed only.",
            clock(at)
        )),
        _ => notes.push(format!(
            "{} sped-up clips were left as they are ({}): separated sound plays at normal speed only.",
            sped_up.len(),
            clocks(&sped_up)
        )),
    }
    noted(
        Edit {
            ops,
            label: label(TOOL, vec![("source", ErrorParam::from(sound.key()))]),
        },
        notes,
    )
}

/// Where clips start, as people read times ("1:20, 3:05"): the first five, then how many more.
fn clocks(starts: &[Us]) -> String {
    let mut text = (starts.iter().take(5))
        .map(|&at| clock(at))
        .collect::<Vec<_>>()
        .join(", ");
    if starts.len() > 5 {
        text.push_str(&format!(" and {} more", starts.len() - 5));
    }
    text
}
