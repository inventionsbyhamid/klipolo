//! A short's stickers and sound effects, placed and timed by Klipolo on what the short shows: the model picks the
//! moments (the word a sticker is for, the payoff line, which kinds of sound), never the places or times.
//!
//! An answer plans a short when it is chat's own (another app's calls are placed as it gives them) and the person asks
//! for a short (a reel, a TikTok), or the answer both cuts (`keepOnly`) and makes a 9:16 version itself. Everything is decided once, on the snapshot's
//! timeline, with the short's own times worked out from what the answer cuts (`Compiler::cut`):
//! - **The hook** is the first words the answer puts on screen (`addText`) within the short's first 3 s, held at most
//!   `HOOK_MAX_US`.
//! - **Stickers** are the pack's pictures (`addImage`, not a background), the person's own with no place given, and
//!   drawn stickers (`makeSticker`), unless the model gave a place where the person said one. Each shows from `LEAD_US` before its word, for `HOLD_US`, ending before its
//!   part does (one that can't show `MIN_HOLD_US` there is left out), popping in and fading out. In the short's time,
//!   in call order (a later round never takes out one an earlier was told is in): none while the hook shows, none in
//!   its last `END_CLEAR_US`, at least `STICKER_GAP_US` between two, and at most one per `STICKERS_PER_US` of it. One
//!   the person named stays where its word is. Where each goes is `clear::beside_speaker`'s, alternating sides when
//!   both have the same room. An `animate` of one is Klipolo's already.
//! - **Sounds** follow what is seen, once the answer has any sound effect (an `addSound`, or an `animate` with a
//!   sound): a pop with each sticker; a rise under the hook and a whoosh at up to `MAX_WHOOSHES` cuts where the speaker
//!   changes (by the transcript's speaker turns, else the scene log's speech items; `STICKER_GAP_US` apart, after the
//!   hook), in the file the model named for that kind or else the library's (`Cues::own`, each its own change); and
//!   one chime or ding on the payoff line, where the model marks it. Any other sound, and any more of those, is left
//!   out, unless the person named it: then it goes where the model put it. A short whose person asks for no sound
//!   effects gets none.
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
/// The least a sticker the person named shows, even right before a cut.
const MIN_DURATION: Us = 500_000;
/// The least time between two stickers' starts, in the short.
pub const STICKER_GAP_US: Us = 10_000_000;
/// At most one sticker per this much of the short.
pub const STICKERS_PER_US: Us = 12_000_000;
/// No sticker shows in the short's last this long.
pub const END_CLEAR_US: Us = 1_500_000;
/// The longest a short's hook shows: sound-off viewers lose the speech under it.
pub const HOOK_MAX_US: Us = 2_500_000;
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

/// Whether `message` names `item`: its name or any of its tags (what it is or suits: "whoosh", "cha ching", "funny"),
/// as words of their own, a plural too.
pub(crate) fn named(message: &[String], item: &LibraryItem) -> bool {
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
    // A file of the person's by its name with or without its extension ("my logo", "logo.png").
    if says(&item.name) || says(&crate::library::plain(&item.name)) {
        return true;
    }
    // A sticker by its name only: its tags ("like", "look", "new") are everyday words.
    item.kind != MediaKind::Image
        && (item.tags.iter())
            .filter(|t| !GENERIC.contains(&t.as_str()))
            .any(|t| says(t))
}

/// Tags too common to say the person asked for that sound ("a short with sound effects and text animations").
const GENERIC: [&str; 21] = [
    "short",
    "text",
    "video",
    "sound",
    "effect",
    "effects",
    "animation",
    "quick",
    "fast",
    "transition",
    "intro",
    "soft",
    "high",
    "deep",
    "fun",
    "button",
    "ui",
    "tech",
    "digital",
    "light",
    "dark",
];

/// Whether `message` asks for a short: "make a short", "into a short", "a 60 second short", "short video", a reel, a
/// TikTok or Shorts. Never "the short" (one already made: "is the short too long", "undo the short"), nor "short" said
/// of something else ("a short title", "short pauses").
pub(crate) fn asks_short(message: &[String]) -> bool {
    // Words that make "short" say how long something else is.
    const OF_SOMETHING: [&str; 30] = [
        "title",
        "titles",
        "text",
        "caption",
        "captions",
        "pause",
        "pauses",
        "silence",
        "silences",
        "gap",
        "gaps",
        "break",
        "breaks",
        "bit",
        "while",
        "time",
        "sentence",
        "intro",
        "outro",
        "delay",
        "fade",
        "transition",
        "sound",
        "music",
        "song",
        "line",
        "phrase",
        "word",
        "words",
        "moment",
    ];
    let duration = |w: &str| {
        matches!(
            w,
            "second" | "seconds" | "sec" | "secs" | "s" | "minute" | "min"
        ) || (!w.is_empty() && w.trim_end_matches('s').chars().all(|c| c.is_ascii_digit()))
    };
    message.iter().enumerate().any(|(k, w)| {
        let prev = k.checked_sub(1).map(|j| message[j].as_str());
        let next = message.get(k + 1).map(String::as_str);
        match w.as_str() {
            "reel" | "reels" | "tiktok" | "tiktoks" => true,
            "shorts" => !matches!(prev, Some("my" | "his" | "her" | "their" | "in")),
            "short" => {
                matches!(next, Some("video" | "videos" | "form" | "version"))
                    || (prev.is_some_and(|p| matches!(p, "a" | "an") || duration(p))
                        && !next.is_some_and(|n| OF_SOMETHING.contains(&n)))
            }
            _ => false,
        }
    })
}

/// Whether `message` asks for no sound effects: a no, without, don't, skip or not shortly before sound, sounds,
/// effects, SFX or whooshes ("without any sound effects", "don't add sound effects", "no whooshes"), or "keep it quiet".
pub(crate) fn no_sounds(message: &[String]) -> bool {
    const SOUNDS: [&str; 9] = [
        "sound", "sounds", "sfx", "effects", "effect", "whoosh", "whooshes", "pops", "noises",
    ];
    const NOT: [&str; 8] = [
        "no", "without", "dont", "skip", "not", "none", "never", "zero",
    ];
    // Words that start a new request between the "no" and the sound ("without the pauses, add sound effects").
    const NEW: [&str; 5] = ["add", "with", "and", "but", "plus"];
    let quiet = message.windows(3).any(|w| {
        w[0] == "keep"
            && matches!(w[1].as_str(), "it" | "this")
            && matches!(w[2].as_str(), "quiet" | "silent")
    });
    quiet
        || message.iter().enumerate().any(|(k, w)| {
            SOUNDS.contains(&w.as_str())
                && (1..=4)
                    .filter_map(|back| k.checked_sub(back))
                    .find_map(|j| {
                        let w = message[j].as_str();
                        // "don't" is two words here: "don" and "t".
                        let negates = |i: usize| {
                            let w = message[i].as_str();
                            NOT.contains(&w)
                                || (w == "t"
                                    && i.checked_sub(1).is_some_and(|d| message[d] == "don"))
                                || (w == "don" && message.get(i + 1).is_some_and(|n| n == "t"))
                        };
                        if negates(j) {
                            Some(true)
                        } else if NEW.contains(&w) {
                            // "don't add sound effects": the new request is itself negated.
                            Some(j.checked_sub(1).is_some_and(negates))
                        } else {
                            None
                        }
                    })
                    == Some(true)
        })
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

/// One of Klipolo's own sound effects in a short: the library's name for it, its file, its kind and its snapshot times.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Own {
    pub name: String,
    pub asset: Asset,
    pub kind: Kind,
    pub at: Vec<Us>,
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
    /// where the speaker changes): each file and its snapshot times.
    pub own: Vec<Own>,
    /// The short has sound effects: this answer makes it, and the person didn't ask for none.
    pub sound_effects: bool,
    /// This answer makes the short itself: it cuts, and makes a 9:16 version or the short's hook.
    pub makes_short: bool,
    /// The sticker calls that place a picture of the person's own: never told "Don't mention it".
    pub own_pictures: Vec<usize>,
    /// By sticker call: the sound it comes in with instead of the pop, where the model's `animate` of it names a sound
    /// the person asked for.
    pub sticker_sounds: HashMap<usize, (String, Asset)>,
}

impl Cues {
    /// The short's plan for `calls`, or None when the answer doesn't plan a short.
    pub(super) fn plan(c: &Compiler, calls: &[ToolCall]) -> Option<Cues> {
        let message = words(&c.inputs.message);
        // This answer makes a short: it cuts and makes a 9:16 version itself, or the person asks for a short. A project
        // that is vertical already, given a plain trim, isn't one.
        let cuts = (calls.iter()).any(|call| matches!(call, ToolCall::KeepOnly { .. }));
        let goes_vertical = (calls.iter()).any(|call| {
            matches!(
                call,
                ToolCall::SetFormat {
                    format: klipolo_model::project::FormatKind::Portrait,
                    ..
                }
            )
        });
        // An undo that stands takes back the change before this message: nothing of a short is made on top of it.
        let first_change = calls.iter().position(super::is_edit);
        let undo_stands = (calls.iter().position(|c| matches!(c, ToolCall::Undo)))
            .is_some_and(|u| first_change.is_none_or(|e| u < e));
        if c.by_app || undo_stands || !(asks_short(&message) || (cuts && goes_vertical)) {
            return None;
        }
        let duration = main_track_duration(&c.snapshot.timeline.main);
        let mut cues = Cues {
            parts: kept_parts(0, duration, &c.cut),
            ..Cues::default()
        };
        let hook = cues.hook_of(c, calls);
        // Klipolo's own sounds and the hook's hold only where this answer makes the short itself: it cuts, and makes a
        // 9:16 version or the short's hook. A follow-up ("is the short too long?") adds none.
        cues.makes_short = cuts && (goes_vertical || hook.is_some());
        cues.hook = hook.filter(|_| cues.makes_short);
        let (stickers, own_pictures) = cues.place_stickers(c, calls, &message);
        cues.stickers = stickers;
        cues.own_pictures = own_pictures;
        // A short's sound effects are Klipolo's unless the person wants none.
        cues.sound_effects = cues.makes_short && !no_sounds(&message);
        if !cues.makes_short {
            // Klipolo's sounds aren't this answer's: a sticker's sound is what the model's animate of it gives.
            for (a, call) in calls.iter().enumerate() {
                let ToolCall::Animate {
                    layer,
                    sound: Some(name),
                    ..
                } = call
                else {
                    continue;
                };
                let Some(j) = c.own_layer_call(calls, layer, a) else {
                    continue;
                };
                if matches!(cues.stickers.get(&j), Some(Sticker::At { .. }))
                    && let Ok(item) = super::media::pick(c, "animate", name, &[MediaKind::Sound])
                {
                    cues.sticker_sounds
                        .insert(j, (item.name.clone(), item.asset.clone()));
                }
            }
            return Some(cues);
        }
        if !cues.sound_effects {
            // None at all, but a sound the person named.
            for (i, call) in calls.iter().enumerate() {
                let ToolCall::AddSound { name, .. } = call else {
                    continue;
                };
                let named_it = super::media::pick(c, "addSound", name, &[MediaKind::Sound])
                    .is_ok_and(|item| named(&message, item));
                if !named_it {
                    let told = NO_EFFECTS.to_owned();
                    cues.sounds.insert(
                        i,
                        Cue {
                            at: Vec::new(),
                            told,
                            kind: Kind::Other,
                        },
                    );
                }
            }
        } else {
            let library = &c.inputs.library;
            let sounds = library.of_kind(MediaKind::Sound, false);
            cues.pop = (sounds.iter())
                .find(|i| i.name.eq_ignore_ascii_case("pop"))
                .or_else(|| sounds.iter().find(|i| sound_kind(i) == Kind::Pop))
                .map(|i| i.asset.clone());
            cues.sounds = cues.time_sounds(c, calls, &message);
            cues.own = cues.own_sounds(c, &sounds);
            // A sticker's sound the person asked for ("pop the heart in with a boing") replaces its pop.
            for (a, call) in calls.iter().enumerate() {
                let ToolCall::Animate {
                    layer,
                    sound: Some(name),
                    ..
                } = call
                else {
                    continue;
                };
                let Some(j) = c.own_layer_call(calls, layer, a) else {
                    continue;
                };
                if !matches!(cues.stickers.get(&j), Some(Sticker::At { .. })) {
                    continue;
                }
                if let Ok(item) = super::media::pick(c, "animate", name, &[MediaKind::Sound])
                    && named(&message, item)
                {
                    cues.sticker_sounds
                        .insert(j, (item.name.clone(), item.asset.clone()));
                }
            }
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
                let len = duration_us
                    .unwrap_or(klipolo_model::media::text::TEXT_US)
                    .min(HOOK_MAX_US);
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
    ) -> (HashMap<usize, Sticker>, Vec<usize>) {
        let mut out = HashMap::new();
        let mut own_pictures = Vec::new();
        let says_where = places(message);
        // Each sticker moment: call, start, length, and whether the person named that sticker.
        let mut found: Vec<(usize, Us, Us, bool)> = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            let (tool, at, position, asked) = match call {
                ToolCall::AddImage {
                    file,
                    at_us,
                    at_words,
                    position,
                    ..
                } => {
                    let Ok(item) = super::media::pick(c, "addImage", file, &[MediaKind::Image])
                    else {
                        continue;
                    };
                    let background = item.tags.iter().any(|t| t == "background");
                    // The person's own picture is a sticker only in the recipe's form, with no place given, and
                    // never one the message names ("my logo"): that goes where asked.
                    if background
                        || item.asset.kind != AssetKind::Image
                        || (!item.pack && (position.is_some() || named(message, item)))
                    {
                        continue;
                    }
                    if !item.pack {
                        own_pictures.push(i);
                    }
                    let at = At::of(at_us, at_words.as_deref());
                    ("addImage", at, *position, named(message, item))
                }
                ToolCall::MakeSticker {
                    at_us,
                    at_words,
                    position,
                    ..
                } => (
                    "makeSticker",
                    At::of(at_us, at_words.as_deref()),
                    *position,
                    false,
                ),
                _ => continue,
            };
            // A place the model gave, where the person said one: as asked.
            if position.is_some() && says_where {
                continue;
            }
            let Some(word) = moment(c, tool, at) else {
                continue;
            };
            let Some(part) = self.part_of(word) else {
                continue;
            };
            let start = word.saturating_sub(LEAD_US).max(part.start);
            let room = (part.end - PART_END_US).saturating_sub(start);
            if room < MIN_HOLD_US && !asked {
                out.insert(i, Sticker::Left(NEAR_CUT));
                continue;
            }
            found.push((i, start, HOLD_US.min(room.max(MIN_DURATION)), asked));
        }
        // In call order, so a later round of the message never takes out a sticker an earlier one was told is in.
        found.sort_by_key(|&(i, ..)| i);
        let length = self.length();
        let most = (length / STICKERS_PER_US).max(1) as usize;
        let hook_end = self.hook_span().map_or(0, |h| h.end);
        let mut kept: Vec<Us> = Vec::new();
        let mut side = Side::Right;
        for (i, start, duration, asked) in found {
            let at = self.short_time(start);
            let left = if at < hook_end {
                Some(UNDER_HOOK)
            } else if at + duration > length.saturating_sub(END_CLEAR_US) {
                Some(AT_END)
            } else if kept
                .iter()
                .any(|&k| at.abs_diff(k) < STICKER_GAP_US.unsigned_abs())
            {
                Some(TOO_CLOSE)
            } else if kept.len() >= most {
                Some(TOO_MANY)
            } else {
                None
            };
            // One the person named stays, where its word is, whatever the spacing.
            match left {
                Some(why) if !asked => {
                    out.insert(i, Sticker::Left(why));
                    continue;
                }
                _ => {}
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
            kept.push(at);
        }
        (out, own_pictures)
    }

    /// A rise under the hook (where there is one) and the whooshes where the speaker changes, when no `addSound` call
    /// of the model's plays that kind (`sounds`), from the library (its sound of that name, else the first of that
    /// kind). Each is its own change, added after the layers (`Compiler::own_sounds`).
    fn own_sounds(&self, c: &Compiler, library: &[&LibraryItem]) -> Vec<Own> {
        let asked =
            |kind: Kind| (self.sounds.values()).any(|cue| cue.kind == kind && !cue.at.is_empty());
        let file = |name: &str, kind: Kind, at: Vec<Us>| {
            (library.iter())
                .find(|i| i.name.eq_ignore_ascii_case(name))
                .or_else(|| library.iter().find(|i| sound_kind(i) == kind))
                .map(|i| Own {
                    name: i.name.clone(),
                    asset: i.asset.clone(),
                    kind,
                    at,
                })
        };
        let mut out = Vec::new();
        if let Some((_, hook)) = self.hook
            && !asked(Kind::Rise)
        {
            out.extend(file("Rise", Kind::Rise, vec![hook.start]));
        }
        let cuts = self.whoosh_cuts(c);
        if !asked(Kind::Whoosh) && !cuts.is_empty() {
            out.extend(file("Whoosh", Kind::Whoosh, cuts));
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
            if speaker_changes(c, before, after) {
                out.push(after.start);
            }
        }
        out
    }
}

/// Who speaks over a stretch, as one source tells it.
#[derive(Clone, Debug, PartialEq)]
enum Who {
    /// A transcript voice of file `asset`: its label ("P1" for a person on screen) and, for a bare voice label, the
    /// transcription part it holds in (`Inputs::turns` writes it as "A@3").
    Voice {
        asset: String,
        part: Option<String>,
        label: String,
    },
    /// The scene log's speech item, by who it names (no one named: a voice off screen).
    Scene(String),
}

/// Whether the speaker differs between `a` and `b`, where the two can be compared: two voices of one file in one
/// transcription part (or two people on screen), or two scene log items. None when they can't be.
fn changes(a: &Who, b: &Who) -> Option<bool> {
    match (a, b) {
        (
            Who::Voice {
                asset: f,
                part: p,
                label: x,
            },
            Who::Voice {
                asset: g,
                part: q,
                label: y,
            },
        ) => (f == g && p == q).then_some(x != y),
        (Who::Scene(x), Who::Scene(y)) => Some(x != y),
        _ => None,
    }
}

/// Who speaks over `window` of the snapshot's timeline (inside one clip, cut to it), by the transcript's speaker turns
/// (`Inputs::turns`, when `turns`) or by the scene log's speech items: what covers the most of it. A recording's clip
/// reads its file's. None when that source doesn't say.
fn speaker(c: &Compiler, window: RangeUs, turns: bool) -> Option<Who> {
    let main = &c.snapshot.timeline.main;
    let (i, start) = clip_at(main, window.start)?;
    let clip = &main[i];
    let id = match &clip.source {
        ClipSource::Asset { id } => id.clone(),
        ClipSource::Recording { id } => {
            let r = c.snapshot.recording(id)?;
            r.primary_source()?.runs.first()?.asset_id.clone()
        }
    };
    let end = window.end.min(start + clip.duration_us());
    let source = |t: Us| clip.source_in_us + ((t - start) as f64 * clip.speed) as Us;
    let (a, b) = (source(window.start), source(end));
    let covered = |from: Us, to: Us| to.min(b) - from.max(a);
    if turns {
        return (c.inputs.turns.get(&id).into_iter().flatten())
            .filter_map(|t| Some((covered(t.start_us, t.end_us), t.speaker.as_deref()?)))
            .filter(|(n, _)| *n > 0)
            .max_by_key(|(n, _)| *n)
            .map(|(_, said)| {
                let (label, part) = match said.split_once('@') {
                    Some((label, part)) => (label.to_owned(), Some(part.to_owned())),
                    None => (said.to_owned(), None),
                };
                Who::Voice {
                    asset: id.clone(),
                    part,
                    label,
                }
            });
    }
    let log = c.inputs.scenes.get(&id)?;
    (log.items.iter())
        .filter(|item| item.kind.trim().eq_ignore_ascii_case("speech"))
        .map(|item| (covered(item.start_us, item.end_us), item))
        .filter(|(n, _)| *n > 0)
        .max_by_key(|(n, item)| (*n, item.start_us))
        .map(|(_, item)| Who::Scene(item.who.trim().to_lowercase()))
}

/// Whether the speaker changes from `before` to `after`: by the transcript's turns where both sides can be compared,
/// else by the scene log's; false where neither can tell, as a missed whoosh is better than a false one.
fn speaker_changes(c: &Compiler, before: RangeUs, after: RangeUs) -> bool {
    let by = |turns: bool| match (speaker(c, before, turns), speaker(c, after, turns)) {
        (Some(x), Some(y)) => changes(&x, &y),
        _ => None,
    };
    by(true).or_else(|| by(false)).unwrap_or(false)
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
pub(crate) const STICKER_DONE: &str = "done: Klipolo shows it beside the speaker from just before the word, for about \
     1.5 s, popping in and fading out.";
/// What the model is told of a sound in a short the person wants without sound effects.
const NO_EFFECTS: &str = "Left out: the person asked for no sound effects.";
/// What the model is told of a sound whose every moment has the same sound already.
pub(crate) const ALREADY_THERE: &str =
    "Left out: the same sound plays there already. Don't mention it twice.";
/// What the model is told of an `animate` of a sticker Klipolo left out.
pub(crate) const LEFT_WITH_STICKER: &str = "Left out with its sticker. Don't mention it.";
/// What the model is told of an `animate` of a sticker Klipolo places.
pub(crate) const ANIMATED_ALREADY: &str = "done: Klipolo pops a short's stickers in and fades them out, with a pop sound when the short has sound effects.";
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

/// What the model is told of a sticker call Klipolo placed or left out; one of the person's own pictures left out is
/// one they may have asked for, so the model is never told to keep quiet about it.
pub(crate) fn told_of(sticker: &Sticker, own: bool) -> String {
    match sticker {
        Sticker::At { .. } => STICKER_DONE.to_owned(),
        Sticker::Left(why) if own => format!(
            "{} Say so if the person asked for it.",
            why.trim_end_matches(" Don't mention it.")
        ),
        Sticker::Left(why) => (*why).to_owned(),
    }
}

impl Cues {
    /// What the model is told of the hook's call: the sound effects Klipolo plays around it, so its reply says what is
    /// there. None in a short without sound effects.
    pub(crate) fn hook_said(&self) -> Option<String> {
        if !self.sound_effects {
            return None;
        }
        let times = |kind: Kind| -> Vec<Us> {
            let own = (self.own.iter())
                .filter(|o| o.kind == kind)
                .flat_map(|o| o.at.clone());
            let asked = (self.sounds.values())
                .filter(|c| c.kind == kind)
                .flat_map(|c| c.at.clone());
            own.chain(asked).collect()
        };
        let mut said = Vec::new();
        if !times(Kind::Rise).is_empty() {
            said.push("a rise under it".to_owned());
        }
        let whooshes: Vec<String> = (times(Kind::Whoosh).iter())
            .map(|&t| secs(self.short_time(t)))
            .collect();
        said.push(match whooshes.as_slice() {
            [] => "no whoosh, since the speaker doesn't change at a cut after it".to_owned(),
            [one] => format!("a whoosh at the cut to a new speaker at {one} s"),
            several => format!(
                "whooshes at the cuts to a new speaker at {} s",
                several.join(" and ")
            ),
        });
        if self
            .stickers
            .values()
            .any(|s| matches!(s, Sticker::At { .. }))
            && self.pop.is_some()
        {
            said.push("a pop with each sticker".to_owned());
        }
        let last = said.pop().unwrap_or_default();
        let list = if said.is_empty() {
            last
        } else {
            format!("{}, and {last}", said.join(", "))
        };
        Some(format!("done. Klipolo plays {list}."))
    }
}
