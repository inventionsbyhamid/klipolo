//! Pictures put over the video (a logo, a sticker): a medium size in the middle of the frame for 5 s from a moment,
//! moving with the clip they start over. Other formats derive their place from the primary's (`resolve_rect`: sizes
//! follow the shorter side, the centre stays centred).
//!
//! Chat places a picture at a size and in a corner too ([`add_image_ops_at`]). A corner is each format's own: an override
//! for every format besides the primary, in that format's area (in 9:16 what the apps' own buttons and captions leave
//! clear, [`PORTRAIT_CLEAR`], as the text presets keep to), at the same share of that format's width. A corner derived
//! from 16:9 would keep its 16:9 margins and sit under TikTok's tabs. The middle stays as the library places it.

use crate::error::{invalid, Result};
use crate::ids::new_id;
use crate::layout::{Anchor, BoxN, Layout};
use crate::messages::code;
use crate::ops::{Op, TrackTarget};
use crate::project::{Asset, AssetKind, FormatKind, Project};
use crate::query::main_track_duration;
use crate::time::Us;
use crate::timeline::{Item, ItemContent};

use super::text::PORTRAIT_CLEAR;

/// "Medium": the picture is this share of the canvas's width, keeping its shape.
pub const IMAGE_SIZE: f64 = 0.35;
/// Kept clear around a picture that would be taller than the frame: a share of each side.
pub const IMAGE_MARGIN: f64 = 0.04;
/// How long a picture shows when it is added.
pub const IMAGE_US: Us = 5_000_000;

/// The canvas size of `format` in pixels (`resolve_rect`'s sizes).
fn canvas(format: FormatKind) -> (f64, f64) {
    match format {
        FormatKind::Landscape => (1920.0, 1080.0),
        FormatKind::Portrait => (1080.0, 1920.0),
        FormatKind::Square => (1080.0, 1080.0),
    }
}

/// Where in the frame a picture sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// A picture's width over its height; an unknown size is square.
fn aspect(width: u32, height: u32) -> f64 {
    if width == 0 || height == 0 {
        1.0
    } else {
        f64::from(width) / f64::from(height)
    }
}

/// Where a `width` x `height` picture goes in `format`: `IMAGE_SIZE` of the width, centred, its shape kept; one too tall
/// for that is as tall as the frame less the margins. An unknown size is square.
pub fn image_rect(width: u32, height: u32, format: FormatKind) -> BoxN {
    centred_rect(width, height, IMAGE_SIZE, format)
}

/// [`image_rect`] at `share` of the width.
pub fn centred_rect(width: u32, height: u32, share: f64, format: FormatKind) -> BoxN {
    let (cw, ch) = canvas(format);
    let aspect = aspect(width, height);
    let mut w = share;
    let mut h = w * cw / aspect / ch;
    let tallest = 1.0 - 2.0 * IMAGE_MARGIN;
    if h > tallest {
        h = tallest;
        w = h * ch * aspect / cw;
    }
    BoxN {
        x: (1.0 - w) / 2.0,
        y: (1.0 - h) / 2.0,
        w,
        h,
    }
}

/// The area a picture in a corner keeps to in `format`: the whole frame, or in 9:16 what the apps keep clear.
fn area(format: FormatKind) -> BoxN {
    match format {
        FormatKind::Portrait => PORTRAIT_CLEAR,
        _ => BoxN::FULL,
    }
}

/// Where a `width` x `height` picture goes in corner `spot` of `format` (the middle for [`Spot::Center`], as
/// [`centred_rect`]): `share` of the frame's width with its shape kept, no bigger than the format's area (`area`) less
/// its margins, `IMAGE_MARGIN` of the frame's shorter side in from the area's edges.
pub fn corner_rect(width: u32, height: u32, share: f64, spot: Spot, format: FormatKind) -> BoxN {
    if spot == Spot::Center {
        return centred_rect(width, height, share, format);
    }
    let (cw, ch) = canvas(format);
    let a = area(format);
    let margin = IMAGE_MARGIN * cw.min(ch);
    let (ax, ay) = (a.x * cw + margin, a.y * ch + margin);
    let (aw, ah) = (a.w * cw - 2.0 * margin, a.h * ch - 2.0 * margin);
    let aspect = aspect(width, height);
    let mut w = (share * cw).min(aw);
    let mut h = w / aspect;
    if h > ah {
        h = ah;
        w = h * aspect;
    }
    let x = match spot {
        Spot::TopLeft | Spot::BottomLeft => ax,
        _ => ax + aw - w,
    };
    let y = match spot {
        Spot::TopLeft | Spot::TopRight => ay,
        _ => ay + ah - h,
    };
    BoxN {
        x: x / cw,
        y: y / ch,
        w: w / cw,
        h: h / ch,
    }
}

/// A `width` x `height` picture's layout at `share` of the width in `spot`, placed in `primary`: in the middle, the
/// centred rect there (the other formats derive theirs); in a corner, that corner of each format's own area, as an
/// override for every format besides `primary`.
pub fn image_layout(
    width: u32,
    height: u32,
    share: f64,
    spot: Spot,
    primary: FormatKind,
) -> Layout {
    let anchor = match spot {
        Spot::Center => Anchor::Center,
        Spot::TopLeft => Anchor::TopLeft,
        Spot::TopRight => Anchor::TopRight,
        Spot::BottomLeft => Anchor::BottomLeft,
        Spot::BottomRight => Anchor::BottomRight,
    };
    let mut layout = Layout::new(anchor, corner_rect(width, height, share, spot, primary));
    if spot != Spot::Center {
        for format in [
            FormatKind::Landscape,
            FormatKind::Portrait,
            FormatKind::Square,
        ] {
            if format != primary {
                let rect = corner_rect(width, height, share, spot, format);
                layout.overrides.insert(format, rect);
            }
        }
    }
    layout
}

/// The ops that add image `asset` at `at_us`: `IMAGE_US` long, centred at `image_rect` in the primary format, on the
/// first overlay track with room, attached to the clip under it (none past the main track's end). The file comes in
/// with it when the project doesn't have it yet. Refused for a file that isn't a picture.
pub fn add_image_ops(p: &Project, asset: &Asset, at_us: Us) -> Result<Vec<Op>> {
    add_image_ops_at(p, asset, at_us, IMAGE_SIZE, Spot::Center)
}

/// [`add_image_ops`] at `share` of the width in `spot` ([`image_layout`]).
pub fn add_image_ops_at(
    p: &Project,
    asset: &Asset,
    at_us: Us,
    share: f64,
    spot: Spot,
) -> Result<Vec<Op>> {
    if asset.kind != AssetKind::Image {
        return Err(invalid(code!("model.image.notImage")));
    }
    let start = at_us.max(0);
    let mut ops = Vec::new();
    if p.asset(&asset.id).is_none() {
        ops.push(Op::AddAssets {
            assets: vec![asset.clone()],
        });
    }
    let layout = image_layout(
        asset.width,
        asset.height,
        share,
        spot,
        p.settings.primary_format,
    );
    let item = Item {
        id: new_id(),
        start_us: start,
        duration_us: IMAGE_US,
        attach: None,
        layout,
        content: ItemContent::Image {
            asset_id: asset.id.clone(),
        },
        effects: Vec::new(),
    };
    ops.push(Op::AddItem {
        track: TrackTarget::Auto,
        item,
        attach: start < main_track_duration(&p.timeline.main),
    });
    Ok(ops)
}
