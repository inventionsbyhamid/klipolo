//! A short's stickers and sound effects, placed and timed by Klipolo on what the short shows: the model picks the
//! moments (the word a sticker is for, the payoff line, which kinds of sound), never the places or times.
//!
//! An answer plans a short when it keeps parts (`keepOnly`) of a video that has or gets a 9:16 version, and it is
//! chat's own (another app's calls are placed as it gives them). Everything is decided once, on the snapshot's
//! timeline, with the short's own times worked out from what the answer cuts (`Compiler::cut`):
//! - **The hook** is the first words the answer puts on screen (`addText`) within the short's first 3 s.
//! - **Stickers** are the pack's pictures (`addImage`, not a background) and drawn stickers (`makeSticker`), unless
//!   the person said where to put them. Each shows from `LEAD_US` before its word, for `HOLD_US`, ending before its
//!   part does (one that can't show `MIN_HOLD_US` there is left out), popping in and fading out. In the short's time,
//!   in order: none while the hook shows, none in its last `END_CLEAR_US`, at least `STICKER_GAP_US` between two, and
//!   at most one per `STICKERS_PER_US` of it. Where each goes is `clear::beside_speaker`'s, alternating sides when
//!   both have the same room. An `animate` of one is Klipolo's already.
//! - **Sounds** follow what is seen, once the answer has any sound effect (an `addSound`, or an `animate` with a
//!   sound): a pop with each sticker; a rise under the hook and a whoosh at up to `MAX_WHOOSHES` cuts where the speaker
//!   changes (by the scene log's speech items, `STICKER_GAP_US` apart and after the hook), in the file the model named
//!   for that kind or else the library's (`Cues::own`, played with the hook's words); and one chime or ding on the
//!   payoff line, where the model marks it. Any other sound, and any more of those, is left out, unless the person
//!   named it: then it goes where the model put it.
//!
//! What the model is told of each call it affects (`Compiled::told`) says what Klipolo did with it, so its reply
//! claims nothing that isn't there.

use std::collections::HashMap;

use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::query::{clip_at, clip_starts, main_track_duration};
use klipolo_model::time::{RangeUs, Us, kept_parts};
use klipolo_model::timeline::ClipSource;

use super::Compiler;
use super::clear::Side;
use super::media::At;
use crate::context::{secs, within_video};
use crate::evidence::in_cut;
use crate::library::{LibraryItem, MediaKind};
use crate::tools::ToolCall;

/// A sticker shows from this long before its word starts.
pub const LEAD_US: Us = 100_000;
/// How long a sticker shows.
pub const HOLD_US: Us = 1_500_000;
/// The least a sticker shows before its part ends; with less room it is left out.
pub const MIN_HOLD_US: Us = 1_200_000;
/// A sticker ends this long before its part does.
const PART_END_US: Us = 100_000;
/// The least time between two stickers' starts, in the short.
pub const STICKER_GAP_US: Us = 10_000_000;
/// At most one sticker per this much of the short.
pub const STICKERS_PER_US: Us = 12_000_000;
/// No sticker shows in the short's last this long.
pub const END_CLEAR_US: Us = 1_500_000;
/// The hook is words that start within this long of the short's start.
pub const HOOK_WITHIN_US: Us = 3_000_000;
/// The most whooshes in a short.
pub const MAX_WHOOSHES: usize = 2;
/// Who speaks either side of a cut is read over this much of each side.
const SIDE_US: Us = 2_000_000;

/// What a sound effect is for, by its name and tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Rise,
    Whoosh,
    Pop,
    Payoff,
    Other,
}

/// The kind of sound `item` is.
pub(crate) fn sound_kind(item: &LibraryItem) -> Kind {
    let words: Vec<String> = std::iter::once(item.name.as_str())
        .chain(item.tags.iter().map(String::as_str))
        .flat_map(|w| w.split(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let has = |names: &[&str]| words.iter().any(|w| names.contains(&w.as_str()));
    if has(&["rise", "riser", "swell"]) {
        Kind::Rise
    } else if has(&["whoosh", "swoosh", "swish", "swipe"]) {
        Kind::Whoosh
    } else if has(&["pop", "pops"]) {
        Kind::Pop
    } else if has(&["chime", "ding", "bell"]) {
        Kind::Payoff
    } else {
        Kind::Other
    }
}

/// The message's words, lower case.
fn words(text: &str) -> Vec<String> {
    (text.split(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether `message` names the sound `item`: its name, or its first tag (what it is: "whoosh", "boom"), as words of
/// their own, a plural too.
fn named(message: &[String], item: &LibraryItem) -> bool {
    let says = |name: &str| {
        let want = words(name);
        !want.is_empty()
            && message.windows(want.len()).any(|w| {
                w.iter().zip(&want).enumerate().all(|(k, (got, x))| {
                    got == x
                        || (k + 1 == want.len()
                            && (got.strip_suffix('s') == Some(x.as_str())
                                || got.strip_suffix("es") == Some(x.as_str())))
                })
            })
    };
    says(&item.name) || item.tags.first().is_some_and(|t| says(t))
}

/// Whether `message` says where to put something on screen.
fn places(message: &[String]) -> bool {
    message.iter().any(|w| {
        matches!(
            w.as_str(),
            "top"
                | "bottom"
                | "left"
                | "right"
                | "corner"
                | "corners"
                | "center"
                | "centre"
                | "middle"
        )
    })
}

/// What Klipolo does with a sticker call.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Sticker {
    /// Shown from `start` (the snapshot's timeline) for `duration`, `prefer` its side when both have the same room.
    At {
        start: Us,
        duration: Us,
        prefer: Side,
    },
    /// Left out, and why, for the model.
    Left(&'static str),
}

/// What Klipolo does with an `addSound` call: plays it at these snapshot times (none: left out), and what the model
/// is told.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cue {
    pub at: Vec<Us>,
    pub told: String,
    pub kind: Kind,
}

/// A short's plan for its stickers and sounds (see the module notes).
#[derive(Clone, Debug, Default)]
pub(crate) struct Cues {
    /// The parts the short keeps, on the snapshot's timeline, in order.
    parts: Vec<RangeUs>,
    /// The hook's call and where it shows, on the snapshot's timeline.
    pub hook: Option<(usize, RangeUs)>,
    /// By call index: each sticker call's place and time.
    pub stickers: HashMap<usize, Sticker>,
    /// By call index: each `addSound` call Klipolo times (one the person named isn't here).
    pub sounds: HashMap<usize, Cue>,
    /// The sound each sticker pops in with, when the answer has sound effects.
    pub pop: Option<Asset>,
    /// Klipolo's own sound effects where the model asked for none of their kind (a rise under the hook, the whooshes
    /// where the speaker changes): each file and its snapshot times, played with the hook's words.
    pub own: Vec<(Asset, Vec<Us>)>,
}

impl Cues {
    /// The short's plan for `calls`, or None when the answer doesn't plan a short.
    pub(super) fn plan(c: &Compiler, calls: &[ToolCall]) -> Option<Cues> {
        let short = !c.by_app
            && c.portrait
            && (calls.iter()).any(|call| matches!(call, ToolCall::KeepOnly { .. }));
        if !short {
            return None;
        }
        let duration = main_track_duration(&c.snapshot.timeline.main);
        let mut cues = Cues {
            parts: kept_parts(0, duration, &c.cut),
            ..Cues::default()
        };
        let message = words(&c.inputs.message);
        cues.hook = cues.hook_of(c, calls);
        cues.stickers = cues.place_stickers(c, calls, &message);
        let wants_sounds = (calls.iter()).any(|call| {
            matches!(call, ToolCall::AddSound { .. })
                || matches!(call, ToolCall::Animate { sound: Some(_), .. })
        });
        if wants_sounds {
            let library = &c.inputs.library;
            let sounds = library.of_kind(MediaKind::Sound, false);
            cues.pop = (sounds.iter())
                .find(|i| i.name.eq_ignore_ascii_case("pop"))
                .or_else(|| sounds.iter().find(|i| sound_kind(i) == Kind::Pop))
                .map(|i| i.asset.clone());
            cues.sounds = cues.time_sounds(c, calls, &message);
            cues.own = cues.own_sounds(c, &sounds);
        }
        Some(cues)
    }

    /// `t` on the snapshot's timeline as the short plays it: less what is cut before it.
    pub fn short_time(&self, t: Us) -> Us {
        let mut at = 0;
        for p in &self.parts {
            if t < p.start {
                break;
            }
            at += t.min(p.end) - p.start;
        }
        at
    }

    /// The short's length.
    pub fn length(&self) -> Us {
        self.parts.iter().map(RangeUs::len).sum()
    }

    /// The kept part `t` is in.
    fn part_of(&self, t: Us) -> Option<RangeUs> {
        self.parts
            .iter()
            .copied()
            .find(|p| p.start <= t && t < p.end)
    }

    /// The hook's span in the short's time: a layer over footage the short cuts ends where that footage is cut, as a
    /// cut's ripple shortens it.
    fn hook_span(&self) -> Option<RangeUs> {
        self.hook.map(|(_, r)| RangeUs {
            start: self.short_time(r.start),
            end: self.short_time(r.end),
        })
    }

    /// The first words the answer puts on screen within the short's first `HOOK_WITHIN_US`, and where they show.
    fn hook_of(&self, c: &Compiler, calls: &[ToolCall]) -> Option<(usize, RangeUs)> {
        (calls.iter().enumerate())
            .filter_map(|(i, call)| {
                let ToolCall::AddText {
                    at_us,
                    at_words,
                    duration_us,
                    ..
                } = call
                else {
                    return None;
                };
                let start = moment(c, "addText", At::of(at_us, at_words.as_deref()))?;
                let len = duration_us.unwrap_or(klipolo_model::media::text::TEXT_US);
                (self.short_time(start) < HOOK_WITHIN_US).then_some((
                    i,
                    RangeUs {
                        start,
                        end: start + len,
                    },
                ))
            })
            .min_by_key(|(_, r)| r.start)
    }

    /// Each sticker call's place and time (see the module notes); a call whose moment stands nowhere isn't here, so it
    /// is refused as before.
    fn place_stickers(
        &self,
        c: &Compiler,
        calls: &[ToolCall],
        message: &[String],
    ) -> HashMap<usize, Sticker> {
        let mut out = HashMap::new();
        if places(message) {
            return out;
        }
        let mut found: Vec<(usize, Us, Us)> = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            let (tool, at) = match call {
                ToolCall::AddImage {
                    file,
                    at_us,
                    at_words,
                    ..
                } => {
                    let Ok(item) = super::media::pick(c, "addImage", file, &[MediaKind::Image])
                    else {
                        continue;
                    };
                    let background = item.tags.iter().any(|t| t == "background");
                    if !item.pack || background || item.asset.kind != AssetKind::Image {
                        continue;
                    }
                    ("addImage", At::of(at_us, at_words.as_deref()))
                }
                ToolCall::MakeSticker {
                    at_us, at_words, ..
                } => ("makeSticker", At::of(at_us, at_words.as_deref())),
                _ => continue,
            };
            let Some(word) = moment(c, tool, at) else {
                continue;
            };
            let Some(part) = self.part_of(word) else {
                continue;
            };
            let start = word.saturating_sub(LEAD_US).max(part.start);
            let room = (part.end - PART_END_US).saturating_sub(start);
            if room < MIN_HOLD_US {
                out.insert(i, Sticker::Left(NEAR_CUT));
                continue;
            }
            found.push((i, start, HOLD_US.min(room)));
        }
        found.sort_by_key(|&(i, start, _)| (start, i));
        let length = self.length();
        let most = (length / STICKERS_PER_US).max(1) as usize;
        let hook_end = self.hook_span().map_or(0, |h| h.end);
        let mut last: Option<Us> = None;
        let mut side = Side::Right;
        let mut kept = 0;
        for (i, start, duration) in found {
            let at = self.short_time(start);
            let left = if at < hook_end {
                Some(UNDER_HOOK)
            } else if at + duration > length.saturating_sub(END_CLEAR_US) {
                Some(AT_END)
            } else if last.is_some_and(|l| at < l + STICKER_GAP_US) {
                Some(TOO_CLOSE)
            } else if kept >= most {
                Some(TOO_MANY)
            } else {
                None
            };
            if let Some(why) = left {
                out.insert(i, Sticker::Left(why));
                continue;
            }
            out.insert(
                i,
                Sticker::At {
                    start,
                    duration,
                    prefer: side,
                },
            );
            side = side.other();
            last = Some(at);
            kept += 1;
        }
        out
    }

    /// A rise under the hook and the whooshes where the speaker changes, when no `addSound` call of the model's plays
    /// that kind (`sounds`), from the library (its sound of that name, else the first of that kind). Only with a hook,
    /// whose words they come with.
    fn own_sounds(&self, c: &Compiler, library: &[&LibraryItem]) -> Vec<(Asset, Vec<Us>)> {
        let Some((_, hook)) = self.hook else {
            return Vec::new();
        };
        let asked =
            |kind: Kind| (self.sounds.values()).any(|cue| cue.kind == kind && !cue.at.is_empty());
        let file = |name: &str, kind: Kind| {
            (library.iter())
                .find(|i| i.name.eq_ignore_ascii_case(name))
                .or_else(|| library.iter().find(|i| sound_kind(i) == kind))
                .map(|i| i.asset.clone())
        };
        let mut out = Vec::new();
        if !asked(Kind::Rise)
            && let Some(rise) = file("Rise", Kind::Rise)
        {
            out.push((rise, vec![hook.start]));
        }
        let cuts = self.whoosh_cuts(c);
        if !asked(Kind::Whoosh)
            && !cuts.is_empty()
            && let Some(whoosh) = file("Whoosh", Kind::Whoosh)
        {
            out.push((whoosh, cuts));
        }
        out
    }

    /// Each `addSound` call's times (see the module notes).
    fn time_sounds(
        &self,
        c: &Compiler,
        calls: &[ToolCall],
        message: &[String],
    ) -> HashMap<usize, Cue> {
        let mut out = HashMap::new();
        let mut done: Vec<Kind> = Vec::new();
        let playhead = within_video(c.snapshot, &c.inputs.view).0;
        for (i, call) in calls.iter().enumerate() {
            let ToolCall::AddSound {
                name,
                at_us,
                at_words,
            } = call
            else {
                continue;
            };
            let Ok(item) = super::media::pick(c, "addSound", name, &[MediaKind::Sound]) else {
                continue;
            };
            if named(message, item) {
                continue;
            }
            let kind = sound_kind(item);
            let again = done.contains(&kind);
            let left = |told: &str| Cue {
                at: Vec::new(),
                told: told.to_owned(),
                kind,
            };
            let cue = match kind {
                Kind::Pop => left(POP_WITH_STICKER),
                Kind::Other => left(NOT_SEEN),
                _ if again => left(ONE_EACH),
                Kind::Rise => match self.hook {
                    Some((_, hook)) => Cue {
                        at: vec![hook.start],
                        told: RISE_DONE.to_owned(),
                        kind,
                    },
                    None => left(NO_HOOK),
                },
                Kind::Whoosh => {
                    let at = self.whoosh_cuts(c);
                    if at.is_empty() {
                        left(NO_SPEAKER_CHANGE)
                    } else {
                        let told = format!(
                            "done: Klipolo plays it at the {} where the speaker changes, at {} s in the short.",
                            if at.len() == 1 { "cut" } else { "cuts" },
                            (at.iter())
                                .map(|&t| secs(self.short_time(t)))
                                .collect::<Vec<_>>()
                                .join(" and ")
                        );
                        Cue { at, told, kind }
                    }
                }
                Kind::Payoff => {
                    // The first place the model gave: quoted words, else seconds.
                    let at = match (at_words.first(), at_us.first()) {
                        (Some(words), _) => At::Words(words, at_us.as_slice()),
                        (None, Some(&t)) => At::Sec(t),
                        (None, None) => At::Playhead,
                    };
                    let payoff = match at {
                        At::Playhead => Some(playhead),
                        _ => moment(c, "addSound", at),
                    };
                    match payoff {
                        Some(t) => Cue {
                            at: vec![t],
                            told: PAYOFF_DONE.to_owned(),
                            kind,
                        },
                        // Said nowhere: refused as before.
                        None => continue,
                    }
                }
            };
            if !cue.at.is_empty() {
                done.push(kind);
            }
            out.insert(i, cue);
        }
        out
    }

    /// Where whooshes go: the snapshot moments the short cuts to a new speaker (the scene log's speech items either
    /// side differ), at most `MAX_WHOOSHES`, after the hook, `STICKER_GAP_US` apart; each at the first frame after the
    /// cut, so all of it plays.
    fn whoosh_cuts(&self, c: &Compiler) -> Vec<Us> {
        let main = &c.snapshot.timeline.main;
        // Each cut: the side before it and the side after it.
        let mut cuts: Vec<(RangeUs, RangeUs)> = Vec::new();
        for w in self.parts.windows(2) {
            let (a, b) = (w[0], w[1]);
            cuts.push((
                RangeUs {
                    start: a.end.saturating_sub(SIDE_US).max(a.start),
                    end: a.end,
                },
                RangeUs {
                    start: b.start,
                    end: (b.start + SIDE_US).min(b.end),
                },
            ));
        }
        for s in clip_starts(main).into_iter().skip(1) {
            if let Some(p) = self.part_of(s).filter(|p| p.start < s) {
                cuts.push((
                    RangeUs {
                        start: s.saturating_sub(SIDE_US).max(p.start),
                        end: s,
                    },
                    RangeUs {
                        start: s,
                        end: (s + SIDE_US).min(p.end),
                    },
                ));
            }
        }
        cuts.sort_by_key(|(_, after)| after.start);
        let hook_end = self.hook_span().map_or(0, |h| h.end);
        let mut out: Vec<Us> = Vec::new();
        for (before, after) in cuts {
            let at = self.short_time(after.start);
            if out.len() >= MAX_WHOOSHES
                || at < hook_end
                || out
                    .last()
                    .is_some_and(|&l| at < self.short_time(l) + STICKER_GAP_US)
            {
                continue;
            }
            match (speaker(c, before), speaker(c, after)) {
                (Some(x), Some(y)) if x != y => out.push(after.start),
                _ => {}
            }
        }
        out
    }
}

/// Who speaks over `window` of the snapshot's timeline (inside one clip): the scene log's speech item that covers the
/// most of it, by who it names (no one named: a voice off screen). None without one.
fn speaker(c: &Compiler, window: RangeUs) -> Option<String> {
    let main = &c.snapshot.timeline.main;
    let (i, start) = clip_at(main, window.start)?;
    let clip = &main[i];
    let ClipSource::Asset { id } = &clip.source else {
        return None;
    };
    let log = c.inputs.scenes.get(id)?;
    let source = |t: Us| clip.source_in_us + ((t - start) as f64 * clip.speed) as Us;
    let (a, b) = (source(window.start), source(window.end));
    (log.items.iter())
        .filter(|item| item.kind.trim().eq_ignore_ascii_case("speech"))
        .map(|item| (item.end_us.min(b) - item.start_us.max(a), item))
        .filter(|(covered, _)| *covered > 0)
        .max_by_key(|(covered, item)| (*covered, item.start_us))
        .map(|(_, item)| item.who.trim().to_lowercase())
}

/// Where a call places something on the snapshot's timeline, in a part the answer keeps; None when it stands nowhere.
fn moment(c: &Compiler, tool: &str, at: At) -> Option<Us> {
    let playhead = within_video(c.snapshot, &c.inputs.view).0;
    let t = match at {
        At::Playhead => Some(playhead),
        At::Sec(t) => Some(c.evidence.place(t, &c.cut)),
        At::Edge(t) => c.evidence.moment(t),
        At::Words(words, near) => (c.said())
            .find(&c.cut, (tool, "atWords"), words, (near, playhead))
            .ok(),
    }?;
    (!in_cut(&c.cut, t)).then_some(t)
}

/// What the model is told of a sticker Klipolo shows.
pub(crate) const STICKER_DONE: &str = "done: Klipolo shows it beside the speaker from just before the word, for 1.5 s, \
     popping in and fading out.";
/// What the model is told of an `animate` of a sticker Klipolo left out.
pub(crate) const LEFT_WITH_STICKER: &str = "Left out with its sticker. Don't mention it.";
/// What the model is told of an `animate` of a sticker Klipolo places.
pub(crate) const ANIMATED_ALREADY: &str =
    "done: Klipolo pops a short's stickers in and fades them out, with their sound.";
const NEAR_CUT: &str = "Left out: its word comes too close to the end of its part for the sticker to show. Don't \
                        mention it.";
const UNDER_HOOK: &str =
    "Left out: no sticker shows while the hook is on screen. Don't mention it.";
const AT_END: &str = "Left out: no sticker shows in the short's last seconds. Don't mention it.";
const TOO_CLOSE: &str = "Left out: stickers in a short are at least 10 s apart, and another comes sooner. Don't \
                         mention it.";
const TOO_MANY: &str = "Left out: a short shows one sticker per 12 s at most. Don't mention it.";
const POP_WITH_STICKER: &str =
    "Left out: each sticker comes with its own pop, so this one isn't needed.";
const NOT_SEEN: &str = "Left out: a short's sound effects follow what is seen (a rise under the hook, a pop with each \
                        sticker, a whoosh where the speaker changes, a chime on the payoff), and the person didn't \
                        ask for this one. Don't mention it.";
const ONE_EACH: &str =
    "Left out: a short has one of these at most, and it is placed already. Don't mention it.";
const RISE_DONE: &str = "done: Klipolo plays it under the hook.";
const NO_HOOK: &str =
    "Left out: a rise plays under the hook, and this short has none. Don't mention it.";
const NO_SPEAKER_CHANGE: &str = "Left out: a whoosh plays only at a cut where the speaker changes, and this short has \
                                 none after the hook. Don't mention it.";
const PAYOFF_DONE: &str = "done: on the payoff line.";

/// What the model is told of a sticker call Klipolo placed or left out.
pub(crate) fn told_of(sticker: &Sticker) -> String {
    match sticker {
        Sticker::At { .. } => STICKER_DONE.to_owned(),
        Sticker::Left(why) => (*why).to_owned(),
    }
}

impl Cues {
    /// The snapshot's stretches from `start` for `len` that the short keeps: where a layer placed there still shows
    /// once the cut is applied (it ends at the first cut it meets).
    pub(crate) fn kept_within(&self, start: Us, len: Us) -> Vec<RangeUs> {
        let end = start + len;
        let first = self.part_of(start);
        (first.into_iter())
            .map(|p| RangeUs {
                start,
                end: p.end.min(end),
            })
            .filter(|r| !r.is_empty())
            .collect()
    }
}
