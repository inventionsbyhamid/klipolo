//! Where a picture chat puts at a top corner goes in 9:16, so it stays off the person on screen: a short's stickers
//! sit over a talking head, and the head fills the middle of the vertical frame.
//!
//! The person is where the clip's Fill crop follows them (`FillFocus::Speaker`, from `followSubject`): their head and
//! upper body, a box `PERSON_HALF_W` either side of where the path puts them in the 9:16 frame and `PERSON_UP` above to
//! `PERSON_DOWN` below. Of the two top corners, the picture takes the one it overlaps less (the one asked for when
//! they tie), at its size when that one is clear; when neither is, it shrinks to `CLEAR_SHARE` of the width and moves
//! out to the frame's edge. Without follow data it does that at once, in the corner asked for. Only the 9:16 box
//! changes; a picture in the middle, at a bottom corner, or in a project with no 9:16 version is left as placed.
//!
//! A short's kinetic hook keeps off the face in 9:16 too, where the 9:16 version shows a face (`face_in_portrait`): a
//! 9:16 Fill crop of camera footage of people shows the source's full height, so the face sits in `FACE_ZONE`
//! whatever the shot (high in a seated wide shot, down to the chin in a close-up), and the kinetic title's words,
//! near the top, land on the eyes. The hook goes `UNDER_FACE` there (`hook_under_face`): below the face, above the
//! captions, inside the clear area. Letterboxed 9:16 (Fit), screen recordings, footage with nobody found and the other
//! formats keep the kinetic title as it is, and other words go where their preset puts them.
//!
//! A short's sticker goes beside the speaker's head instead (`beside_speaker`), as `cues` times it: at the head's
//! height, on the side with more room, never on the head or, where nothing says where the speaker is, the face zone.

use std::collections::HashMap;

use klipolo_model::Project;
use klipolo_model::fill_focus::FillFocus;
use klipolo_model::framing::effective_framing;
use klipolo_model::layout::{Anchor, BoxN};
use klipolo_model::media::image::{IMAGE_MARGIN, Spot, corner_rect};
use klipolo_model::ops::Op;
use klipolo_model::project::{Asset, AssetKind, FormatKind};
use klipolo_model::query::clip_at;
use klipolo_model::time::Us;
use klipolo_model::timeline::{ClipSource, FramingMode, ItemContent};

use crate::context::SceneLog;

/// A picture's share of the width where no corner is clear of the person.
pub const CLEAR_SHARE: f64 = 0.28;
/// The person's box around where the path puts them, in shares of the 9:16 frame: half its width, and how far it
/// reaches above and below.
const PERSON_HALF_W: f64 = 0.25;
const PERSON_UP: f64 = 0.3;
const PERSON_DOWN: f64 = 0.35;

/// The 9:16 frame's width over its height.
const PORTRAIT_ASPECT: f64 = 9.0 / 16.0;

/// Where a face is in a 9:16 crop of a talk, in shares of the frame: across the middle, from near the top to just below
/// the middle.
pub const FACE_ZONE: BoxN = BoxN {
    x: 0.2,
    y: 0.08,
    w: 0.6,
    h: 0.38,
};
/// Where the hook goes in 9:16 instead of over the face: under it, above the captions, inside the apps' clear area
/// (`captions::HOOK_BAND`); while it shows there, the 9:16 captions wait (`captions::hook_holds_captions`).
pub const UNDER_FACE: BoxN = klipolo_model::captions::HOOK_BAND;

/// The width and height of the picture clip `i` of `p` shows: its file, or a recording's primary source's first file.
fn picture_size(p: &Project, source: &ClipSource) -> Option<(f64, f64)> {
    let asset = match source {
        ClipSource::Asset { id } => p.asset(id)?,
        ClipSource::Recording { id } => {
            let r = p.recording(id)?;
            let run = r.primary_source()?.runs.first()?;
            p.asset(&run.asset_id)?
        }
    };
    (asset.width > 0 && asset.height > 0).then(|| (f64::from(asset.width), f64::from(asset.height)))
}

/// The person's box in the 9:16 frame at timeline `at_us` on `p`, where the clip there follows them in 9:16; None
/// without follow data.
fn person_box(p: &Project, at_us: Us) -> Option<BoxN> {
    let (i, start) = clip_at(&p.timeline.main, at_us)?;
    let clip = &p.timeline.main[i];
    let FillFocus::Speaker { path, .. } = clip.fill_focus.get(&FormatKind::Portrait)? else {
        return None;
    };
    let source_us = clip.source_in_us + ((at_us - start) as f64 * clip.speed) as Us;
    let (sx, sy) = path.centre_at(source_us)?;
    let (w, h) = picture_size(p, &clip.source)?;
    // Fill crops the picture to 9:16 around the path's point, kept inside the picture.
    let (fx, fy) = if w / h > PORTRAIT_ASPECT {
        let crop = PORTRAIT_ASPECT / (w / h);
        let left = sx.clamp(crop / 2.0, 1.0 - crop / 2.0) - crop / 2.0;
        ((sx - left) / crop, sy)
    } else {
        let crop = (w / h) / PORTRAIT_ASPECT;
        let top = sy.clamp(crop / 2.0, 1.0 - crop / 2.0) - crop / 2.0;
        (sx, (sy - top) / crop)
    };
    Some(BoxN {
        x: fx - PERSON_HALF_W,
        y: fy - PERSON_UP,
        w: 2.0 * PERSON_HALF_W,
        h: PERSON_UP + PERSON_DOWN,
    })
}

/// How much of `a` and `b` overlap, in shares of the frame squared.
fn overlap(a: BoxN, b: BoxN) -> f64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    w.max(0.0) * h.max(0.0)
}

/// `asset` at `CLEAR_SHARE` in `spot`'s top corner of 9:16, out at the frame's edge (a margin of `IMAGE_MARGIN` of the
/// width), as high as a corner picture sits.
fn at_edge(asset: &Asset, spot: Spot) -> BoxN {
    let mut b = corner_rect(
        asset.width,
        asset.height,
        CLEAR_SHARE,
        spot,
        FormatKind::Portrait,
    );
    let margin = IMAGE_MARGIN;
    b.x = match spot {
        Spot::TopLeft => margin,
        _ => 1.0 - margin - b.w,
    };
    b
}

/// `ops`' picture layer of `asset` (placed by the library's rules at `share` in `spot`, from timeline `at_us`) moved in
/// 9:16 clear of the person, as the module says.
pub(super) fn clear_of_person(
    p: &Project,
    ops: &mut [Op],
    asset: &Asset,
    (share, spot): (f64, Spot),
    at_us: Us,
) {
    if !p.settings.formats.contains(&FormatKind::Portrait)
        || !matches!(spot, Spot::TopLeft | Spot::TopRight)
    {
        return;
    }
    let other = if spot == Spot::TopLeft {
        Spot::TopRight
    } else {
        Spot::TopLeft
    };
    let (rect, corner) = match person_box(p, at_us) {
        Some(person) => {
            let asked = corner_rect(asset.width, asset.height, share, spot, FormatKind::Portrait);
            let flipped = corner_rect(
                asset.width,
                asset.height,
                share,
                other,
                FormatKind::Portrait,
            );
            let (best, corner) = if overlap(flipped, person) < overlap(asked, person) {
                (flipped, other)
            } else {
                (asked, spot)
            };
            if overlap(best, person) > 0.0 {
                (at_edge(asset, corner), corner)
            } else {
                (best, corner)
            }
        }
        None => (at_edge(asset, spot), spot),
    };
    let primary = p.settings.primary_format;
    for op in ops {
        let Op::AddItem { item, .. } = op else {
            continue;
        };
        if !matches!(item.content, ItemContent::Image { .. }) {
            continue;
        }
        if primary == FormatKind::Portrait {
            item.layout.rect = rect;
            item.layout.anchor = if corner == Spot::TopLeft {
                Anchor::TopLeft
            } else {
                Anchor::TopRight
            };
        } else {
            item.layout.overrides.insert(FormatKind::Portrait, rect);
        }
    }
}

/// Whether the 9:16 version shows a face where words at timeline `at_us` of `p` would sit: the clip there is camera
/// footage (an imported video, or a take with no screen) that Fill crops in 9:16, and people are found in it: its 9:16
/// crop follows a speaker, the answer follows someone (`follows`), or its scene log says who is on screen.
pub(super) fn face_in_portrait(
    p: &Project,
    at_us: Us,
    follows: bool,
    scenes: &HashMap<String, SceneLog>,
) -> bool {
    let Some((i, _)) = clip_at(&p.timeline.main, at_us) else {
        return false;
    };
    let clip = &p.timeline.main[i];
    let footage = match &clip.source {
        ClipSource::Asset { id } => (p.asset(id))
            .is_some_and(|a| a.kind == AssetKind::Video)
            .then_some(id.as_str()),
        ClipSource::Recording { id } => match p.recording(id) {
            Some(r) if r.screen().is_none() => Some(""),
            _ => None,
        },
    };
    let Some(asset) = footage else {
        return false;
    };
    if effective_framing(p, clip, FormatKind::Portrait).mode != FramingMode::Fill {
        return false;
    }
    let followed = matches!(
        clip.fill_focus.get(&FormatKind::Portrait),
        Some(FillFocus::Speaker { .. })
    );
    let named = (scenes.get(asset))
        .is_some_and(|log| (log.items.iter()).any(|item| !item.who.trim().is_empty()));
    followed || follows || named
}

/// `ops`' layers placed `UNDER_FACE` in 9:16: their own box where 9:16 is `p`'s main format, else their 9:16 place.
pub(super) fn hook_under_face(p: &Project, ops: &mut [Op]) {
    for op in ops {
        let Op::AddItem { item, .. } = op else {
            continue;
        };
        if p.settings.primary_format == FormatKind::Portrait {
            item.layout.rect = UNDER_FACE;
            item.layout.anchor = Anchor::TopCenter;
        } else {
            item.layout
                .overrides
                .insert(FormatKind::Portrait, UNDER_FACE);
        }
    }
}

/// Which side of the speaker a short's sticker goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// The other side.
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// A short's sticker in shares of the frame's height (every format shows a 16:9 picture's full height, so a head is as
/// tall in each): 0.1125 is 20 % of the 9:16 frame's width for a square picture, where the speaker's head is known.
pub const STICKER_H: f64 = 0.1125;
/// Its height where only `FACE_ZONE` is known: 18 % of the 9:16 width, which fits beside the zone.
const ZONE_STICKER_H: f64 = 0.10125;
/// The smallest it shrinks to, to stay off the head: 15 % of the 9:16 width.
const MIN_STICKER_H: f64 = 0.084;
/// Half a followed head's width, beside which a sticker goes, in shares of the frame's height (22 % of the 9:16 width
/// each side of where the follow keeps the speaker: their head, and where the crop lets them drift before it moves).
pub const HEAD_HALF_H: f64 = 0.124;
/// Where a head is down the frame: a sticker's middle sits at this share of the frame's height.
pub const HEAD_Y: f64 = 0.22;
/// Kept clear at the frame's edges and beside the head, in shares of the frame's shorter side.
const EDGE: f64 = 0.04;
const GAP: f64 = 0.02;
/// Kept clear of `FACE_ZONE`, which has room around the face already.
const ZONE_GAP: f64 = 0.002;

/// A format's frame in pixels (`resolve_rect`'s sizes).
fn frame(format: FormatKind) -> (f64, f64) {
    match format {
        FormatKind::Landscape => (1920.0, 1080.0),
        FormatKind::Portrait => (1080.0, 1920.0),
        FormatKind::Square => (1080.0, 1080.0),
    }
}

/// Where the speaker is across `format`'s frame at timeline `at_us` of `p` (the clip there Fill crops its picture in
/// that format), and across the picture when a follow's path says: from the clip's path in that format; or from its
/// path in another where that format shows the whole picture's width; or the middle where this answer follows the
/// speaker (`follows`) and the format crops the picture, as the follow will keep them there. None when nothing says.
fn speaker_at(
    p: &Project,
    at_us: Us,
    format: FormatKind,
    follows: bool,
) -> Option<(f64, Option<f64>)> {
    let (i, start) = clip_at(&p.timeline.main, at_us)?;
    let clip = &p.timeline.main[i];
    if effective_framing(p, clip, format).mode != FramingMode::Fill {
        return None;
    }
    let (w, h) = picture_size(p, &clip.source)?;
    let (fw, fh) = frame(format);
    // The share of the picture's width the frame shows.
    let crop = ((fw / fh) / (w / h)).min(1.0);
    let source_us = clip.source_in_us + ((at_us - start) as f64 * clip.speed) as Us;
    let path_x = |f: FormatKind| match clip.fill_focus.get(&f) {
        Some(FillFocus::Speaker { path, .. }) => path.centre_at(source_us).map(|(x, _)| x),
        _ => None,
    };
    let own = path_x(format);
    let any = own.or_else(|| {
        [FormatKind::Portrait, FormatKind::Square]
            .into_iter()
            .find_map(path_x)
    });
    match (own, any) {
        (Some(sx), _) => {
            let left = sx.clamp(crop / 2.0, 1.0 - crop / 2.0) - crop / 2.0;
            Some(((sx - left) / crop, Some(sx)))
        }
        (None, Some(sx)) if crop >= 0.999 => Some((sx, Some(sx))),
        _ if follows && crop < 0.999 => Some((0.5, None)),
        _ => None,
    }
}

/// Where a short's sticker of `asset` goes in each of `p`'s formats at timeline `at_us`: beside the speaker's head, at
/// its height, on the side of the frame with more room (`prefer` when the two are alike; with a follow's path, the
/// side toward the picture's nearer edge, where a group's outside is), never on the head. The head is where the follow
/// keeps the speaker (`speaker_at`), `HEAD_HALF_H` either side, and the sticker goes as near it as the frame's edge
/// allows. Where nothing says where the speaker is, the face may be anywhere in `FACE_ZONE` across: the sticker is a
/// little smaller and sits out at the frame's edge, clear of the zone. Sizes are shares of the frame's height, so it
/// stands as tall beside the head in every format; the hook and the captions sit lower (from 0.465 in 9:16). Other
/// formats take the primary format's side. The boxes by format, and the side chosen in the primary format.
pub(super) fn beside_speaker(
    p: &Project,
    at_us: Us,
    asset: &Asset,
    follows: bool,
    prefer: Side,
) -> (Vec<(FormatKind, BoxN)>, Side) {
    let aspect = if asset.width > 0 && asset.height > 0 {
        f64::from(asset.width) / f64::from(asset.height)
    } else {
        1.0
    };
    let primary = p.settings.primary_format;
    let mut formats = p.settings.formats.clone();
    formats.sort_by_key(|f| *f != primary);
    let mut out = Vec::new();
    let mut chosen = prefer;
    for format in formats {
        let (fw, fh) = frame(format);
        let short = fw.min(fh);
        let speaker = speaker_at(p, at_us, format, follows);
        // The head across the frame, how tall the sticker is, how far from the edge and from the head it keeps.
        let (head, tall, edge, gap) = match speaker {
            Some((x, _)) => {
                let half = HEAD_HALF_H * fh / fw;
                (
                    (x - half, x + half),
                    STICKER_H,
                    EDGE * short / fw,
                    GAP * short / fw,
                )
            }
            None => (
                (FACE_ZONE.x, FACE_ZONE.x + FACE_ZONE.w),
                ZONE_STICKER_H,
                GAP * short / fw,
                ZONE_GAP * short / fw,
            ),
        };
        let (room_l, room_r) = (head.0 - gap - edge, 1.0 - edge - gap - head.1);
        let side = if format != primary {
            chosen
        } else if (room_l - room_r).abs() > 0.02 {
            if room_l > room_r {
                Side::Left
            } else {
                Side::Right
            }
        } else {
            match speaker.and_then(|(_, sx)| sx) {
                Some(sx) if sx < 0.5 => Side::Left,
                Some(_) => Side::Right,
                None => prefer,
            }
        };
        // As tall as asked where that fits the room on `side`, else as tall as the room lets it be; None under
        // `MIN_STICKER_H`.
        let fit = |side: Side| {
            let room = match side {
                Side::Left => room_l,
                Side::Right => room_r,
            };
            let h = tall.min(room * fw / (fh * aspect));
            (h >= MIN_STICKER_H - 1e-9).then(|| {
                let w = h * fh * aspect / fw;
                let x = match (side, speaker) {
                    (Side::Left, Some(_)) => (head.0 - gap - w).max(edge),
                    (Side::Right, Some(_)) => (head.1 + gap).min(1.0 - edge - w),
                    (Side::Left, None) => edge,
                    (Side::Right, None) => 1.0 - edge - w,
                };
                BoxN {
                    x,
                    y: HEAD_Y - h / 2.0,
                    w,
                    h,
                }
            })
        };
        let (rect, side) = match fit(side) {
            Some(b) => (b, side),
            None => match fit(side.other()) {
                Some(b) => (b, side.other()),
                // No room beside the head either side: out at the frame's edge, at the least size.
                None => {
                    let w = MIN_STICKER_H * fh * aspect / fw;
                    let x = if side == Side::Left {
                        edge
                    } else {
                        1.0 - edge - w
                    };
                    let b = BoxN {
                        x,
                        y: HEAD_Y - MIN_STICKER_H / 2.0,
                        w,
                        h: MIN_STICKER_H,
                    };
                    (b, side)
                }
            },
        };
        if format == primary {
            chosen = side;
        }
        out.push((format, rect));
    }
    (out, chosen)
}

/// `ops`' picture layer placed as `beside_speaker` says: its own box in the primary format, the others as overrides.
pub(super) fn place_beside(p: &Project, ops: &mut [Op], boxes: &[(FormatKind, BoxN)]) {
    let primary = p.settings.primary_format;
    for op in ops {
        let Op::AddItem { item, .. } = op else {
            continue;
        };
        if !matches!(item.content, ItemContent::Image { .. }) {
            continue;
        }
        item.layout.overrides.clear();
        for &(format, rect) in boxes {
            if format == primary {
                item.layout.rect = rect;
                item.layout.anchor = Anchor::Center;
            } else {
                item.layout.overrides.insert(format, rect);
            }
        }
    }
}
