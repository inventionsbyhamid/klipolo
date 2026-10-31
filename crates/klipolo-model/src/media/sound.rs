//! Sound effects put at a moment (a whoosh at a topic change): the whole file at full volume with no fades, moving with
//! the clip it starts over so it keeps its moment when earlier parts are cut, and cut at the video's end. A sound effect
//! has no `duck` (`music::is_music`), so it plays on the Sounds lane and never fits itself to the video as music does.

use crate::error::{invalid, Result};
use crate::ids::new_id;
use crate::layout::{Anchor, BoxN, Layout};
use crate::messages::code;
use crate::ops::{Op, TrackTarget};
use crate::project::{Asset, Project};
use crate::query::{main_track_duration, video_duration};
use crate::time::{Us, MIN_DURATION_US};
use crate::timeline::{Item, ItemContent};

/// Full volume: a sound effect is meant to be heard over the voice.
pub const SOUND_VOLUME: f64 = 1.0;

/// The ops that add `asset` as a sound effect at `at_us`: on the first audio track with room (`TrackTarget::Auto`),
/// attached to the clip under it (none past the main track's end), the whole file, cut where the video ends when it
/// would play past it (with no video yet, it plays whole). The file comes in with it when the
/// project doesn't have it yet (the library's pack). Refused when the file has no sound, or no room is left before the
/// end (a moment at or past the video's end plays over nothing).
pub fn add_sound_ops(p: &Project, asset: &Asset, at_us: Us) -> Result<Vec<Op>> {
    if !asset.has_audio || asset.duration_us < MIN_DURATION_US {
        return Err(invalid(code!("model.sound.noSound")));
    }
    let start = at_us.max(0);
    let video = video_duration(&p.timeline);
    let length = if video > 0 {
        asset.duration_us.min(video.saturating_sub(start))
    } else {
        asset.duration_us
    };
    if length < MIN_DURATION_US {
        return Err(invalid(code!("model.sound.noRoom")));
    }
    let mut ops = Vec::new();
    if p.asset(&asset.id).is_none() {
        ops.push(Op::AddAssets {
            assets: vec![asset.clone()],
        });
    }
    let item = Item {
        id: new_id(),
        start_us: start,
        duration_us: length,
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
    };
    ops.push(Op::AddItem {
        track: TrackTarget::Auto,
        item,
        attach: start < main_track_duration(&p.timeline.main),
    });
    Ok(ops)
}
