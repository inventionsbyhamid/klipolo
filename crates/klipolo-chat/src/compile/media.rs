//! The media calls, on the additions' working copy: files from the library (`Inputs::library`, by name) placed as ops,
//! and pictures Muse Image draws first as jobs.
//! - `addMusic`: `media::music::add_music_ops` (30 %, ducking under speech, fades, to the video's end) from the time
//!   given, else the playhead. "The song" and the like are the person's one music file (`library::is_generic`).
//! - `addSound`: the whole file at full volume with no fades at each time, attached to the clip under it, cut where the
//!   video ends; two times on one moment are one sound.
//! - `addText`: a title template (`TitlePreset`, a scene preset) reading the words, its other words left empty so only
//!   they show, filling the frame in every format (`Layout::full_frame`). Words the person wrote are theirs as they
//!   wrote them (`persons_words`); others the model's without dashes (`sanitize::reply`).
//! - `addImage`: the picture at a size and a place in the frame (`picture_layout`).
//! - `makeSticker`: a `Job::MakeSticker` only; the app places the picture with `picture_ops` once it is drawn. A job's
//!   prompt names the picture in the library too, so it has no dashes either.
//! - `makeTitleCard`: the title card template with the person's exact title, applied at once, and a
//!   `Job::MakeTitleBackground` for the picture to go under it. No title of theirs: refused back, for the model to ask.
//!
//! A time from the model stands on the evidence like any other (`Evidence::moment`: a change of shot or topic first),
//! and is then mapped into the working copy. A file from the pack comes in with the ops that first place it.

use std::collections::BTreeMap;

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::Catalog;
use klipolo_model::ids::new_id;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::media::music::{MusicOpts, add_music_ops};
use klipolo_model::ops::{Op, TrackTarget};
use klipolo_model::params::{ParamKind, ParamValue};
use klipolo_model::project::{Asset, AssetKind, FormatKind};
use klipolo_model::query::{main_track_duration, video_duration};
use klipolo_model::time::{MIN_DURATION_US, Us, secs_to_us};
use klipolo_model::timeline::{Item, ItemContent};

use super::{Compiler, Edit, Job, Refused, label, to_model, unfounded};
use crate::context::{secs, within_video};
use crate::library::{Found, LibraryItem, MediaKind, is_generic};
use crate::sanitize;
use crate::tools::media::{PICTURE_SECS, TEXT_SECS};
use crate::tools::{PictureSize, Position, TitlePreset};

/// A sound effect's volume: full, to be heard over the voice.
pub const SOUND_VOLUME: f64 = 1.0;
/// Kept clear between a picture and the frame's edges: a share of the frame's shorter side.
pub const PICTURE_MARGIN: f64 = 0.04;
/// The most names a refusal lists.
const LISTED: usize = 12;
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
/// evidence (`Evidence::moment`) or refused, naming `field`. A time at the main track's end or past it is the working
/// copy's main track end.
fn start(
    c: &Compiler,
    work: &Project,
    tool: &str,
    field: &str,
    at: Option<Us>,
) -> Result<Us, Refused> {
    let at = match at {
        None => within_video(c.snapshot, &c.inputs.view).0,
        Some(t) => (c.evidence.moment(t)).ok_or_else(|| to_model(unfounded(tool, field, t)))?,
    };
    let past_the_clips = at >= main_track_duration(&c.snapshot.timeline.main);
    match c.map.time(work, at) {
        Some(now) => Ok(now),
        None if past_the_clips => Ok(main_track_duration(&work.timeline.main)),
        None => Err(to_model(format!(
            "{tool}: {field} {} s was taken out by an earlier change in this answer.",
            secs(at)
        ))),
    }
}

/// How long a layer from `start` shows in `work`: `wanted` (None: `default_secs`), cut where the video ends when it
/// starts before that. Refused when no time is left.
fn length(
    work: &Project,
    tool: &str,
    start: Us,
    wanted: Option<Us>,
    default_secs: f64,
) -> Result<Us, Refused> {
    let wanted = wanted.unwrap_or_else(|| secs_to_us(default_secs));
    let end = video_duration(&work.timeline);
    let got = if start < end {
        wanted.min(end - start)
    } else {
        wanted
    };
    if got < MIN_DURATION_US {
        return Err(to_model(format!(
            "{tool}: it would start as the video ends, with no time left to show it."
        )));
    }
    Ok(got)
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

/// The ops that add title template `preset` reading `text` (its other words left empty) from `start` for `duration`
/// (None: `TEXT_SECS`), filling the frame, attached to the clip under it; and the new layer's id. Refused when the
/// template holds fewer characters, said for `tool`'s `field`.
fn title_ops(
    work: &Project,
    (tool, field): (&str, &str),
    preset: TitlePreset,
    text: &str,
    start: Us,
    duration: Option<Us>,
) -> Result<(Vec<Op>, String, Us), Refused> {
    let source = preset.source();
    let def = (Catalog::builtin().get(&source)).ok_or_else(|| {
        to_model(format!(
            "{tool}: this build has no {} template.",
            preset.key()
        ))
    })?;
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
    let duration = length(work, tool, start, duration, TEXT_SECS)?;
    let id = new_id();
    let ops = vec![Op::AddItem {
        track: TrackTarget::Auto,
        attach: start < main_track_duration(&work.timeline.main),
        item: Item {
            id: id.clone(),
            start_us: start,
            duration_us: duration,
            attach: None,
            layout: Layout::full_frame(),
            content: ItemContent::generator(source, params),
            effects: Vec::new(),
        },
    }];
    Ok((ops, id, duration))
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
    let video_end = video_duration(&work.timeline);
    let mut starts: Vec<Us> = Vec::with_capacity(times.len());
    for (k, &t) in times.iter().enumerate() {
        let field = format!("atSec[{k}]");
        let at = start(c, work, "addSound", &field, Some(t))?;
        if video_end.saturating_sub(at) < MIN_DURATION_US {
            return Err(to_model(format!(
                "addSound: {field} {} s is where the video ends, so the sound would not be heard.",
                secs(t)
            )));
        }
        if !starts.contains(&at) {
            starts.push(at);
        }
    }
    let mut ops = Vec::with_capacity(starts.len() + 1);
    if work.asset(&asset.id).is_none() {
        ops.push(Op::AddAssets {
            assets: vec![asset.clone()],
        });
    }
    let main_end = main_track_duration(&work.timeline.main);
    for &at in &starts {
        ops.push(Op::AddItem {
            track: TrackTarget::Auto,
            attach: at < main_end,
            item: Item {
                id: new_id(),
                start_us: at,
                duration_us: asset.duration_us.min(video_end - at),
                attach: None,
                // Sound has no picture: the whole canvas, as every sound layer's box.
                layout: Layout::new(Anchor::TopLeft, BoxN::FULL),
                content: ItemContent::Audio {
                    asset_id: asset.id.clone(),
                    source_in_us: 0,
                    volume: SOUND_VOLUME,
                    muted: false,
                    fade_in_us: 0,
                    fade_out_us: 0,
                    effect: None,
                    drift_ppm: 0.0,
                    centre_from: None,
                    tie: None,
                    duck: None,
                },
                effects: Vec::new(),
            },
        });
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
    preset: TitlePreset,
    (at, duration): (Option<Us>, Option<Us>),
) -> Result<Edit, Refused> {
    let start = start(c, work, "addText", "atSec", at)?;
    let shown = persons_words(&c.inputs.message, text).unwrap_or_else(|| sanitize::reply(text));
    let (ops, _, _) = title_ops(work, ("addText", "text"), preset, &shown, start, duration)?;
    Ok(Edit {
        ops,
        label: label(
            "addText",
            vec![
                ("text", ErrorParam::from(shown.as_str())),
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
    let duration = length(work, "addImage", start, duration, PICTURE_SECS)?;
    Ok(Edit {
        ops: picture_ops(
            work,
            &item.asset,
            start,
            duration,
            size.unwrap_or(PictureSize::Medium),
            position.unwrap_or(Position::Center),
        ),
        label: label(
            "addImage",
            vec![("file", ErrorParam::from(item.name.as_str()))],
        ),
    })
}

pub(super) fn make_sticker(
    c: &Compiler,
    work: &Project,
    prompt: &str,
    position: Option<Position>,
    (at, duration): (Option<Us>, Option<Us>),
) -> Result<Job, Refused> {
    let start = start(c, work, "makeSticker", "atSec", at)?;
    let duration = length(work, "makeSticker", start, duration, PICTURE_SECS)?;
    Ok(Job::MakeSticker {
        prompt: sanitize::reply(prompt),
        at_us: start,
        duration_us: duration,
        position: position.unwrap_or(Position::Center),
        size: PictureSize::Medium,
    })
}

pub(super) fn make_title_card(
    c: &Compiler,
    work: &Project,
    (title, prompt): (&str, &str),
    (at, duration): (Option<Us>, Option<Us>),
) -> Result<(Edit, Job), Refused> {
    let Some(words) = persons_words(&c.inputs.message, title) else {
        return Err(to_model(format!(
            "makeTitleCard: the title must be the person's own words, and they did not write {title:?}. If they gave \
             no title, ask them for it with askUser."
        )));
    };
    let start = start(c, work, "makeTitleCard", "atSec", at)?;
    let (ops, title_id, duration) = title_ops(
        work,
        ("makeTitleCard", "title"),
        TitlePreset::TitleCard,
        &words,
        start,
        duration,
    )?;
    let job = Job::MakeTitleBackground {
        prompt: sanitize::reply(prompt),
        format: work.settings.primary_format,
        at_us: start,
        duration_us: duration,
        title_id,
    };
    let edit = Edit {
        ops,
        label: label(
            "makeTitleCard",
            vec![("title", ErrorParam::from(words.as_str()))],
        ),
    };
    Ok((edit, job))
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
