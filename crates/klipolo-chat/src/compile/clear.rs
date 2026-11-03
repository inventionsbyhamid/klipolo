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
/// Where the hook goes in 9:16 instead of over the face: under it, above the captions (word pop's are centred at 0.69),
/// inside the apps' clear area (`media::text::PORTRAIT_CLEAR`). Two lines of the hook's 130 pixels fit its height.
pub const UNDER_FACE: BoxN = BoxN {
    x: 0.06,
    y: 0.465,
    w: 0.81,
    h: 0.155,
};

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
            .then(|| id.as_str()),
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
