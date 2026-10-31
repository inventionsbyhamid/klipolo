//! The compiler's picture jobs as pictures to draw (`PictureAsk`): a sticker placed where `sticker_placement` says once
//! it is drawn, with `picture_ops` at the job's size and place in the frame; a title card's background under its title
//! layer (`pictures::background_ops`). This is the one place that reads the jobs' fields.

use std::sync::Arc;

use klipolo_chat::{MakeSticker, picture_ops, sticker_placement};
use klipolo_model::Project;
use klipolo_model::ids::Id;
use klipolo_model::ops::Op;
use klipolo_model::project::Asset;
use klipolo_model::time::Us;

use super::pictures::{PictureAsk, StickerPlacement};

/// A `makeSticker` job, read again on the project as it is when its picture comes.
struct Sticker(MakeSticker);

impl StickerPlacement for Sticker {
    fn placement(&self, project: &Project) -> Option<(Us, Us)> {
        sticker_placement(project, &self.0)
    }

    fn ops(&self, project: &Project, asset: &Asset, start_us: Us, duration_us: Us) -> Vec<Op> {
        let job = &self.0;
        picture_ops(
            project,
            asset,
            start_us,
            duration_us,
            job.size,
            job.position,
        )
    }
}

/// A sticker to draw.
pub(super) fn sticker(job: &MakeSticker) -> PictureAsk {
    PictureAsk::Sticker {
        prompt: job.prompt.clone(),
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
