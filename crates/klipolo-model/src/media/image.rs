//! Pictures put over the video (a logo, a sticker): a medium size in the middle of the frame for 5 s from a moment,
//! moving with the clip they start over. Other formats derive their place from the primary's (`resolve_rect`: sizes
//! follow the shorter side, the centre stays centred).

use crate::error::{invalid, Result};
use crate::ids::new_id;
use crate::layout::{Anchor, BoxN, Layout};
use crate::messages::code;
use crate::ops::{Op, TrackTarget};
use crate::project::{Asset, AssetKind, FormatKind, Project};
use crate::query::main_track_duration;
use crate::time::Us;
use crate::timeline::{Item, ItemContent};

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

/// Where a `width` x `height` picture goes in `format`: `IMAGE_SIZE` of the width, centred, its shape kept; one too tall
/// for that is as tall as the frame less the margins. An unknown size is square.
pub fn image_rect(width: u32, height: u32, format: FormatKind) -> BoxN {
    let (cw, ch) = canvas(format);
    let aspect = if width == 0 || height == 0 {
        1.0
    } else {
        f64::from(width) / f64::from(height)
    };
    let mut w = IMAGE_SIZE;
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

/// The ops that add image `asset` at `at_us`: `IMAGE_US` long, centred at `image_rect` in the primary format, on the
/// first overlay track with room, attached to the clip under it (none past the main track's end). The file comes in with it when the project doesn't
/// have it yet. Refused for a file that isn't a picture.
pub fn add_image_ops(p: &Project, asset: &Asset, at_us: Us) -> Result<Vec<Op>> {
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
    let rect = image_rect(asset.width, asset.height, p.settings.primary_format);
    let item = Item {
        id: new_id(),
        start_us: start,
        duration_us: IMAGE_US,
        attach: None,
        layout: Layout::new(Anchor::Center, rect),
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
