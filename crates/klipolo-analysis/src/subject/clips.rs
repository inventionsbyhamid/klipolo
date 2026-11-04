//! Main clips and the speaker: which file and stretch a clip plays (`footage`), the formats whose Fill crops it
//! (`cropped_formats`), its path from a tracked stretch (`clip_path`; `speaker_path` keeps whoever speaks in frame, or
//! who `Keep` names), the windows its voices are matched in (`voice_windows`), its people (`clip_people`) and the op
//! that makes it follow (`follow_op`, `follow_op_keeping`); or, where its only people are a presenter's corner inset
//! (`in_inset`), the op that shows its footage whole instead (`inset_op`).
//!
//! An imported video plays its file in its own time. A camera take (its camera the main footage) plays its camera
//! run's file: a clip's source time is the take's, its file time the run's own (`SourceRun::local_us`), so its path is
//! moved onto the take's clock; its voice is its mic's, lined up beside the camera's file (`camera_voice`). A screen or
//! game take follows the cursor instead, and has no footage here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use klipolo_engine::plan::fill_crop;
use klipolo_engine::plan::framing::as_shown;
use klipolo_model::fill_focus::{FillFocus, FocusPath, Keep, MAX_PEOPLE, Person, Voice};
use klipolo_model::framing::effective_framing;
use klipolo_model::ids::Id;
use klipolo_model::layout::BoxN;
use klipolo_model::ops::{ClipPatch, Op};
use klipolo_model::project::{AssetKind, FormatKind, Project, SourceRun};
use klipolo_model::speech::voice_runs;
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource, FramingMode};

use super::SubjectTrack;
use super::path::{MIN_CONFIDENCE, PathInput, focus_path, people_in, shots};
use super::voices::{self, SoundBeside, VoiceTurn, Window};
use crate::providers::{Heard, TrackedBox, TrackedFrame};

/// A Fill crop at least this share of the picture on both sides crops nothing.
const WHOLE: f64 = 0.999;
/// The tracked frames read either side of a clip's footage.
const MARGIN_US: Us = 1_000_000;

/// Where a main clip's footage comes from: a video file and the stretch the clip plays of it, on the file's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Footage {
    pub asset_id: Id,
    pub path: String,
    pub range: RangeUs,
    /// The clip's source time is the file's time plus this.
    pub shift_us: Us,
}

impl Footage {
    pub fn file(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }
}

/// `clip`'s footage: an imported video's stretch, or a camera take's camera run's (the run playing at the clip's start,
/// else the first that starts inside the clip, as a camera whose first frame came a frame after the take's start;
/// cut to the clip's end). None for a screen or game take, a picture, or a take with no camera run there.
pub fn footage(p: &Project, clip: &Clip) -> Option<Footage> {
    let (asset_id, range, shift_us) = match &clip.source {
        ClipSource::Asset { id } => (
            id.clone(),
            RangeUs {
                start: clip.source_in_us,
                end: clip.source_out_us,
            },
            0,
        ),
        ClipSource::Recording { id } => {
            let rec = p.recording(id).filter(|r| r.camera_only())?;
            let source = rec.primary_source()?;
            let nudge = source.nudge_us;
            let (run, start, shift) = match source.run_at(clip.source_in_us) {
                Some((run, start)) => (run, start, clip.source_in_us - start),
                None => {
                    let within = |r: &&SourceRun| {
                        (clip.source_in_us..clip.source_out_us).contains(&r.rec_span(nudge).0)
                    };
                    let run = source.runs.iter().find(within)?;
                    let start = run.file_in_us;
                    (run, start, run.rec_of(start, nudge) - start)
                }
            };
            let (_, run_end) = run.rec_span(nudge);
            let end = run.local_us(clip.source_out_us.min(run_end), nudge);
            (run.asset_id.clone(), RangeUs { start, end }, shift)
        }
    };
    let asset = p.asset(&asset_id).filter(|a| a.kind == AssetKind::Video)?;
    (range.end > range.start).then(|| Footage {
        asset_id,
        path: asset.path.clone(),
        range,
        shift_us,
    })
}

/// The sound a camera take's camera file `asset_id` plays beside: each run of the take's voice (its mic, as the
/// captions read it: `speech::voice_runs`) where it plays beside one of the camera's runs, lined up on the take's clock.
/// None for any other file, or a camera whose own file carries the voice.
pub fn camera_voice(p: &Project, asset_id: &str) -> Vec<SoundBeside> {
    let mut out = Vec::new();
    for r in p.recordings.iter().filter(|r| r.camera_only()) {
        let Some(camera) = r.primary_source() else {
            continue;
        };
        for run in camera.runs.iter().filter(|run| run.asset_id == asset_id) {
            let (from, to) = run.rec_span(camera.nudge_us);
            let whole = Clip::new(ClipSource::Recording { id: r.id.clone() }, from, to);
            for voice in voice_runs(p, &whole) {
                let Some(file) = p.asset(&voice.asset).filter(|_| voice.asset != asset_id) else {
                    continue;
                };
                let at = |rec| run.local_us(rec, camera.nudge_us);
                let start = at(voice.rec_from);
                out.push(SoundBeside {
                    asset_id: voice.asset.clone(),
                    path: PathBuf::from(&file.path),
                    span: RangeUs {
                        start,
                        end: at(voice.rec_to),
                    },
                    lead_us: start - voice.local_us(voice.rec_from),
                });
            }
        }
    }
    out
}

/// Which of `formats` crop `clip`'s footage when it fills them: where its Fill crop is narrower or shorter than the
/// picture. A 16:9 video fills 16:9 whole, so only a vertical or square version has anywhere to follow.
pub fn cropped_formats(p: &Project, clip: &Clip, formats: &[FormatKind]) -> Vec<FormatKind> {
    formats
        .iter()
        .copied()
        .filter(|f| fill_crop(p, clip, *f).is_some_and(|c| c.w < WHOLE || c.h < WHOLE))
        .collect()
}

/// A presenter inset (a face camera's box in a corner of a screen lesson) is shorter than this share of the picture.
pub const INSET_H: f64 = 0.3;
/// Its middle is within this share of the picture of a side, and of the top or the bottom.
pub const INSET_EDGE: f64 = 0.25;

/// Whether every person tracked in `clip`'s footage is a presenter inset: each box, on every frame, under `INSET_H` of
/// the picture's height with its middle within `INSET_EDGE` of a side and of the top or bottom (in what the clip's own
/// crop keeps). A crop following them would hold the frame at the picture's edge on a small box; the whole picture
/// shows them and what they present. False with nobody tracked.
pub fn in_inset(clip: &Clip, footage: &Footage, tracks: &[SubjectTrack]) -> bool {
    let kept = kept_in_file(clip);
    let (frames, _) = file_frames(footage, tracks, kept);
    let mut boxes = (frames.iter())
        .flat_map(|(_, f)| &f.boxes)
        .filter(|b| b.confidence.is_none_or(|c| c >= MIN_CONFIDENCE))
        .peekable();
    boxes.peek().is_some()
        && boxes.all(|b| {
            let h = (b.bottom - b.top) / kept.h;
            let x = (b.centre_x() - kept.x) / kept.w;
            let y = ((b.top + b.bottom) / 2.0 - kept.y) / kept.h;
            let near = |v: f64| !(INSET_EDGE..=1.0 - INSET_EDGE).contains(&v);
            h < INSET_H && near(x) && near(y)
        })
}

/// The op that frames `clip` Fit in each of `formats` whose Fill crops it now, where it is on Fill: a presenter inset's
/// footage shown whole (`in_inset`). None when there is no such format.
pub fn inset_op(p: &Project, clip: &Clip, formats: &[FormatKind]) -> Option<Op> {
    let mut framing = clip.framing.clone();
    let mut any = false;
    for format in followable_formats(p, clip, formats) {
        let mut own = effective_framing(p, clip, format);
        own.mode = FramingMode::Fit;
        framing.insert(format, own);
        any = true;
    }
    any.then(|| Op::UpdateClip {
        id: clip.id.clone(),
        patch: ClipPatch {
            framing: Some(framing),
            ..ClipPatch::default()
        },
    })
}

/// Which of `formats` `clip` may follow its speaker in: where its Fill crops the footage and it is on Fill (its own
/// framing, else the default). A format the person set to Fit is never turned to Fill by a follow.
pub fn followable_formats(p: &Project, clip: &Clip, formats: &[FormatKind]) -> Vec<FormatKind> {
    (cropped_formats(p, clip, formats).into_iter())
        .filter(|f| effective_framing(p, clip, *f).mode == FramingMode::Fill)
        .collect()
}

/// Who speaks when and who to keep, beyond the tracked frames: the file's voice `turns` (its clock), what each voice
/// was matched to, and `keep`.
#[derive(Clone, Copy, Debug)]
pub struct Speech<'a> {
    pub turns: &'a [VoiceTurn],
    pub voices: &'a BTreeMap<Voice, Heard>,
    pub keep: &'a Keep,
}

impl Speech<'_> {
    /// Nobody heard, and whoever speaks kept.
    pub fn none() -> Speech<'static> {
        static NO_VOICES: BTreeMap<Voice, Heard> = BTreeMap::new();
        static SPEAKER: Keep = Keep::Speaker;
        Speech {
            turns: &[],
            voices: &NO_VOICES,
            keep: &SPEAKER,
        }
    }
}

/// `clip`'s speaker path in `format` from the tracked stretches of its file (`tracks`, from one or several requests,
/// in any order), over just the footage it plays, in its source time. Only people whose middle is inside what the
/// clip's crop keeps count, and the path stays inside it (`kept_in_file`). None when nobody was found there, or Fill
/// crops nothing.
pub fn clip_path(
    p: &Project,
    clip: &Clip,
    format: FormatKind,
    footage: &Footage,
    tracks: &[SubjectTrack],
) -> Option<FocusPath> {
    speaker_path(p, clip, format, footage, tracks, &Speech::none())
}

/// [`clip_path`] keeping whoever speaks in frame (`speech`), or who its keep names, where two or more people don't all
/// fit the crop.
pub fn speaker_path(
    p: &Project,
    clip: &Clip,
    format: FormatKind,
    footage: &Footage,
    tracks: &[SubjectTrack],
    speech: &Speech,
) -> Option<FocusPath> {
    let crop = path_crop(p, clip, format)?;
    let shift = footage.shift_us;
    let kept = kept_in_file(clip);
    let (frames, cuts) = file_frames(footage, tracks, kept);
    let frames: Vec<(Us, &TrackedFrame)> = frames.iter().map(|(at, f)| (at + shift, f)).collect();
    let cuts: Vec<Us> = cuts.into_iter().map(|c| c + shift).collect();
    let talk = voices::talk(speech.turns, speech.voices, shift);
    let input = PathInput {
        frames: &frames,
        start_us: footage.range.start + shift,
        end_us: footage.range.end + shift,
        cuts: &cuts,
        crop,
        talk: &talk,
        keep: speech.keep,
        area: kept,
    };
    focus_path(&input)
}

/// The tracked frames around `footage` (a second either side, so the crop eases in toward its first and last frames),
/// with only the people whose middle is inside `kept`, and its cuts, on the file's clock, in order.
fn file_frames(
    footage: &Footage,
    tracks: &[SubjectTrack],
    kept: BoxN,
) -> (Vec<(Us, TrackedFrame)>, Vec<Us>) {
    let (from, to) = (footage.range.start, footage.range.end);
    let mut frames: Vec<(Us, TrackedFrame)> = Vec::new();
    let mut cuts: Vec<Us> = Vec::new();
    for t in tracks
        .iter()
        .filter(|t| t.range.start < to && from < t.range.end)
    {
        let fps = if t.fps > 0.0 { t.fps } else { 1.0 };
        frames.extend((t.frames.iter()).filter_map(|f| {
            let at = t.range.start + (f64::from(f.frame) / fps * 1e6).round() as Us;
            (from - MARGIN_US..to + MARGIN_US)
                .contains(&at)
                .then(|| (at, kept_boxes(f, kept)))
        }));
        cuts.extend(t.cuts.iter().filter(|c| (from..to).contains(*c)));
    }
    frames.sort_by_key(|(at, _)| *at);
    frames.dedup_by_key(|(at, _)| *at);
    cuts.sort_unstable();
    cuts.dedup();
    (frames, cuts)
}

/// What `clip`'s crop keeps of its footage, where the file has it: a mirrored camera take's crop is on the picture as
/// shown, so it is flipped back, as the engine draws it (`fill_focused`). The whole picture when uncropped.
fn kept_in_file(clip: &Clip) -> BoxN {
    let mirror = matches!(clip.source, ClipSource::Recording { .. })
        && clip.camera.as_ref().is_some_and(|c| c.mirror);
    as_shown(mirror, clip.crop.unwrap_or_default().kept())
}

/// `frame` with only the boxes whose middle is inside `kept`.
fn kept_boxes(frame: &TrackedFrame, kept: BoxN) -> TrackedFrame {
    let inside = |b: &&TrackedBox| {
        let (x, y) = (b.centre_x(), (b.top + b.bottom) / 2.0);
        (kept.x..=kept.x + kept.w).contains(&x) && (kept.y..=kept.y + kept.h).contains(&y)
    };
    TrackedFrame {
        frame: frame.frame,
        boxes: frame.boxes.iter().filter(inside).copied().collect(),
    }
}

/// The share of the whole file's picture `clip`'s Fill crop shows in `format` (across, down): Fill's crop is in what
/// the clip's own crop leaves, the path in the whole file. None when Fill crops nothing there.
fn path_crop(p: &Project, clip: &Clip, format: FormatKind) -> Option<(f64, f64)> {
    let kept = kept_in_file(clip);
    let crop = fill_crop(p, clip, format)?;
    Some((crop.w * kept.w, crop.h * kept.h))
}

/// The windows `clip`'s voices are matched in (`voices::windows`), in the footage it plays, for the narrowest crop of
/// `formats` it may follow in. None where it follows in none.
pub fn voice_windows(
    p: &Project,
    clip: &Clip,
    formats: &[FormatKind],
    footage: &Footage,
    tracks: &[SubjectTrack],
    turns: &[VoiceTurn],
) -> Vec<Window> {
    let narrowest = (followable_formats(p, clip, formats).into_iter())
        .filter_map(|f| path_crop(p, clip, f))
        .map(|c| c.0)
        .min_by(f64::total_cmp);
    let Some(crop_w) = narrowest else {
        return Vec::new();
    };
    let kept = kept_in_file(clip);
    let (frames, cuts) = file_frames(footage, tracks, kept);
    let frames: Vec<(Us, &TrackedFrame)> = frames.iter().map(|(at, f)| (*at, f)).collect();
    voices::windows_in(&frames, &cuts, footage.range, turns, crop_w, kept)
}

/// The people `clip`'s crop chooses between, left to right: as many as its busiest shot has (none when that is one),
/// each with the voices matched to them and their share of the speech in the footage it plays.
pub fn clip_people(
    clip: &Clip,
    footage: &Footage,
    tracks: &[SubjectTrack],
    turns: &[VoiceTurn],
    heard: &BTreeMap<Voice, Heard>,
) -> Vec<Person> {
    let kept = kept_in_file(clip);
    let (frames, cuts) = file_frames(footage, tracks, kept);
    let range = footage.range;
    let most = (shots(range.start, range.end, &cuts).into_iter())
        .map(|(from, to)| {
            let in_shot: Vec<TrackedFrame> = (frames.iter())
                .filter(|(t, _)| (from..to).contains(t))
                .map(|(_, f)| (*f).clone())
                .collect();
            people_in(&in_shot, kept).len()
        })
        .max()
        .unwrap_or(0)
        .min(usize::from(MAX_PEOPLE));
    if most < 2 {
        return Vec::new();
    }
    let within = |t: &VoiceTurn| (t.end_us.min(range.end) - t.start_us.max(range.start)).max(0);
    let total: Us = turns.iter().map(within).sum();
    (1..=most as u8)
        .map(|n| {
            let mine: Vec<&Voice> = (heard.iter())
                .filter(|(_, h)| **h == Heard::Person { person: n })
                .map(|(v, _)| v)
                .collect();
            let said: Us = (turns.iter())
                .filter(|t| mine.contains(&&t.voice))
                .map(within)
                .sum();
            let share = if total > 0 {
                ((said as f64 / total as f64) * 100.0).round() as u8
            } else {
                0
            };
            Person {
                // A label the project can't keep (none, or too long) isn't named.
                voices: (mine.into_iter())
                    .filter(|v| {
                        v.is_valid() && turns.iter().any(|t| &t.voice == *v && within(t) > 0)
                    })
                    .cloned()
                    .collect(),
                share,
            }
        })
        .collect()
}

/// The op that makes `clip` follow its speaker in each of `paths`' formats: Fill there (the framing's own focus point
/// kept for Fixed), with the path as its focus. Other formats keep their framing and focus.
pub fn follow_op(p: &Project, clip: &Clip, paths: &BTreeMap<FormatKind, FocusPath>) -> Op {
    follow_op_keeping(p, clip, paths, &Keep::Speaker, &[])
}

/// [`follow_op`], each path made for `keep` among `people`.
pub fn follow_op_keeping(
    p: &Project,
    clip: &Clip,
    paths: &BTreeMap<FormatKind, FocusPath>,
    keep: &Keep,
    people: &[Person],
) -> Op {
    let mut framing = clip.framing.clone();
    let mut focus = clip.fill_focus.clone();
    for (format, path) in paths {
        let mut own = effective_framing(p, clip, *format);
        own.mode = FramingMode::Fill;
        framing.insert(*format, own);
        focus.insert(
            *format,
            FillFocus::Speaker {
                path: path.clone(),
                keep: keep.clone(),
                people: people.to_vec(),
            },
        );
    }
    Op::UpdateClip {
        id: clip.id.clone(),
        patch: ClipPatch {
            framing: Some(framing),
            fill_focus: Some(focus),
            ..ClipPatch::default()
        },
    }
}
