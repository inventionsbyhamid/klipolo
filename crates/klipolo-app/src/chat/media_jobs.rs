//! The compiler's picture jobs as pictures to draw (`PictureAsk`): a sticker placed by `sticker_ops` once it is drawn
//! (where `sticker_placement` says, at the job's size and place in the frame, as the library places a picture); a title
//! card's background under its title layer (`pictures::background_ops`). This is the one place that reads the jobs'
//! fields.

use std::sync::Arc;

use klipolo_chat::tools::Position;
use klipolo_chat::{MakeSticker, sticker_ops};
use klipolo_model::Project;
use klipolo_model::ids::Id;
use klipolo_model::ops::Op;
use klipolo_model::project::Asset;

use super::pictures::{PictureAsk, StickerPlacement};

/// A `makeSticker` job, read again on the project as it is when its picture comes.
struct Sticker(MakeSticker);

impl StickerPlacement for Sticker {
    fn ops(&self, project: &Project, asset: &Asset) -> Option<Vec<Op>> {
        sticker_ops(project, &self.0, asset)
    }
}

/// A sticker to draw.
pub(super) fn sticker(job: &MakeSticker) -> PictureAsk {
    let corner = match job.position {
        Position::TopLeft => "in the top left corner",
        Position::TopRight => "in the top right corner",
        Position::BottomLeft => "in the bottom left corner",
        Position::BottomRight => "in the bottom right corner",
        Position::Center => "in the middle",
    };
    // No time: the sticker follows its footage, so a time said now could be wrong by the time it is placed.
    PictureAsk::Sticker {
        prompt: job.prompt.clone(),
        place: format!("Goes {corner}, over the moment it was asked for."),
        placement: Arc::new(Sticker(job.clone())),
    }
}

/// A title card's background to draw: the primary format is read again when the drawing starts.
pub(super) fn title_background(prompt: &str, title_id: &Id) -> PictureAsk {
    PictureAsk::TitleBackground {
        prompt: prompt.to_owned(),
        title_id: title_id.clone(),
    }
}
