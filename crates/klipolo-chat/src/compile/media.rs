//! The media calls, on the additions' working copy: files from the library (`Inputs::library`, by name) placed as ops,
//! and pictures Muse Image draws first as jobs.
//! - `addMusic`: `media::music::add_music_ops` (30 %, ducking under speech, fades, to the video's end) from the time
//!   given, else the playhead. "The song" and the like are the person's one music file (`library::is_generic`).
//! - `addSound`: a sound at each time as the library's + puts one (`media::sound::add_sound_ops`: the whole file at full
//!   volume with no fades, attached to the clip under it, cut where the video ends); two times within
//!   `evidence::TOLERANCE_US` of each other are one sound, at the first.
//! - `addText`: the person's own words (`persons_words`; words of the model's own are refused back, for it to ask) in
//!   one of the four text presets, placed by the model's `media::text::add_text_ops` exactly as the canvas's Add title
//!   and the library's presets place it: each format's own place, in 9:16 inside the area the apps keep clear.
//! - `addImage`: the picture as the library's + puts one (`media::image::add_image_ops_at`), at a size and in a corner of
//!   each format's own area (in 9:16 the area the apps keep clear) when one was asked for.
//! - `makeSticker`: a `Job::MakeSticker` only, on the project as the message's additions leave it, anchored to the
//!   footage it shows over as a layer attaches (`Attach`); once the picture is drawn the app asks `sticker_placement`
//!   where and how long, on the project as it is then, and places it with `picture_ops`. A job's prompt names the
//!   picture in the library too, so it has no dashes either.
//! - `makeTitleCard`: the title card template as the canvas adds one (`media::text::add_template_ops`, filling the frame
//!   in every format) with the person's exact title (`persons_words`) and its other words left empty so only the
//!   title shows, for the template's own length (`TEMPLATE_US`) unless the person gave one, applied at once; and a
//!   `Job::MakeTitleBackground` for the picture to go under it, in the primary format as the additions leave it. No
//!   title of theirs: refused back, for the model to ask.
//!
//! A time from the model stands on the evidence like any other (`Evidence::moment`: a change of shot or topic first),
//! and is then mapped into the working copy. Nothing is placed at the video's end or past it, and a layer that would
//! play past the end is cut there, so no media call makes the video longer. A file from the pack comes in with the ops
//! that first place it.

use std::borrow::Cow;
use std::collections::BTreeMap;

use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::Catalog;
use klipolo_model::ids::{Id, new_id};
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::media::image::{IMAGE_US, add_image_ops_at};
use klipolo_model::media::music::{MusicOpts, add_music_ops};
use klipolo_model::media::sound::add_sound_ops;
use klipolo_model::media::text::{
    TEMPLATE_US, TEXT_US, TitleTemplate, add_template_ops, add_text_ops,
};
use klipolo_model::ops::{Op, TrackTarget};
use klipolo_model::params::{ParamKind, ParamValue};
use klipolo_model::project::{Asset, AssetKind, FormatKind};
use klipolo_model::query::{anchor_at, main_track_duration, resolve_attach, video_duration};
use klipolo_model::time::{MIN_DURATION_US, Us};
use klipolo_model::timeline::{Item, ItemContent};
use klipolo_model::{Project, apply_ops};

use super::{Compiler, Edit, Job, MakeSticker, Refused, label, to_model, unfounded};
use crate::context::{secs, within_video};
use crate::evidence::TOLERANCE_US;
use crate::library::{Found, LibraryItem, MediaKind, is_generic};
use crate::sanitize;
use crate::tools::{PictureSize, Position, TextPreset};

/// Kept clear between a picture and the frame's edges: a share of the frame's shorter side.
pub const PICTURE_MARGIN: f64 = 0.04;
/// The most names a refusal lists, then how many more there are: as many as Library shows of a kind.
const LISTED: usize = crate::context::LIBRARY_NAMES;
/// What may stand around a title the person quoted, left out of it.
const QUOTES: [char; 9] = ['\'', '"', '`', '‘', '’', '“', '”', '«', '»'];

/// The canvas of `format` in pixels.
fn canvas(format: FormatKind) -> (f64, f64) {
    let (w, h) = format.size();
    (f64::from(w), f64::from(h))
}

/// Where a `width` x `height` picture goes in `format`: `size`'s share of the frame's width with its shape kept (at
/// most as tall as the frame less the margins), at `position`: a corner `PICTURE_MARGIN` of the shorter side in from
/// its edges, or the middle. Other formats derive their place from its anchor (`resolve_rect`). An unknown size is
/// square.
pub fn picture_layout(
    width: u32,
    height: u32,
    size: PictureSize,
    position: Position,
    format: FormatKind,
) -> Layout {
    let (cw, ch) = canvas(format);
    let aspect = if width == 0 || height == 0 {
        1.0
    } else {
        f64::from(width) / f64::from(height)
    };
    let margin = PICTURE_MARGIN * cw.min(ch);
    let (mx, my) = (margin / cw, margin / ch);
    let mut w = size.share();
    let mut h = w * cw / aspect / ch;
    let tallest = 1.0 - 2.0 * my;
    if h > tallest {
        h = tallest;
        w = h * ch * aspect / cw;
    }
    let (x, y, anchor) = match position {
        Position::TopLeft => (mx, my, Anchor::TopLeft),
        Position::TopRight => (1.0 - mx - w, my, Anchor::TopRight),
        Position::BottomLeft => (mx, 1.0 - my - h, Anchor::BottomLeft),
        Position::BottomRight => (1.0 - mx - w, 1.0 - my - h, Anchor::BottomRight),
        Position::Center => ((1.0 - w) / 2.0, (1.0 - h) / 2.0, Anchor::Center),
    };
    Layout::new(anchor, BoxN { x, y, w, h })
}

/// The ops that show picture `asset` from `start_us` for `duration_us`, at `size` and `position` in the primary format
/// (`picture_layout`), on the first overlay track with room, attached to the clip under it (none past the main track's
/// end). The file comes in with it when the project doesn't have it yet. Also for a sticker once it is drawn
/// (`Job::MakeSticker`).
pub fn picture_ops(
    p: &Project,
    asset: &Asset,
    start_us: Us,
    duration_us: Us,
    size: PictureSize,
    position: Position,
) -> Vec<Op> {
    let mut ops = Vec::new();
    if p.asset(&asset.id).is_none() {
        ops.push(Op::AddAssets {
            assets: vec![asset.clone()],
        });
    }
    let layout = picture_layout(
        asset.width,
        asset.height,
        size,
        position,
        p.settings.primary_format,
    );
    ops.push(Op::AddItem {
        track: TrackTarget::Auto,
        attach: start_us < main_track_duration(&p.timeline.main),
        item: Item {
            id: new_id(),
            start_us,
            duration_us,
            attach: None,
            layout,
            content: ItemContent::Image {
                asset_id: asset.id.clone(),
            },
            effects: Vec::new(),
        },
    });
    ops
}

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
fn pick<'l>(
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
fn start(
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

/// `ops` with the layer they add lasting `duration_us` (the model's ops add one at its default length).
fn lasting(mut ops: Vec<Op>, duration_us: Us) -> Vec<Op> {
    for op in &mut ops {
        if let Op::AddItem { item, .. } = op {
            item.duration_us = duration_us;
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

fn said(text: &str) -> Vec<Said> {
    let mut out = Vec::new();
    let mut at = 0;
    for piece in text.split_whitespace() {
        let start = at + text[at..].find(piece).unwrap_or(0);
        let end = start + piece.len();
        at = end;
        let bare: String = piece
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        if !bare.is_empty() {
            out.push(Said { bare, start, end });
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
            QUOTES.contains(&c) || matches!(c, ',' | '.' | ';' | ':') || c.is_whitespace()
        });
    (!span.is_empty()).then(|| span.to_owned())
}

/// The ops that add the title card template reading `text` (its other words left empty) from `start` for `duration`
/// (None: `TEMPLATE_US`), as the canvas adds a title template (`media::text::add_template_ops`: filling the frame in
/// every format, attached to the clip under it); and the new layer's id. Refused when the template holds fewer
/// characters, said for `tool`'s `field`.
fn title_ops(
    work: &Project,
    (tool, field): (&str, &str),
    text: &str,
    start: Us,
    duration: Option<Us>,
) -> Result<(Vec<Op>, Id), Refused> {
    let template = TitleTemplate::TitleCard;
    let def = (Catalog::builtin().get(&template.source()))
        .ok_or_else(|| to_model(format!("{tool}: this build has no title card template.")))?;
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
    let start = start(c, work, "addMusic", "atSec", at)?;
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

pub(super) fn add_text(
    c: &Compiler,
    work: &Project,
    text: &str,
    preset: TextPreset,
    (at, duration): (Option<Us>, Option<Us>),
) -> Result<Edit, Refused> {
    let Some(words) = persons_words(&c.inputs.message, text) else {
        return Err(to_model(format!(
            "addText: the words must be the person's own, and they did not write {text:?}. If they gave none, ask \
             them with askUser."
        )));
    };
    let start = start(c, work, "addText", "atSec", at)?;
    let ops = add_text_ops(work, preset.preset(), &words, start)
        .map_err(|e| to_model(format!("addText: {e}")))?;
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
    let ops = add_image_ops_at(work, &item.asset, start, size.share(), spot)
        .map_err(|e| to_model(format!("addImage: {e}")))?;
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
    }))
}

/// `makeTitleCard`'s title, an addition on the additions' working copy, and its layer's id for the background's job.
pub(super) fn make_title_card(
    c: &Compiler,
    work: &Project,
    title: &str,
    (at, duration): (Option<Us>, Option<Us>),
) -> Result<(Edit, Id), Refused> {
    let Some(words) = persons_words(&c.inputs.message, title) else {
        return Err(to_model(format!(
            "makeTitleCard: the title must be the person's own words, and they did not write {title:?}. If they gave \
             no title, ask them for it with askUser."
        )));
    };
    let start = start(c, work, "makeTitleCard", "atSec", at)?;
    let (ops, title_id) = title_ops(work, ("makeTitleCard", "title"), &words, start, duration)?;
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
}
