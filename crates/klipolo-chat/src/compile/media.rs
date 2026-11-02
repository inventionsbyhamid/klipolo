//! The media calls, on the additions' working copy: files from the library (`Inputs::library`, by name) placed as ops,
//! and pictures Muse Image draws first as jobs.
//! - `addMusic`: `media::music::add_music_ops` (30 %, ducking under speech, fades, to the video's end) from the time
//!   given, else the video's start: background music, wherever the playhead is. "The song" and the like are the
//!   person's one music file (`library::is_generic`).
//! - `addSound`: a sound at each time as the library's + puts one (`media::sound::add_sound_ops`: the whole file at full
//!   volume with no fades, attached to the clip under it, cut where the video ends); two times within
//!   `evidence::TOLERANCE_US` of each other are one sound, at the first.
//! - `addText`: the person's own words (`persons_words`; words of the model's own are refused back, for it to ask), or
//!   another app's as it gave them (`apps_words`), in one of the four text presets, placed by the model's
//!   `media::text::add_text_ops` exactly as the canvas's Add title and the library's presets place it: each format's
//!   own place, in 9:16 inside the area the apps keep clear.
//! - `addImage`: the picture as the library's + puts one (`media::image::add_image_ops_at`), at a size and in a corner of
//!   each format's own area (in 9:16 the area the apps keep clear) when one was asked for.
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
//! A time from the model stands on the evidence like any other (`Evidence::moment`: a change of shot or topic first),
//! and is then mapped into the working copy. Nothing is placed at the video's end or past it, and a layer that would
//! play past the end is cut there, so no media call makes the video longer. A file from the pack comes in with the ops
//! that first place it.

use std::borrow::Cow;
use std::collections::BTreeMap;

use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::Catalog;
use klipolo_model::ids::Id;
use klipolo_model::media::image::{IMAGE_US, add_image_ops_at};
use klipolo_model::media::music::{MusicOpts, add_music_ops};
use klipolo_model::media::sound::add_sound_ops;
use klipolo_model::media::text::{
    TEMPLATE_US, TEXT_US, TitleTemplate, add_template_ops, add_text_ops,
};
use klipolo_model::motion::{self, Motion};
use klipolo_model::ops::Op;
use klipolo_model::params::{ParamKind, ParamValue};
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::query::{anchor_at, main_track_duration, resolve_attach, video_duration};
use klipolo_model::speech::{WordKind, is_mark, joins_unspaced, word_ranges};
use klipolo_model::text_safety::clean_shown;
use klipolo_model::time::{MIN_DURATION_US, Us};
use klipolo_model::timeline::ItemContent;
use klipolo_model::validate::MAX_TEXT;
use klipolo_model::{Project, apply_ops};

use super::{
    Compiler, Edit, Job, MakeSticker, Refused, animate, clear, label, to_model, unfounded,
};
use crate::context::{secs, within_video};
use crate::evidence::TOLERANCE_US;
use crate::library::{Found, LibraryItem, MediaKind, is_generic};
use crate::sanitize;
use crate::tools::{Entrance, PictureSize, Position, TextPreset, ToolCall};

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

/// Where a media call's time plays in `work`: the playhead when the model gave none, else its time standing on the
/// evidence (`Evidence::moment`) or refused, naming `field`. A time past the main track's end keeps its distance from
/// it. Nothing starts at the video's end or past it (refused, so no layer ever makes the video longer).
pub(super) fn start(
    c: &Compiler,
    work: &Project,
    tool: &str,
    field: &str,
    at: Option<Us>,
) -> Result<Us, Refused> {
    let said = match at {
        None => within_video(c.snapshot, &c.inputs.view).0,
        Some(t) => (c.evidence.moment(t)).ok_or_else(|| to_model(unfounded(tool, field, t)))?,
    };
    let snapshot_end = main_track_duration(&c.snapshot.timeline.main);
    let now = match c.map.time(work, said) {
        Some(now) => now,
        None if said >= snapshot_end => {
            main_track_duration(&work.timeline.main).saturating_add(said - snapshot_end)
        }
        None => {
            return Err(to_model(format!(
                "{tool}: {field} {} s was taken out by an earlier change in this answer.",
                secs(said)
            )));
        }
    };
    if video_duration(&work.timeline).saturating_sub(now) < MIN_DURATION_US {
        let what = match at {
            Some(_) => format!("{field} {}", secs(said)),
            None => format!("the playhead ({} s)", secs(said)),
        };
        return Err(to_model(format!(
            "{tool}: {what} is at or past the video's end, where nothing can be added."
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
        _ => start(c, work, "addMusic", "atSec", at)?,
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

pub(super) fn add_sound(
    c: &Compiler,
    work: &Project,
    name: &str,
    times: &[Us],
) -> Result<Edit, Refused> {
    let item = pick(c, "addSound", name, &[MediaKind::Sound])?;
    let asset = &item.asset;
    if !asset.has_audio || asset.duration_us < MIN_DURATION_US {
        return Err(to_model(format!(
            "addSound: {:?} has no sound to play.",
            item.name
        )));
    }
    let mut starts: Vec<Us> = Vec::with_capacity(times.len());
    for (k, &t) in times.iter().enumerate() {
        let field = format!("atSec[{k}]");
        let at = start(c, work, "addSound", &field, Some(t))?;
        // Two times within the evidence's tolerance are one moment: one sound, at the first.
        if !starts
            .iter()
            .any(|&s| s.abs_diff(at) <= TOLERANCE_US.unsigned_abs())
        {
            starts.push(at);
        }
    }
    // The file comes in once, then a sound at each moment as the library's + puts one (`add_sound_ops`).
    let mut ops = Vec::with_capacity(starts.len() + 1);
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
    for &at in &starts {
        let sound =
            add_sound_ops(&with_file, asset, at).map_err(|e| to_model(format!("addSound: {e}")))?;
        ops.extend(sound);
    }
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
    (at, duration): (Option<Us>, Option<Us>),
    animation: Option<Entrance>,
    font: Option<&str>,
) -> Result<Edit, Refused> {
    let words = screen_words(c, ("addText", "text"), text)?;
    let start = start(c, work, "addText", "atSec", at)?;
    // Kinetic words are the kinetic title template, near the top whatever the preset, in its own font.
    if animation == Some(Entrance::Kinetic) {
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
    let mut ops = add_text_ops(work, preset.preset(), &words, start)
        .map_err(|e| to_model(format!("addText: {e}")))?;
    if let Some(animation) = animation {
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
    Ok(Edit {
        ops: lasting(ops, length(work, start, duration, TEXT_US)),
        label: label(
            "addText",
            vec![
                ("text", ErrorParam::from(words.as_str())),
                ("preset", ErrorParam::from(preset.key())),
            ],
        ),
    })
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

pub(super) fn add_image(
    c: &Compiler,
    work: &Project,
    file: &str,
    (size, position): (Option<PictureSize>, Option<Position>),
    (at, duration): (Option<Us>, Option<Us>),
) -> Result<Edit, Refused> {
    let item = pick(c, "addImage", file, &[MediaKind::Image])?;
    if item.asset.kind != AssetKind::Image {
        return Err(to_model(format!(
            "addImage: {:?} is not a picture.",
            item.name
        )));
    }
    let start = start(c, work, "addImage", "atSec", at)?;
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
    let placed = (job.size.share(), job.position.spot());
    clear::clear_of_person(p, &mut ops, asset, placed, start);
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
        let exit_only = m
            .as_ref()
            .is_some_and(|m| m.enter.is_none() && m.looping.is_none());
        let at = animate::sound_start(exit_only, start, duration, m.as_ref());
        let with_picture = apply_ops(p, &ops).ok()?;
        if let Ok(more) = add_sound_ops(&with_picture, sound, at) {
            ops.extend(more);
        }
    }
    Some(ops)
}

/// `makeSticker`'s job, on the project as the message's additions leave it (`with_additions`), whatever came before it
/// in the answer: where it shows, and the footage it shows over (`query::anchor_at`, as a layer attaches; None past
/// the main track's end).
pub(super) fn make_sticker(
    c: &Compiler,
    with_additions: &Project,
    prompt: &str,
    position: Option<Position>,
    (at, duration): (Option<Us>, Option<Us>),
) -> Result<Job, Refused> {
    let start = start(c, with_additions, "makeSticker", "atSec", at)?;
    Ok(Job::MakeSticker(MakeSticker {
        prompt: sanitize::reply(prompt),
        at_us: start,
        duration_us: length(with_additions, start, duration, IMAGE_US),
        position: position.unwrap_or(Position::Center),
        size: PictureSize::Medium,
        anchor: anchor_at(&with_additions.timeline.main, start),
        motion: None,
        sound: None,
    }))
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
    let start = start(c, work, "makeTitleCard", "atSec", at)?;
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
