//! The media calls, on the additions' working copy: files from the library (`Inputs::library`, by name) placed as ops,
//! and pictures Muse Image draws first as jobs.
//! - `addMusic`: `media::music::add_music_ops` (30 %, ducking under speech, fades, to the video's end) from the time
//!   given, else the video's start: background music, wherever the playhead is. "The song" and the like are the
//!   person's one music file (`library::is_generic`).
//! - `addSound`: a sound at each time as the library's + puts one (`media::sound::add_sound_ops`: the whole file at full
//!   volume with no fades, attached to the clip under it, cut where the video ends); where each quote is said (the
//!   times given beside quotes only pick among places), else at each time; two moments within
//!   `evidence::TOLERANCE_US` of each other are one sound, at the first, and none goes on top of the same sound there
//!   already from this answer (`sound_near`: a sticker's `animate` sound on the same word). A short's own sounds (its
//!   stickers' pops, its payoff chime) never go where the project plays one of their file or kind already
//!   (`kind_there`), so a rerun of the short never doubles them.
//! - `addText`: the person's own words (`persons_words`; words of the model's own are refused back, for it to ask), or
//!   another app's as it gave them (`apps_words`), in one of the four text presets, placed by the model's
//!   `media::text::add_text_ops` exactly as the canvas's Add title and the library's presets place it: each format's
//!   own place, in 9:16 inside the area the apps keep clear. A kinetic hook under the face is refused back where words
//!   show in the hook's place then already (`hook_there`): those are changed, never drawn over.
//! - `addImage`: the picture as the library's + puts one (`media::image::add_image_ops_at`), at a size and in a corner of
//!   each format's own area (in 9:16 the area the apps keep clear) when one was asked for. In a short, the person's own
//!   picture with no place given (a logo) goes in a top corner off the face instead (`clear::own_corner`), for 3 s,
//!   placed again once the follow's path is known (`place_beside_again`).
//! - `makeSticker`: a `Job::MakeSticker` only, on the project as the message's additions leave it, anchored to the
//!   footage it shows over as a layer attaches (`Attach`); once the picture is drawn the app places it with
//!   `sticker_ops` (where and how long `sticker_placement` says, on the project as it is then). A job's prompt names the
//!   picture in the library too, so it has no dashes either.
//! - `makeTitleCard`: the title card template as the canvas adds one (`media::text::add_template_ops`, filling the frame
//!   in every format) with the person's exact title (`persons_words`, or an app's own: `apps_words`) and its other
//!   words left empty so only the title shows, for the template's own length (`TEMPLATE_US`) unless the person gave
//!   one, applied at once; and a `Job::MakeTitleBackground` for the picture to go under it, in the primary format as
//!   the additions leave it. No title of theirs: refused back, for the model to ask.
//!
//! A moment from the model is where the words it quotes are said (`atWords`, `quote`), or its time snapped to a word's
//! start or a cut near it (`Evidence::place`), and is then mapped into the working copy. Nothing is placed at the
//! video's end or past it, and a layer that would play past the end is cut there, so no media call makes the video
//! longer. A file from the pack comes in with the ops that first place it.

use std::borrow::Cow;
use std::collections::BTreeMap;

use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::Catalog;
use klipolo_model::font::FontRef;
use klipolo_model::ids::Id;
use klipolo_model::media::image::{IMAGE_SIZE, IMAGE_US, add_image_ops_at};
use klipolo_model::media::music::{MusicOpts, add_music_ops};
use klipolo_model::media::sound::add_sound_ops;
use klipolo_model::media::text::TextPreset as ModelTextPreset;
use klipolo_model::media::text::{
    TEMPLATE_US, TEXT_US, TitleTemplate, add_template_ops, add_text_ops,
};
use klipolo_model::media::text_motion::{self, TextMotion, Unit};
use klipolo_model::motion::{self, Ease, Motion};
use klipolo_model::ops::Op;
use klipolo_model::params::{ParamKind, ParamValue};
use klipolo_model::project::{Asset, AssetKind, FormatKind};
use klipolo_model::query::{anchor_at, main_track_duration, resolve_attach, video_duration};
use klipolo_model::scene::check_scene;
use klipolo_model::speech::{WordKind, is_mark, joins_unspaced, word_ranges};
use klipolo_model::text_safety::clean_shown;
use klipolo_model::time::{MIN_DURATION_US, Us};
use klipolo_model::timeline::{ItemContent, Outline};
use klipolo_model::validate::MAX_TEXT;
use klipolo_model::{Project, apply_ops};

use super::clear::Side;
use super::{
    Beside, Compiler, Edit, Job, MakeSticker, Refused, animate, clear, cues, label, quote,
    to_model, unfounded,
};
use crate::context::{secs, within_video};
use crate::evidence::{TOLERANCE_US, in_cut};
use crate::library::{Found, LibraryItem, MediaKind, is_generic};
use crate::sanitize;
use crate::tools::{Entrance, Exit, PictureSize, Position, TextPreset, ToolCall};
use klipolo_model::media::image::Spot;

/// The most names a refusal lists, then how many more there are: as many as Library shows of a kind.
const LISTED: usize = crate::context::LIBRARY_NAMES;
/// What may stand around a title the person quoted, left out of it.
const QUOTES: [char; 9] = ['\'', '"', '`', '‘', '’', '“', '”', '«', '»'];

/// Up to `LISTED` names, quoted, and how many more.
fn names(items: &[&LibraryItem]) -> String {
    let mut out: Vec<String> = (items.iter().take(LISTED))
        .map(|i| format!("{:?}", i.name))
        .collect();
    if items.len() > LISTED {
        out.push(format!("and {} more", items.len() - LISTED));
    }
    out.join(", ")
}

/// The library item `asked` names among `kinds`, or why none, said for `tool`.
pub(super) fn pick<'l>(
    c: &'l Compiler,
    tool: &str,
    asked: &str,
    kinds: &[MediaKind],
) -> Result<&'l LibraryItem, Refused> {
    let library = &c.inputs.library;
    let noun = kinds[0].noun();
    match library.find(asked, kinds) {
        Found::One(item) => Ok(item),
        Found::Several(items) => Err(to_model(format!(
            "{tool}: {asked:?} could be {}. Use one of these names, or ask the person which with askUser.",
            names(&items)
        ))),
        Found::None => {
            let all: Vec<&LibraryItem> = (kinds.iter())
                .flat_map(|&k| library.of_kind(k, false))
                .collect();
            let has = if all.is_empty() {
                format!("Library has no {noun}.")
            } else {
                format!("Library has {}.", names(&all))
            };
            Err(to_model(format!(
                "{tool}: no {noun} in Library is called {asked:?}. {has} Use a name from it, or ask the person \
                 with askUser."
            )))
        }
    }
}

/// The music `asked` names: a name from the library, or for "the song" the person's one music file.
fn music<'l>(c: &'l Compiler, asked: &str) -> Result<&'l LibraryItem, Refused> {
    let library = &c.inputs.library;
    // A file of that very name ("song.mp3") is still found by its name.
    if is_generic(asked) && !matches!(library.find(asked, &[MediaKind::Music]), Found::One(_)) {
        let own = library.of_kind(MediaKind::Music, true);
        return match own.as_slice() {
            [one] => Ok(one),
            [] => Err(to_model(format!(
                "addMusic: the person has no music file of their own in Library, so {asked:?} names none. Ask them \
                 which music with askUser."
            ))),
            several => Err(to_model(format!(
                "addMusic: the person has {} music files ({}), so {asked:?} could be any of them. Ask them which \
                 with askUser.",
                several.len(),
                names(several)
            ))),
        };
    }
    pick(c, "addMusic", asked, &[MediaKind::Music])
}

/// Where a call places something, on the snapshot's timeline as the context showed it.
#[derive(Clone, Copy, Debug)]
pub(super) enum At<'q> {
    /// No time given: the playhead.
    Playhead,
    /// Seconds a media tool gave: snapped to a word's start or a cut near it, else as given (`Evidence::place`).
    Sec(Us),
    /// Seconds that must stand on the evidence (`Evidence::moment`), or are refused: a layer's new start or split.
    Edge(Us),
    /// Where these words are said (`quote`); of several places, the one nearest one of these times (seconds given
    /// beside them).
    Words(&'q str, &'q [Us]),
}

impl<'q> At<'q> {
    /// A media call's place: where its words are said when it quotes some (its time picking among several places),
    /// else its time, else the playhead.
    pub(super) fn of(at_us: &'q Option<Us>, at_words: Option<&'q str>) -> Self {
        match (at_words, at_us) {
            (Some(words), near) => At::Words(words, near.as_slice()),
            (None, Some(t)) => At::Sec(*t),
            (None, None) => At::Playhead,
        }
    }

    /// The field it came from, for a call's one place.
    pub(super) fn field(self) -> &'static str {
        match self {
            At::Words(..) => "atWords",
            _ => "atSec",
        }
    }

    /// How a refusal names it: `atSec 12.0`, `atWords "went flying" (10.6 s)`, `the playhead (3.0 s)`.
    fn said(self, field: &str, t: Us) -> String {
        match self {
            At::Playhead => format!("the playhead ({} s)", secs(t)),
            At::Sec(_) | At::Edge(_) => format!("{field} {}", secs(t)),
            At::Words(words, _) => format!("{field} {} ({} s)", quote::echo(words), secs(t)),
        }
    }
}

/// Where a call's place `at` plays in `work`, said for `tool`'s `field`: the playhead, seconds snapped (`At::Sec`) or
/// standing on the evidence (`At::Edge`), or where the words quoted are said (`At::Words`, refused when they are said
/// nowhere, or in several places and no time picks one). The playhead or seconds in footage this answer's removals
/// take out (`Compiler::cut`) are refused: the layer would go with it, or move, while the reply says it was added. A
/// time past the main track's end keeps its distance from it. Nothing starts at the video's end or past it (refused, so
/// no layer ever makes the video longer).
pub(super) fn start(
    c: &Compiler,
    work: &Project,
    tool: &str,
    field: &str,
    at: At,
) -> Result<Us, Refused> {
    let playhead = within_video(c.snapshot, &c.inputs.view).0;
    let said = match at {
        At::Playhead => playhead,
        At::Sec(t) => c.evidence.place(t, &c.cut),
        At::Edge(t) => (c.evidence.moment(t)).ok_or_else(|| to_model(unfounded(tool, field, t)))?,
        At::Words(words, near) => {
            (c.said()).find(&c.cut, (tool, field), words, (near, playhead))?
        }
    };
    if matches!(at, At::Playhead | At::Sec(_)) && in_cut(&c.cut, said) {
        // The tools without atWords are asked for a time.
        let instead = match tool {
            "makeTitleCard" | "addMusic" => "Give a time in the parts you keep.",
            _ => "Quote words in the parts you keep.",
        };
        return Err(to_model(format!(
            "{tool}: {} is in a part this answer cuts. {instead}",
            at.said(field, said)
        )));
    }
    let snapshot_end = main_track_duration(&c.snapshot.timeline.main);
    let now = match c.map.time(work, said) {
        Some(now) => now,
        None if said >= snapshot_end => {
            main_track_duration(&work.timeline.main).saturating_add(said - snapshot_end)
        }
        None => {
            return Err(to_model(format!(
                "{tool}: {} was taken out by an earlier change in this answer.",
                at.said(field, said)
            )));
        }
    };
    if video_duration(&work.timeline).saturating_sub(now) < MIN_DURATION_US {
        return Err(to_model(format!(
            "{tool}: {} is at or past the video's end, where nothing can be added.",
            at.said(field, said)
        )));
    }
    Ok(now)
}

/// How long a layer from `start` (before the video's end, as `start` gives it) shows in `work`: `wanted` (None:
/// `default_us`), cut where the video ends.
fn length(work: &Project, start: Us, wanted: Option<Us>, default_us: Us) -> Us {
    (wanted.unwrap_or(default_us)).min(video_duration(&work.timeline).saturating_sub(start))
}

/// `ops` with the layer they add lasting `duration_us` (the model's ops add one at its default length), its motion
/// fitted to that length as a trim fits it (`Motion::fit`: a text's fades no longer than the layer).
fn lasting(mut ops: Vec<Op>, duration_us: Us) -> Vec<Op> {
    for op in &mut ops {
        if let Op::AddItem { item, .. } = op {
            item.duration_us = duration_us;
            item.layout.motion = item.layout.motion.take().and_then(|mut m| {
                m.fit(duration_us);
                motion::kept(m)
            });
        }
    }
    ops
}

/// A word of a message with where it sits, compared as `bare`.
struct Said {
    bare: String,
    start: usize,
    end: usize,
}

/// `text`'s words as captions show them (`word_ranges`: a Chinese or Japanese character is a word, as the transcript
/// holds it), each with its letters and digits lowercased.
fn said(text: &str) -> Vec<Said> {
    let mut out = Vec::new();
    for range in word_ranges(text) {
        let bare: String = text[range.clone()]
            .trim_matches(|c: char| !c.is_alphanumeric() && !is_mark(c))
            .to_lowercase();
        if !bare.is_empty() {
            out.push(Said {
                bare,
                start: range.start,
                end: range.end,
            });
        }
    }
    out
}

/// `text` as the person wrote it in `message`, when they did: the same words in a row, whatever their case and the
/// punctuation around them, taken from the message as written, without the quotes or the comma or full stop around
/// them. None when the message doesn't hold them.
pub(crate) fn persons_words(message: &str, text: &str) -> Option<String> {
    let wanted = said(text);
    let words = said(message);
    if wanted.is_empty() || wanted.len() > words.len() {
        return None;
    }
    let at = (0..=words.len() - wanted.len()).find(|&i| {
        (words[i..i + wanted.len()].iter().zip(&wanted)).all(|(w, x)| w.bare == x.bare)
    })?;
    let span = &message[words[at].start..words[at + wanted.len() - 1].end];
    let span = span
        .trim_start_matches(|c: char| QUOTES.contains(&c) || c.is_whitespace())
        .trim_end_matches(|c: char| {
            QUOTES.contains(&c)
                || matches!(
                    c,
                    ',' | '.' | ';' | ':' | '，' | '。' | '、' | '；' | '：' | '।' | '،' | '؛'
                )
                || c.is_whitespace()
        });
    (!span.is_empty()).then(|| span.to_owned())
}

/// Words another app gives for the screen, as it gave them: control, invisible and bidirectional characters dropped,
/// but what real text needs kept (joiners, variation selectors, and line breaks, which a text layer draws), and runs of
/// spaces one space (`text_safety::clean_shown`). Refused when they are longer than a text layer holds (`MAX_TEXT`) or
/// nothing is left, said for `tool`'s `field` with what to do.
fn apps_words((tool, field): (&str, &str), text: &str) -> Result<String, Refused> {
    let n = text.chars().count();
    if n > MAX_TEXT {
        return Err(to_model(format!(
            "{tool}: {field} is {n} characters; words on screen hold at most {MAX_TEXT}. Use fewer words."
        )));
    }
    let words = clean_shown(text, MAX_TEXT);
    if words.is_empty() {
        return Err(to_model(format!(
            "{tool}: {field} has no words to show. Give the words to put on screen."
        )));
    }
    Ok(words)
}

/// The ops that add title template `template` (the title card, or the kinetic title) reading `text` (its other words
/// left empty) from `start` for `duration` (None: `TEMPLATE_US`), as the canvas adds a title template
/// (`media::text::add_template_ops`: filling the frame in every format, attached to the clip under it); and the new
/// layer's id. Refused when the template holds fewer characters, said for `tool`'s `field`.
fn title_ops(
    work: &Project,
    template: TitleTemplate,
    (tool, field): (&str, &str),
    text: &str,
    (start, duration): (Us, Option<Us>),
) -> Result<(Vec<Op>, Id), Refused> {
    let def = (Catalog::builtin().get(&template.source()))
        .ok_or_else(|| to_model(format!("{tool}: this build has no such title template.")))?;
    let mut params = BTreeMap::new();
    for spec in &def.params {
        let ParamKind::Text { max_len, .. } = &spec.kind else {
            continue;
        };
        let value = if spec.key == "title" {
            let n = text.chars().count();
            if n > *max_len as usize {
                return Err(to_model(format!(
                    "{tool}: {field} is {n} characters; a {} holds at most {max_len}.",
                    def.name.to_lowercase()
                )));
            }
            text.to_owned()
        } else {
            String::new()
        };
        params.insert(spec.key.clone(), ParamValue::Text(value));
    }
    let ops = add_template_ops(work, template, params, start);
    let id = (ops.iter())
        .find_map(|op| match op {
            Op::AddItem { item, .. } => Some(item.id.clone()),
            _ => None,
        })
        .ok_or_else(|| to_model(format!("{tool}: the title card template added nothing.")))?;
    Ok((lasting(ops, length(work, start, duration, TEMPLATE_US)), id))
}

pub(super) fn add_music(
    c: &Compiler,
    work: &Project,
    file: &str,
    (volume, duck): (Option<f64>, Option<bool>),
    at: Option<Us>,
) -> Result<Edit, Refused> {
    let item = music(c, file)?;
    let start = match at {
        // No time given: background music, from the video's start, wherever the playhead is.
        None if video_duration(&work.timeline) >= MIN_DURATION_US => 0,
        _ => start(c, work, "addMusic", "atSec", At::of(&at, None))?,
    };
    let defaults = MusicOpts::default();
    let opts = MusicOpts {
        volume: volume.unwrap_or(defaults.volume),
        duck_db: if duck == Some(false) {
            None
        } else {
            defaults.duck_db
        },
        start_us: start,
        end_us: None,
    };
    let ops =
        add_music_ops(work, &item.asset, opts).map_err(|e| to_model(format!("addMusic: {e}")))?;
    Ok(Edit {
        ops,
        label: label(
            "addMusic",
            vec![("file", ErrorParam::from(item.name.as_str()))],
        ),
    })
}

/// Whether an earlier call of this answer put the sound file `asset_id` within `TOLERANCE_US` of `at` in `work`: one
/// more there would play on top of it (an `addSound` and a sticker's `animate` sound on one word), so it is left out.
/// A sound the project had before the message doesn't count: "another ding there" adds one.
pub(super) fn sound_near(c: &Compiler, work: &Project, asset_id: &str, at: Us) -> bool {
    let had =
        |id: &Id| (c.snapshot.timeline.tracks.iter()).any(|t| t.items.iter().any(|i| &i.id == id));
    (work.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .any(|item| {
            matches!(&item.content, ItemContent::Audio { asset_id: a, .. } if a == asset_id)
                && item.start_us.abs_diff(at) <= TOLERANCE_US.unsigned_abs()
                && !had(&item.id)
        })
}

/// Whether `item` is a whoosh, this answer cuts the video again, and the project had a whoosh within `TOLERANCE_US` of
/// `at` in `work` before the message: a cut's whoosh again where a short is cut once more would play two at once.
/// Asked for without a cut ("another whoosh there"), and any other sound there again, it still plays.
fn whoosh_there(c: &Compiler, work: &Project, item: &LibraryItem, at: Us) -> bool {
    let whoosh = |name: &str| {
        (c.inputs.library.items.iter())
            .find(|l| l.asset.id == name)
            .is_some_and(|l| cues::sound_kind(l) == cues::Kind::Whoosh)
    };
    let had =
        |id: &Id| (c.snapshot.timeline.tracks.iter()).any(|t| t.items.iter().any(|i| &i.id == id));
    cues::sound_kind(item) == cues::Kind::Whoosh
        && !c.cut.is_empty()
        && (work.timeline.tracks.iter())
            .flat_map(|t| &t.items)
            .any(|i| {
                matches!(&i.content, ItemContent::Audio { asset_id, .. } if whoosh(asset_id))
                    && i.start_us.abs_diff(at) <= TOLERANCE_US.unsigned_abs()
                    && had(&i.id)
            })
}

pub(super) fn add_sound(
    c: &Compiler,
    work: &Project,
    name: &str,
    (words, times): (&[String], &[Us]),
) -> Result<Edit, Refused> {
    let item = pick(c, "addSound", name, &[MediaKind::Sound])?;
    let asset = &item.asset;
    if !asset.has_audio || asset.duration_us < MIN_DURATION_US {
        return Err(to_model(format!(
            "addSound: {:?} has no sound to play.",
            item.name
        )));
    }
    // With quotes, the times only pick where each is said: one each when there are as many, else any of them.
    let mut starts: Vec<Us> = Vec::with_capacity(words.len() + times.len());
    let quoted = (words.iter().enumerate()).map(|(k, w)| {
        let near = match times.get(k) {
            Some(t) if times.len() == words.len() => std::slice::from_ref(t),
            _ => times,
        };
        (format!("atWords[{k}]"), At::Words(w, near))
    });
    let timed = (times.iter().enumerate())
        .filter(|_| words.is_empty())
        .map(|(k, &t)| (format!("atSec[{k}]"), At::Sec(t)));
    for (field, at) in quoted.chain(timed) {
        let at = start(c, work, "addSound", &field, at)?;
        // Two moments within the evidence's tolerance are one: one sound, at the first. The same sound already there
        // (an earlier call's) plays for this one too.
        if !starts
            .iter()
            .any(|&s| s.abs_diff(at) <= TOLERANCE_US.unsigned_abs())
            && !sound_near(c, work, &asset.id, at)
            && !whoosh_there(c, work, item, at)
        {
            starts.push(at);
        }
    }
    // The file comes in once, then a sound at each moment as the library's + puts one (`add_sound_ops`).
    let ops = sounds_at(work, asset, &starts)?;
    Ok(Edit {
        ops,
        label: label(
            "addSound",
            vec![
                ("name", ErrorParam::from(item.name.as_str())),
                ("count", ErrorParam::from(starts.len())),
            ],
        ),
    })
}

/// `text` as it is said in the video, when it is: the same words in a row in one file's transcript (`persons_words` on
/// the transcript's words), as the transcript writes them. Files are looked at in the project's order.
fn said_in_video(c: &Compiler, text: &str) -> Option<String> {
    let transcripts = &c.inputs.speech.transcripts;
    (c.snapshot.assets.iter())
        .filter_map(|asset| transcripts.get(&asset.id))
        .find_map(|t| {
            // As captions show them: two Chinese or Japanese words meet without a space.
            let mut said = String::new();
            for w in (t.words.iter()).filter(|w| w.kind != WordKind::Pause) {
                let joins = said.ends_with(joins_unspaced) && w.text.starts_with(joins_unspaced);
                if !said.is_empty() && !joins {
                    said.push(' ');
                }
                said.push_str(&w.text);
            }
            persons_words(&said, text)
        })
}

/// The most words a hook paraphrasing the video holds.
pub const HOOK_WORDS: usize = 6;

/// `text` as a short's hook that paraphrases the video: at most `HOOK_WORDS` words, more than half of its words of three
/// letters or more said in the video (in a transcript of the project's files). As the model wrote it, trimmed.
fn hook_from_video(c: &Compiler, text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || words.len() > HOOK_WORDS {
        return None;
    }
    let bare = |w: &str| {
        w.trim_matches(|ch: char| !ch.is_alphanumeric())
            .to_lowercase()
    };
    let content: Vec<String> = (words.iter())
        .map(|w| bare(w))
        .filter(|w| w.chars().count() >= 3)
        .collect();
    if content.is_empty() {
        return None;
    }
    let transcripts = &c.inputs.speech.transcripts;
    let said: std::collections::HashSet<String> = (c.snapshot.assets.iter())
        .filter_map(|a| transcripts.get(&a.id))
        .flat_map(|t| t.words.iter())
        .filter(|w| w.kind != WordKind::Pause)
        .map(|w| bare(&w.text))
        .collect();
    let found = content.iter().filter(|w| said.contains(*w)).count();
    (2 * found > content.len()).then(|| words.join(" "))
}

pub(super) fn add_text(
    c: &Compiler,
    work: &Project,
    text: &str,
    preset: TextPreset,
    (at, duration): (At, Option<Us>),
    animation: Option<Entrance>,
    font: Option<&str>,
) -> Result<Edit, Refused> {
    let words = screen_words(c, ("addText", "text"), text)?;
    let start = start(c, work, "addText", at.field(), at)?;
    // Kinetic words are the kinetic title template, near the top whatever the preset, in its own font; but where the
    // 9:16 version shows a face there (`clear::face_in_portrait`), the template would put them on it, so they are words
    // on a layer of their own (`kinetic_look`), on two lines, whose letters rise in one by one as its do
    // (`rising_letters`), under the face (`clear::hook_under_face`).
    let kinetic = animation == Some(Entrance::Kinetic);
    let under_face =
        kinetic && c.portrait && clear::face_in_portrait(work, start, c.follows, &c.inputs.scenes);
    if kinetic && !under_face {
        let (ops, _) = title_ops(
            work,
            TitleTemplate::KineticTitle,
            ("addText", "text"),
            &words,
            (start, duration.or(Some(TEXT_US))),
        )?;
        return Ok(Edit {
            ops,
            label: label(
                "addText",
                vec![
                    ("text", ErrorParam::from(words.as_str())),
                    ("preset", ErrorParam::from("kinetic")),
                ],
            ),
        });
    }
    let (shown, layout) = if kinetic {
        (two_lines(&words.to_uppercase()), ModelTextPreset::Title)
    } else {
        (words.clone(), preset.preset())
    };
    let mut ops =
        add_text_ops(work, layout, &shown, start).map_err(|e| to_model(format!("addText: {e}")))?;
    if kinetic {
        kinetic_look(&mut ops);
    } else if let Some(animation) = animation {
        for op in &mut ops {
            if let Op::AddItem { item, .. } = op {
                let change = animate::Change {
                    enter: Some(animation),
                    ..animate::Change::default()
                };
                let now = item.layout.motion.take();
                item.layout.motion =
                    animate::animated(now, &change, item.duration_us, Some(&words));
            }
        }
    }
    if let Some(name) = font {
        in_font(c, &mut ops, name)?;
    }
    let len = length(work, start, duration, TEXT_US);
    if kinetic {
        rising_letters(&mut ops, &shown);
        clear::hook_under_face(work, &mut ops);
        // One hook in its place at a time: words there already (an earlier answer's short, or this one's) are changed,
        // never drawn over.
        if let Some(there) = hook_there(c, work, (start, start + len)) {
            return Err(to_model(format!(
                "addText: {there} show in the hook's place then already. Change those words with styleText (their \
                 text), or put these words at another time."
            )));
        }
    }
    Ok(Edit {
        ops: lasting(ops, len),
        label: label(
            "addText",
            vec![
                ("text", ErrorParam::from(words.as_str())),
                (
                    "preset",
                    ErrorParam::from(if kinetic { "kinetic" } else { preset.key() }),
                ),
            ],
        ),
    })
}

/// The words in the hook's place in 9:16 (`captions::in_hook_band`) on `work` over `(from, to)`, as the model names
/// them: their text, and their ref where the context listed them ("the words \"FISH ARE SMALLER\" (t1)"). None
/// when nothing is there.
fn hook_there(c: &Compiler, work: &Project, (from, to): (Us, Us)) -> Option<String> {
    let item = (work.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| {
            i.start_us < to && from < i.end_us() && klipolo_model::captions::in_hook_band(work, i)
        })?;
    let text = match &item.content {
        ItemContent::Text { text, .. } => text.clone(),
        ItemContent::Generator { params, .. } => match params.get("title") {
            Some(ParamValue::Text(title)) => title.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let refs = &c.inputs.refs;
    let short = (1..)
        .map(|k| format!("t{k}"))
        .take_while(|r| refs.text(r).is_some())
        .find(|r| refs.text(r) == Some(item.id.as_str()));
    Some(match short {
        Some(r) => format!("the words {text:?} ({r})"),
        None => format!("the words {text:?}, added by this answer,"),
    })
}

/// The hook's size in pixels at a 1080-pixel short side, in Anton (condensed, so a line of 13 capitals fits the band
/// at full size): its capitals stand about 1.3 times word pop's 96-pixel captions'.
pub const HOOK_PX: f64 = 130.0;

/// `text` on two lines, broken at the space that makes the longer line shortest (one word stays one line).
fn two_lines(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let best = (1..words.len()).min_by_key(|&k| {
        let (a, b) = (words[..k].join(" "), words[k..].join(" "));
        a.chars().count().max(b.chars().count())
    });
    match best {
        Some(k) => format!("{}\n{}", words[..k].join(" "), words[k..].join(" ")),
        None => words.join(" "),
    }
}

/// The hook's look on `ops`' words, as bold as the kinetic title's but condensed to stay big on two lines: Anton,
/// `HOOK_PX`, white with a black outline and a shadow.
fn kinetic_look(ops: &mut [Op]) {
    for op in ops {
        let Op::AddItem { item, .. } = op else {
            continue;
        };
        let ItemContent::Text { style, .. } = &mut item.content else {
            continue;
        };
        style.font = FontRef::bundled("Anton");
        style.weight = 400;
        style.size = HOOK_PX;
        style.outline = Some(Outline {
            color: "#000000".into(),
            width: 6.0,
        });
        style.shadow = true;
    }
}

/// `ops`' words as animated words whose letters rise in one by one, as the kinetic title's do (60 pixels up from
/// clear, 0.45 s each, 0.04 s apart), or word by word where there are too many letters to draw in time. Words the
/// scene check refuses stay plain words, fading in.
fn rising_letters(ops: &mut [Op], text: &str) {
    for op in ops {
        let Op::AddItem { item, .. } = op else {
            continue;
        };
        let ItemContent::Text { style, .. } = &item.content else {
            continue;
        };
        let mut m = TextMotion {
            unit: Unit::Letter,
            from: text_motion::From {
                y: Some(60.0),
                opacity: Some(0.0),
                ..text_motion::From::default()
            },
            unit_secs: Some(0.45),
            ease: Some(Ease::Power3Out),
            stagger_secs: Some(0.04),
            order: None,
            exit: false,
        };
        if text_motion::problem(text, &m).is_some() {
            m.unit = Unit::Word;
            m.stagger_secs = None;
        }
        let scene = text_motion::scene(text, style, &m);
        if text_motion::problem(text, &m).is_some()
            || check_scene(&scene, Catalog::builtin()).is_err()
        {
            continue;
        }
        let params = text_motion::params(text, style, &m);
        item.content = ItemContent::generator(text_motion::source(), params);
        if let Some(mut moves) = item.layout.motion.take() {
            moves.enter = None;
            item.layout.motion = motion::kept(moves);
        }
    }
}

/// The new text in the font `name` names, at its face nearest the preset's weight and style; refused to the model when
/// no font has that name. A Google font not on this computer is named all the same: it is never downloaded by itself
/// (`font_note` says so, and the person downloads it).
fn in_font(c: &Compiler, ops: &mut [Op], name: &str) -> Result<(), Refused> {
    for op in ops {
        let Op::AddItem { item, .. } = op else {
            continue;
        };
        let ItemContent::Text { style, .. } = &mut item.content else {
            continue;
        };
        let named = c.inputs.fonts.named(name, style.weight, style.italic).ok_or_else(|| {
            to_model(format!(
                "addText: no font is named {name:?}, here or at Google. Leave font out for the preset's."
            ))
        })?;
        style.font = named.font;
        style.weight = named.weight;
        style.italic = named.italic;
    }
    Ok(())
}

/// What the reply says of `addText`'s or `setCaptions`' font when it isn't on this computer yet (a Google font): it shows
/// in Inter until the person downloads it, which the missing-fonts note over the preview offers. None for any other call
/// or font.
pub(super) fn font_note(c: &Compiler, call: &ToolCall) -> Option<String> {
    let (name, what) = match call {
        ToolCall::AddText {
            font: Some(name), ..
        } => (name, "words"),
        // The captions' font, as `setCaptions` sets it.
        ToolCall::SetCaptions { look, .. } => (look.font.as_ref()?.as_ref()?, "captions"),
        _ => return None,
    };
    let named = c.inputs.fonts.named(name, 400, false)?;
    named.download_bytes?;
    let family = named.font.family;
    Some(format!(
        "{family} isn't on this Mac yet, so the {what} show in Inter until you download it over the preview."
    ))
}

/// Whether `addImage` of `file` with this place is a sticker Klipolo places beside the speaker: a pack picture (not a
/// background) with no place given. The person's own pictures (a logo) go where asked, as ever.
pub(super) fn is_sticker(c: &Compiler, file: &str, position: Option<Position>) -> bool {
    let Ok(item) = pick(c, "addImage", file, &[MediaKind::Image]) else {
        return false;
    };
    let background = item.tags.iter().any(|t| t == "background");
    position.is_none() && item.pack && !background && item.asset.kind == AssetKind::Image
}

/// How long the person's own picture with no place given shows in a short, unless asked: about as long as a logo takes
/// to read.
const CORNER_US: Us = 3_000_000;

/// Whether a picture at timeline `start` of `work` goes in a short: this answer makes one (`cues`), or the video is
/// vertical and its 9:16 version shows a face there.
fn in_short(c: &Compiler, work: &Project, start: Us) -> bool {
    c.cues.as_ref().is_some_and(|q| q.makes_short)
        || (work.settings.primary_format == FormatKind::Portrait
            && clear::face_in_portrait(work, start, c.follows, &c.inputs.scenes))
}

/// Whether `item`, given `position`, at timeline `start` of `work`, is the person's own picture in a short's top
/// corner (`add_image`).
fn goes_in_corner(
    c: &Compiler,
    work: &Project,
    (item, position): (&LibraryItem, Option<Position>),
    start: Us,
) -> bool {
    !item.pack && position.is_none() && in_short(c, work, start)
}

/// The picture `edit` (`add_image` of `file` given `position`, on `work`) put in a short's top corner, by id; None for
/// any other picture.
pub(super) fn in_corner(
    c: &Compiler,
    work: &Project,
    (file, position): (&str, Option<Position>),
    edit: &Edit,
) -> Option<Id> {
    let item = pick(c, "addImage", file, &[MediaKind::Image]).ok()?;
    let (id, start) = (edit.ops.iter()).find_map(|op| match op {
        Op::AddItem { item, .. } if matches!(item.content, ItemContent::Image { .. }) => {
            Some((item.id.clone(), item.start_us))
        }
        _ => None,
    })?;
    goes_in_corner(c, work, (item, position), start).then_some(id)
}

pub(super) fn add_image(
    c: &Compiler,
    work: &Project,
    file: &str,
    (size, position): (Option<PictureSize>, Option<Position>),
    (at, duration): (At, Option<Us>),
) -> Result<Edit, Refused> {
    let item = pick(c, "addImage", file, &[MediaKind::Image])?;
    if item.asset.kind != AssetKind::Image {
        return Err(to_model(format!(
            "addImage: {:?} is not a picture.",
            item.name
        )));
    }
    let start = start(c, work, "addImage", at.field(), at)?;
    // A sticker with no place given, in a short or not: beside the speaker for a moment, never in the middle of the
    // frame on them.
    if is_sticker(c, file, position) {
        let len = length(work, start, duration.or(Some(cues::HOLD_US)), IMAGE_US);
        let mut ops = add_image_ops_at(
            work,
            &item.asset,
            start,
            PictureSize::Medium.share(),
            Spot::TopRight,
        )
        .map_err(|e| to_model(format!("addImage: {e}")))?;
        let (boxes, _) = clear::beside_speaker(work, start, &item.asset, c.follows, Side::Right);
        clear::place_beside(work, &mut ops, &boxes);
        for op in &mut ops {
            if let Op::AddItem { item, .. } = op {
                item.layout.motion = popping(len);
            }
        }
        return Ok(Edit {
            ops: lasting(ops, len),
            label: label(
                "addImage",
                vec![("file", ErrorParam::from(item.name.as_str()))],
            ),
        });
    }
    // The person's own picture with no place given, in a short ("put my logo at the start"): a top corner off the
    // face, the hook and the captions, for the time asked or `CORNER_US`; the middle would be over the speaker.
    if goes_in_corner(c, work, (item, position), start) {
        let len = length(work, start, duration.or(Some(CORNER_US)), IMAGE_US);
        let mut ops = add_image_ops_at(work, &item.asset, start, IMAGE_SIZE, Spot::TopLeft)
            .map_err(|e| to_model(format!("addImage: {e}")))?;
        let (boxes, spot) = clear::own_corner(work, start, &item.asset, c.follows);
        clear::place_in_corner(work, &mut ops, (&boxes, spot));
        return Ok(Edit {
            ops: lasting(ops, len),
            label: label(
                "addImage",
                vec![("file", ErrorParam::from(item.name.as_str()))],
            ),
        });
    }
    let size = size.unwrap_or(PictureSize::Medium);
    let spot = position.unwrap_or(Position::Center).spot();
    // As the library's + puts a picture (`add_image_ops`), at a size and in a corner when one was asked for.
    let mut ops = add_image_ops_at(work, &item.asset, start, size.share(), spot)
        .map_err(|e| to_model(format!("addImage: {e}")))?;
    clear::clear_of_person(work, &mut ops, &item.asset, (size.share(), spot), start);
    Ok(Edit {
        ops: lasting(ops, length(work, start, duration, IMAGE_US)),
        label: label(
            "addImage",
            vec![("file", ErrorParam::from(item.name.as_str()))],
        ),
    })
}

/// Where a drawn sticker goes on `p`, the project as it is when the picture comes, and for how long: over the footage
/// its job's anchor names (`query::resolve_attach`, as an attached layer finds its moment: its own clip first, then a
/// piece or copy of it), or at `at_us` when it has none (past the main track's end). None drops the job: the anchored
/// footage was cut while the picture was drawn, or the start is now at the video's end or past it. The length is cut
/// where the video ends.
pub fn sticker_placement(p: &Project, job: &MakeSticker) -> Option<(Us, Us)> {
    let start = match &job.anchor {
        Some(anchor) => resolve_attach(&p.timeline.main, anchor)?,
        None => job.at_us,
    };
    let room = video_duration(&p.timeline).saturating_sub(start);
    (room >= MIN_DURATION_US).then(|| (start, job.duration_us.min(room)))
}

/// The ops that place drawn sticker `asset` for `job` on `p`, the project as it is when the picture comes: where and for
/// how long `sticker_placement` says, at the job's size and in its corner as the library places a picture
/// (`media::image::add_image_ops_at`). None when there is nowhere to place it: the footage it was for was cut, or its
/// start is now at the video's end (or the file isn't a picture, which a drawing always is).
pub fn sticker_ops(p: &Project, job: &MakeSticker, asset: &Asset) -> Option<Vec<Op>> {
    let (start, duration) = sticker_placement(p, job)?;
    let mut ops = add_image_ops_at(p, asset, start, job.size.share(), job.position.spot()).ok()?;
    match job.beside {
        // A short's: beside the speaker, on the project as it is now (its follow's path, once it has one).
        Some(beside) => {
            let prefer = if beside.right {
                Side::Right
            } else {
                Side::Left
            };
            let (boxes, _) = clear::beside_speaker(p, start, asset, beside.follows, prefer);
            clear::place_beside(p, &mut ops, &boxes);
        }
        None => {
            let placed = (job.size.share(), job.position.spot());
            clear::clear_of_person(p, &mut ops, asset, placed, start);
        }
    }
    if let Some(m) = &job.motion {
        for op in &mut ops {
            if let Op::AddItem { item, .. } = op {
                item.layout.motion = Some(Motion::clone(m));
            }
        }
    }
    let mut ops = lasting(ops, duration);
    // Its sound where its animation starts, as `animate` places one (left out where it no longer fits).
    if let Some(sound) = &job.sound {
        let m = (ops.iter()).find_map(|op| match op {
            Op::AddItem { item, .. } => item.layout.motion.clone(),
            _ => None,
        });
        let at = animate::sound_start(exit_only(m.as_ref()), start, duration, m.as_ref());
        let with_picture = apply_ops(p, &ops).ok()?;
        if let Ok(more) = add_sound_ops(&with_picture, sound, at) {
            ops.extend(more);
        }
    }
    Some(ops)
}

/// Whether a picture moving as `m` has only an exit, so its sound plays as it goes out.
fn exit_only(m: Option<&Motion>) -> bool {
    m.is_some_and(|m| m.enter.is_none() && m.looping.is_none())
}

/// `job`'s sound, unless an earlier call of this answer put the same sound where it would play in `with_additions`
/// (`sound_near`): left out of the job then, as `animate` leaves it out on a layer.
pub(super) fn sticker_sound(c: &Compiler, with_additions: &Project, job: &mut MakeSticker) {
    let Some(sound) = &job.sound else {
        return;
    };
    let m = job.motion.as_deref();
    let at = animate::sound_start(exit_only(m), job.at_us, job.duration_us, m);
    if sound_near(c, with_additions, &sound.id, at) {
        job.sound = None;
    }
}

/// `makeSticker`'s job, on the project as the message's additions leave it (`with_additions`), whatever came before it
/// in the answer: where it shows, and the footage it shows over (`query::anchor_at`, as a layer attaches; None past
/// the main track's end).
pub(super) fn make_sticker(
    c: &Compiler,
    with_additions: &Project,
    prompt: &str,
    position: Option<Position>,
    (at, duration): (At, Option<Us>),
) -> Result<Job, Refused> {
    let start = start(c, with_additions, "makeSticker", at.field(), at)?;
    // No place given: beside the speaker for a moment, popping in, never in the middle of the frame on them.
    let beside = position.is_none().then_some(Beside {
        follows: c.follows,
        right: true,
        corner: false,
    });
    let held = if beside.is_some() {
        duration.or(Some(cues::HOLD_US))
    } else {
        duration
    };
    let duration_us = length(with_additions, start, held, IMAGE_US);
    Ok(Job::MakeSticker(MakeSticker {
        prompt: sanitize::reply(prompt),
        at_us: start,
        duration_us,
        position: position.unwrap_or(Position::TopRight),
        size: PictureSize::Medium,
        anchor: anchor_at(&with_additions.timeline.main, start),
        motion: beside.and_then(|_| popping(duration_us)).map(Box::new),
        sound: None,
        beside,
    }))
}

/// A short's drawn sticker (`cues::Sticker::At`): its job at Klipolo's time, from `start` on the snapshot's timeline
/// for its `duration`, popping in and fading out with the short's pop, placed beside the speaker once it comes
/// (`MakeSticker::beside`). None for one left out.
pub(super) fn cued_make_sticker(
    c: &Compiler,
    with_additions: &Project,
    prompt: &str,
    sticker: &cues::Sticker,
) -> Result<Option<Job>, Refused> {
    let cues::Sticker::At {
        start,
        duration,
        prefer,
    } = *sticker
    else {
        return Ok(None);
    };
    let at = (c.map.time(with_additions, start)).ok_or_else(|| {
        to_model(
            "makeSticker: its words were taken out by an earlier change in this answer.".to_owned(),
        )
    })?;
    Ok(Some(Job::MakeSticker(MakeSticker {
        prompt: sanitize::reply(prompt),
        at_us: at,
        duration_us: length(with_additions, at, Some(duration), IMAGE_US),
        position: Position::TopRight,
        size: PictureSize::Medium,
        anchor: anchor_at(&with_additions.timeline.main, at),
        motion: popping(duration).map(Box::new),
        sound: (c.cues.as_ref()).and_then(|q| q.pop.clone()).map(Box::new),
        beside: Some(Beside {
            follows: c.follows,
            right: prefer == Side::Right,
            corner: false,
        }),
    })))
}

/// A short's sticker's motion for a layer `len_us` long: in with a pop, out with a fade.
fn popping(len_us: Us) -> Option<Motion> {
    let change = animate::Change {
        enter: Some(Entrance::Pop),
        exit: Some(Exit::Fade),
        emphasis: None,
    };
    animate::animated(None, &change, len_us, None)
}

/// A short's sticker `file` (`cues::Sticker`): shown from `start` on the snapshot's timeline for its `duration`, beside
/// the speaker (`clear::beside_speaker`), popping in and fading out, with the short's pop as it comes in. One left out
/// adds nothing.
pub(super) fn cued_sticker(
    c: &Compiler,
    work: &Project,
    (call, file): (usize, &str),
    sticker: &cues::Sticker,
) -> Result<Edit, Refused> {
    let item = pick(c, "addImage", file, &[MediaKind::Image])?;
    let label = label(
        "addImage",
        vec![("file", ErrorParam::from(item.name.as_str()))],
    );
    let cues::Sticker::At {
        start,
        duration,
        prefer,
    } = *sticker
    else {
        return Ok(Edit {
            ops: Vec::new(),
            label,
        });
    };
    let at = (c.map.time(work, start)).ok_or_else(|| {
        to_model(
            "addImage: its words were taken out by an earlier change in this answer.".to_owned(),
        )
    })?;
    let mut ops = add_image_ops_at(
        work,
        &item.asset,
        at,
        PictureSize::Medium.share(),
        Spot::TopRight,
    )
    .map_err(|e| to_model(format!("addImage: {e}")))?;
    let (boxes, _) = clear::beside_speaker(work, at, &item.asset, c.follows, prefer);
    clear::place_beside(work, &mut ops, &boxes);
    for op in &mut ops {
        if let Op::AddItem { item, .. } = op {
            item.layout.motion = popping(duration);
        }
    }
    let duration = length(work, at, Some(duration), IMAGE_US);
    let mut ops = lasting(ops, duration);
    // Its pop, or the sound the person asked for it; a sound that can't play leaves the sticker as it is.
    let cues = c.cues.as_ref();
    let sound = (cues
        .and_then(|q| q.sticker_sounds.get(&call))
        .map(|(_, a)| a))
    .or_else(|| cues.and_then(|q| q.pop.as_ref()));
    if let Some(sound) = sound
        && let Ok(with) = apply_ops(work, &ops)
        && !kind_there(c, &with, &sound.id, at)
        && let Ok(more) = add_sound_ops(&with, sound, at)
    {
        ops.extend(more);
    }
    Ok(Edit { ops, label })
}

/// Whether `work` plays the sound file `asset_id`, or one of its kind (`cues::sound_kind`, by the library: a pop, a
/// chime), within `TOLERANCE_US` of `at`, from this answer or before it: a short's own sound there again (a rerun of
/// the short) would play two at once.
fn kind_there(c: &Compiler, work: &Project, asset_id: &str, at: Us) -> bool {
    let kind_of = |id: &str| {
        (c.inputs.library.items.iter())
            .find(|l| l.asset.id == id)
            .map(cues::sound_kind)
            .filter(|k| *k != cues::Kind::Other)
    };
    let kind = kind_of(asset_id);
    (work.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .any(|i| match &i.content {
            ItemContent::Audio { asset_id: a, .. } => {
                i.start_us.abs_diff(at) <= TOLERANCE_US.unsigned_abs()
                    && (a == asset_id || (kind.is_some() && kind_of(a) == kind))
            }
            _ => false,
        })
}

/// A short's sound effect `name` at Klipolo's moments (`cues::Cue`, on the snapshot's timeline), as `addSound` places
/// one; none (left out) adds nothing.
pub(super) fn cued_sound(
    c: &Compiler,
    work: &Project,
    name: &str,
    at: &[Us],
) -> Result<Edit, Refused> {
    let item = pick(c, "addSound", name, &[MediaKind::Sound])?;
    let mut starts: Vec<Us> = Vec::new();
    for &t in at {
        let Some(now) = c.map.time(work, t) else {
            continue;
        };
        if video_duration(&work.timeline).saturating_sub(now) >= MIN_DURATION_US
            && !starts.contains(&now)
            && !kind_there(c, work, &item.asset.id, now)
        {
            starts.push(now);
        }
    }
    let ops = sounds_at(work, &item.asset, &starts)?;
    Ok(Edit {
        ops,
        label: label(
            "addSound",
            vec![
                ("name", ErrorParam::from(item.name.as_str())),
                ("count", ErrorParam::from(starts.len())),
            ],
        ),
    })
}

/// The picture layer `edit` adds, by id.
pub(super) fn picture_id(edit: &Edit) -> Option<Id> {
    (edit.ops.iter()).find_map(|op| match op {
        Op::AddItem { item, .. } if matches!(item.content, ItemContent::Image { .. }) => {
            Some(item.id.clone())
        }
        _ => None,
    })
}

/// One of a short's own sound effects (`cues::Own`: a rise under the hook, the whooshes where the speaker changes) as
/// its own change, labelled as `addSound`'s; None when none of its moments fits.
pub(super) fn own_sound(c: &Compiler, work: &Project, own: &cues::Own) -> Option<Edit> {
    // One the project has there already, from this message or an earlier one, isn't added again.
    let there = |now: Us| {
        (work.timeline.tracks.iter())
            .flat_map(|t| &t.items)
            .any(|item| {
                matches!(&item.content, ItemContent::Audio { asset_id, .. } if *asset_id == own.asset.id)
                    && item.start_us.abs_diff(now) <= TOLERANCE_US.unsigned_abs()
            })
    };
    let starts: Vec<Us> = (own.at.iter())
        .filter_map(|&t| c.map.time(work, t))
        .filter(|&now| video_duration(&work.timeline).saturating_sub(now) >= MIN_DURATION_US)
        .filter(|&now| !there(now))
        .collect();
    if starts.is_empty() {
        return None;
    }
    let ops = sounds_at(work, &own.asset, &starts).ok()?;
    Some(Edit {
        ops,
        label: label(
            "addSound",
            vec![
                ("name", ErrorParam::from(own.name.as_str())),
                ("count", ErrorParam::from(starts.len())),
            ],
        ),
    })
}

/// The ops that play `asset` at each of `starts` of `work`: the file once (when the project hasn't got it), then a
/// sound at each moment as the library's + puts one (`add_sound_ops`).
fn sounds_at(work: &Project, asset: &Asset, starts: &[Us]) -> Result<Vec<Op>, Refused> {
    let mut ops = Vec::with_capacity(starts.len() + 1);
    if starts.is_empty() {
        return Ok(ops);
    }
    let with_file = if work.asset(&asset.id).is_none() {
        let add = Op::AddAssets {
            assets: vec![asset.clone()],
        };
        let with_file = apply_ops(work, std::slice::from_ref(&add))
            .map_err(|e| to_model(format!("addSound: {e}")))?;
        ops.push(add);
        Cow::Owned(with_file)
    } else {
        Cow::Borrowed(work)
    };
    for &at in starts {
        let sound =
            add_sound_ops(&with_file, asset, at).map_err(|e| to_model(format!("addSound: {e}")))?;
        ops.extend(sound);
    }
    Ok(ops)
}

/// The short's hook `edit`, held at most `cues::HOOK_MAX_US` (a cut ends it sooner, as its ripple shortens a layer
/// that runs past it): sound-off viewers read the captions, which wait while it shows in its 9:16 band
/// (`captions::hook_holds_captions`).
pub(super) fn short_hook(edit: Edit) -> Edit {
    let len = (edit.ops.iter())
        .find_map(|op| match op {
            Op::AddItem { item, .. } => Some(item.duration_us),
            _ => None,
        })
        .unwrap_or(cues::HOOK_MAX_US)
        .min(cues::HOOK_MAX_US);
    Edit {
        ops: lasting(edit.ops, len),
        label: edit.label,
    }
}

/// `makeTitleCard`'s title, an addition on the additions' working copy, and its layer's id for the background's job.
pub(super) fn make_title_card(
    c: &Compiler,
    work: &Project,
    title: &str,
    (at, duration): (Option<Us>, Option<Us>),
) -> Result<(Edit, Id), Refused> {
    let words = if c.by_app {
        apps_words(("makeTitleCard", "title"), title)?
    } else {
        persons_words(&c.inputs.message, title).ok_or_else(|| {
            to_model(format!(
                "makeTitleCard: the title must be the person's own words, and they did not write {title:?}. If they \
                 gave no title, ask them for it with askUser."
            ))
        })?
    };
    let start = start(c, work, "makeTitleCard", "atSec", At::of(&at, None))?;
    let (ops, title_id) = title_ops(
        work,
        TitleTemplate::TitleCard,
        ("makeTitleCard", "title"),
        &words,
        (start, duration),
    )?;
    let edit = Edit {
        ops,
        label: label(
            "makeTitleCard",
            vec![("title", ErrorParam::from(words.as_str()))],
        ),
    };
    Ok((edit, title_id))
}

/// The background's job for the title layer `title_id`, in the primary format as the message's additions leave it
/// (`with_additions`).
pub(super) fn title_background(prompt: &str, with_additions: &Project, title_id: Id) -> Job {
    Job::MakeTitleBackground {
        prompt: sanitize::reply(prompt),
        format: with_additions.settings.primary_format,
        title_id,
    }
}

/// Words for the screen, as `addText` takes them: an app's own, cleaned (`apps_words`); chat's the person's own or said
/// in the video, else refused for `tool`'s `field`.
pub(super) fn screen_words(
    c: &Compiler,
    (tool, field): (&str, &str),
    text: &str,
) -> Result<String, Refused> {
    if c.by_app {
        return apps_words((tool, field), text);
    }
    (persons_words(&c.inputs.message, text)
        .or_else(|| said_in_video(c, text))
        .or_else(|| hook_from_video(c, text)))
    .ok_or_else(|| {
        to_model(format!(
            "{tool}: the words must be the person's own or said in the video, and {text:?} is neither. If they gave \
             none, quote the video or ask them with askUser."
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_persons_words_are_taken_as_written() {
        let m = "put a title 'Day 1 in Lisbon' for 3 seconds";
        assert_eq!(
            persons_words(m, "Day 1 in Lisbon").as_deref(),
            Some("Day 1 in Lisbon")
        );
        assert_eq!(
            persons_words(
                "call it \u{201c}Day one: Lisbon!\u{201d}, ok",
                "day one Lisbon"
            )
            .as_deref(),
            Some("Day one: Lisbon!")
        );
        assert_eq!(
            persons_words("say day 1 in lisbon.", "Day 1 in Lisbon").as_deref(),
            Some("day 1 in lisbon")
        );
        assert_eq!(persons_words(m, "Day 2 in Lisbon"), None);
        assert_eq!(persons_words(m, ""), None);
        assert_eq!(persons_words("Lisbon", "Day 1 in Lisbon"), None);
    }

    /// Hindi, Tamil and Chinese words are found as written, their own marks kept and the sentence's mark left out.
    #[test]
    fn the_persons_words_are_found_in_any_script() {
        assert_eq!(
            persons_words("आज हम वीडियो एडिटिंग सीखेंगे।", "वीडियो एडिटिंग सीखेंगे").as_deref(),
            Some("वीडियो एडिटिंग सीखेंगे")
        );
        assert_eq!(
            persons_words("வணக்கம் நண்பர்களே, இன்று", "வணக்கம் நண்பர்களே").as_deref(),
            Some("வணக்கம் நண்பர்களே")
        );
        assert_eq!(
            persons_words("大家好，今天我来教大家。", "今天我来").as_deref(),
            Some("今天我来")
        );
        assert_eq!(persons_words("大家好，今天我来教大家。", "明天"), None);
    }
}
