//! Who speaks, as a person: each voice of a file's transcript matched to one of the people the tracker found, so the
//! crop can keep whoever is speaking (`path::Talk`).
//!
//! - **Only where it matters:** shots with two or more people (`path::people`) who don't all fit the crop. In them, a
//!   voice's windows (`windows`) are where it talks alone for at least `WINDOW_MIN_US`, at most `WINDOW_US` of each
//!   from its middle, the longest `MAX_WINDOWS`.
//! - **Voices** are a transcription request's own (`Voice`: its part and label), so a long file sent in parts keeps
//!   each part's "A" apart.
//! - **The question** (`VoiceMatcher`): a window copied small with its sound (`SubjectCopy::copy_with_sound`) and the
//!   people on screen, numbered from the left with their boxes; the answer is a person, nobody on screen, or unclear.
//! - **The vote** (`vote`): what most of a voice's windows said; a tie says nothing. Windows are asked in turn and
//!   stop once two agree.
//! - **The price** comes first: `uncached_usd` for windows known (the tracking is cached), `most_usd` before anything
//!   is tracked (every voice's windows at the longest). What the cache holds is free.
//! - **The cache:** each answer, unclear ones too (they were paid for), is kept in the file's analysis cache folder
//!   (`voices.json`) by model, window and how many people were asked about: never asked again.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use klipolo_model::fill_focus::Voice;
use klipolo_model::time::{RangeUs, Us};
use serde::{Deserialize, Serialize};

use super::path::{Talk, fit_together, people, shots};
use super::{Copy, FPS, SubjectCopy, unique_name};
use crate::cache::Dir;
use crate::error::{AnalysisError, VideoError};
use crate::providers::{Heard, ScreenBox, TrackedBox, TrackedFrame, VoiceMatcher};

/// A voice must talk alone at least this long for a window.
pub const WINDOW_MIN_US: Us = 2_000_000;
/// The most of a stretch a window holds, from its middle.
pub const WINDOW_US: Us = 3_000_000;
/// The most windows asked about for one voice.
pub const MAX_WINDOWS: usize = 3;
/// The cache file of a file's answers.
const CACHE_NAME: &str = "voices";

/// One voice's turn, on the file's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceTurn {
    pub start_us: Us,
    pub end_us: Us,
    pub voice: Voice,
}

/// A stretch of a file to ask about: `voice` alone, on the file's clock, and the people on screen left to right.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub voice: Voice,
    pub range: RangeUs,
    pub people: Vec<ScreenBox>,
}

/// What a file's voices were matched to: each voice the vote decided, what asking cost now, and why it stopped early
/// (a copy or a call that failed: the voices after it are left unmatched).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Matched {
    pub voices: BTreeMap<Voice, Heard>,
    pub paid_usd: f64,
    pub stopped: Option<String>,
}

/// One answer, as the cache keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Asked {
    model: String,
    range: RangeUs,
    people: usize,
    heard: Heard,
    cost_usd: f64,
}

/// Where a file's windows are copied from, cached and made.
pub struct Asking<'a> {
    pub src: &'a Path,
    /// Its analysis cache folder; None asks without one.
    pub cache: Option<&'a Dir>,
    /// Where the copies are made (each removed once sent).
    pub work: &'a Path,
    pub cancel: Arc<AtomicBool>,
}

/// The windows to ask about in `range` of a file (its clock), from its tracked `frames` and `cuts` there and its
/// voices' `turns`, for a crop `crop_w` wide: for each voice, at most `MAX_WINDOWS`, longest first.
pub fn windows(
    frames: &[(Us, &TrackedFrame)],
    cuts: &[Us],
    range: RangeUs,
    turns: &[VoiceTurn],
    crop_w: f64,
) -> Vec<Window> {
    // The shots whose people don't all fit.
    let mut contested: Vec<Contested> = Vec::new();
    for (from, to) in shots(range.start, range.end, cuts) {
        let in_shot: Vec<(Us, TrackedFrame)> = (frames.iter())
            .filter(|(t, _)| (from..to).contains(t))
            .map(|(t, f)| (*t, (*f).clone()))
            .collect();
        let plain: Vec<TrackedFrame> = in_shot.iter().map(|(_, f)| f.clone()).collect();
        let ids = people(&plain);
        if ids.len() >= 2 && !fit_together(&plain, &ids, crop_w) {
            contested.push((from, to, ids, in_shot));
        }
    }
    let mut by_voice: BTreeMap<Voice, Vec<(RangeUs, usize)>> = BTreeMap::new();
    for (voice, alone) in alone(turns) {
        for (i, (from, to, _, _)) in contested.iter().enumerate() {
            let (a, b) = (alone.start.max(*from), alone.end.min(*to));
            if b - a >= WINDOW_MIN_US {
                (by_voice.entry(voice.clone()).or_default())
                    .push((RangeUs { start: a, end: b }, i));
            }
        }
    }
    let mut out = Vec::new();
    for (voice, mut stretches) in by_voice {
        stretches.sort_by_key(|(r, _)| (std::cmp::Reverse(r.end - r.start), r.start));
        for (r, shot) in stretches.into_iter().take(MAX_WINDOWS) {
            let len = (r.end - r.start).min(WINDOW_US);
            let start = r.start + (r.end - r.start - len) / 2;
            let range = RangeUs {
                start,
                end: start + len,
            };
            let (_, _, ids, in_shot) = &contested[shot];
            let people: Option<Vec<ScreenBox>> = (ids.iter())
                .map(|id| mean_box(in_shot, *id, range))
                .collect();
            if let Some(people) = people {
                out.push(Window {
                    voice: voice.clone(),
                    range,
                    people,
                });
            }
        }
    }
    out
}

/// A shot whose people don't all fit: from, to, their ids left to right, and its frames.
type Contested = (Us, Us, Vec<u32>, Vec<(Us, TrackedFrame)>);

/// Each voice's stretches with no other voice talking, in order.
fn alone(turns: &[VoiceTurn]) -> Vec<(Voice, RangeUs)> {
    let mut out = Vec::new();
    for t in turns.iter().filter(|t| t.end_us > t.start_us) {
        let mut others: Vec<(Us, Us)> = (turns.iter())
            .filter(|o| o.voice != t.voice && o.start_us < t.end_us && t.start_us < o.end_us)
            .map(|o| (o.start_us, o.end_us))
            .collect();
        others.sort_unstable();
        let mut at = t.start_us;
        for (a, b) in others {
            if a > at {
                out.push((t.voice.clone(), RangeUs { start: at, end: a }));
            }
            at = at.max(b);
        }
        if at < t.end_us {
            out.push((
                t.voice.clone(),
                RangeUs {
                    start: at,
                    end: t.end_us,
                },
            ));
        }
    }
    out
}

/// `id`'s average box over the frames in `range` (else over all of `frames`).
fn mean_box(frames: &[(Us, TrackedFrame)], id: u32, range: RangeUs) -> Option<ScreenBox> {
    let boxes = |within: bool| -> Vec<TrackedBox> {
        (frames.iter())
            .filter(|(t, _)| !within || (range.start..range.end).contains(t))
            .flat_map(|(_, f)| f.boxes.iter().filter(|b| b.id == id).copied())
            .collect()
    };
    let mut found = boxes(true);
    if found.is_empty() {
        found = boxes(false);
    }
    if found.is_empty() {
        return None;
    }
    let n = found.len() as f64;
    let sum = |f: fn(&TrackedBox) -> f64| found.iter().map(f).sum::<f64>() / n;
    Some(ScreenBox {
        left: sum(|b| b.left),
        top: sum(|b| b.top),
        right: sum(|b| b.right),
        bottom: sum(|b| b.bottom),
    })
}

/// What most of `heard` said (a person, or nobody on screen); None for a tie or nothing said. Unclear answers don't
/// count.
pub fn vote(heard: &[Heard]) -> Option<Heard> {
    let mut counts: Vec<(Heard, usize)> = Vec::new();
    for h in heard.iter().filter(|h| **h != Heard::Unclear) {
        match counts.iter_mut().find(|(c, _)| c == h) {
            Some((_, n)) => *n += 1,
            None => counts.push((*h, 1)),
        }
    }
    let top = counts.iter().map(|(_, n)| *n).max()?;
    let mut tops = counts.iter().filter(|(_, n)| *n == top);
    let first = tops.next()?.0;
    tops.next().is_none().then_some(first)
}

/// Whether `heard` settles a voice whatever its other windows say: an answer two of them gave.
fn settled(heard: &[Heard]) -> bool {
    (heard.iter())
        .filter(|h| **h != Heard::Unclear)
        .any(|h| heard.iter().filter(|o| *o == h).count() >= 2)
}

/// The cached answer for `window`, if `model` gave one.
fn cached(kept: &[Asked], model: &str, window: &Window) -> Option<Heard> {
    (kept.iter())
        .find(|a| a.model == model && a.range == window.range && a.people == window.people.len())
        .map(|a| a.heard)
}

/// The windows still to ask about, by voice in the order given: none for a voice its cached answers settle.
fn to_ask<'w>(kept: &[Asked], model: &str, windows: &'w [Window]) -> Vec<&'w Window> {
    let mut voices: Vec<&Voice> = windows.iter().map(|w| &w.voice).collect();
    voices.dedup();
    let mut out = Vec::new();
    for voice in voices {
        let own: Vec<&Window> = windows.iter().filter(|w| &w.voice == voice).collect();
        let heard: Vec<Heard> = own.iter().filter_map(|w| cached(kept, model, w)).collect();
        if !settled(&heard) {
            out.extend(own.into_iter().filter(|w| cached(kept, model, w).is_none()));
        }
    }
    out
}

/// What asking about `windows` may cost now: each window the cache doesn't hold, of voices it doesn't settle.
/// Blocking (a small read).
pub fn uncached_usd(matcher: &dyn VoiceMatcher, cache: Option<&Dir>, windows: &[Window]) -> f64 {
    let kept: Vec<Asked> = cache.and_then(|d| d.load(CACHE_NAME)).unwrap_or_default();
    (to_ask(&kept, matcher.model(), windows).iter())
        .map(|w| matcher.estimate_usd((w.range.end - w.range.start) as f64 / 1e6))
        .sum()
}

/// The most asking about the voices of `turns` in `range` can cost, before anything is tracked: `MAX_WINDOWS` (or as
/// many stretches alone as a voice has) of `WINDOW_US` for each voice.
pub fn most_usd(matcher: &dyn VoiceMatcher, turns: &[VoiceTurn], range: RangeUs) -> f64 {
    let mut count: BTreeMap<Voice, usize> = BTreeMap::new();
    for (voice, r) in alone(turns) {
        let (a, b) = (r.start.max(range.start), r.end.min(range.end));
        if b - a >= WINDOW_MIN_US {
            *count.entry(voice).or_default() += 1;
        }
    }
    let one = matcher.estimate_usd(WINDOW_US as f64 / 1e6);
    count
        .values()
        .map(|n| (*n).min(MAX_WINDOWS) as f64 * one)
        .sum()
}

/// Each voice of `windows` matched (see the module notes): cached answers read, the rest asked in turn until two
/// agree, each answer cached as it comes. The first failure stops it (`Matched::stopped`); what was asked before
/// stays cached and counted.
pub async fn match_voices(
    matcher: &dyn VoiceMatcher,
    copier: Arc<dyn SubjectCopy>,
    asking: &Asking<'_>,
    windows: &[Window],
) -> Matched {
    let model = matcher.model().to_owned();
    let mut kept: Vec<Asked> = match asking.cache {
        Some(dir) => {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || dir.load(CACHE_NAME))
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    let mut voices: Vec<&Voice> = windows.iter().map(|w| &w.voice).collect();
    voices.dedup();
    let mut matched = Matched::default();
    for voice in voices {
        let mut heard = Vec::new();
        for window in windows.iter().filter(|w| &w.voice == voice) {
            if settled(&heard) {
                break;
            }
            if let Some(h) = cached(&kept, &model, window) {
                heard.push(h);
                continue;
            }
            let answer = match ask(matcher, copier.clone(), asking, window).await {
                Ok(answer) => answer,
                Err(e) => {
                    matched.stopped = Some(format!("{}: {e}", e.user_message()));
                    return matched;
                }
            };
            matched.paid_usd += answer.cost_usd;
            heard.push(answer.heard);
            kept.push(Asked {
                model: model.clone(),
                range: window.range,
                people: window.people.len(),
                heard: answer.heard,
                cost_usd: answer.cost_usd,
            });
            if let Some(dir) = asking.cache {
                let (dir, all) = (dir.clone(), kept.clone());
                let _ = tokio::task::spawn_blocking(move || dir.store(CACHE_NAME, &all)).await;
            }
        }
        if let Some(h) = vote(&heard) {
            matched.voices.insert(voice.clone(), h);
        }
    }
    matched
}

/// One window copied small with its sound and asked about; the copy is removed once sent.
async fn ask(
    matcher: &dyn VoiceMatcher,
    copier: Arc<dyn SubjectCopy>,
    asking: &Asking<'_>,
    window: &Window,
) -> Result<crate::providers::VoiceAnswer, AnalysisError> {
    std::fs::create_dir_all(asking.work)
        .map_err(|e| VideoError::Failed(format!("working folder: {e}")))?;
    let copy = Copy(asking.work.join(format!("voice-{}.mp4", unique_name())));
    {
        let (src, out, range, cancel) = (
            asking.src.to_path_buf(),
            copy.0.clone(),
            window.range,
            asking.cancel.clone(),
        );
        tokio::task::spawn_blocking(move || {
            copier.copy_with_sound(&src, range, FPS, &out, &cancel)
        })
        .await
        .map_err(|e| VideoError::Failed(format!("copy: {e}")))??;
    }
    let answer = matcher.who_speaks(&copy.0, &window.people).await?;
    drop(copy);
    Ok(answer)
}

/// Who on screen speaks when, from `turns` (the file's clock) and what their voices were matched to, moved by `shift`
/// onto the clip's time, in order. A voice matched to nobody on screen, or to no one, is left out.
pub fn talk(turns: &[VoiceTurn], voices: &BTreeMap<Voice, Heard>, shift: Us) -> Vec<Talk> {
    let mut out: Vec<Talk> = (turns.iter())
        .filter(|t| t.end_us > t.start_us)
        .filter_map(|t| match voices.get(&t.voice)? {
            Heard::Person { person } => Some(Talk {
                start_us: t.start_us + shift,
                end_us: t.end_us + shift,
                person: *person,
            }),
            Heard::Nobody | Heard::Unclear => None,
        })
        .collect();
    out.sort_by_key(|t| (t.start_us, t.person));
    out
}
