//! Pictures chat asks for (Muse Image behind `ImageMaker`): a sticker, or the background of a title card under its
//! title. A picture is never an op until it exists, and everything comes back from commands:
//! - a reply names each picture it asked for (`ChatPicture`);
//! - `draw` draws it (about 35 s) into the project's media folder, not into the project, for its card; Try again draws
//!   it again, and every drawing stays usable: Use names the one its card shows. Every picture drawn stays in that
//!   folder (Muse Image's), paid for, whatever becomes of its card. A title card's background is drawn in the video's
//!   primary format as it is when the drawing starts, and not at all once its title is gone;
//! - `place` (Use) puts it in the project as one undo step from chat: a sticker where its job says once the picture is
//!   there (`StickerPlacement`; nowhere when the moment it was for was cut meanwhile, said plainly), a background on a
//!   new track just under its title's, for as long as the title shows.
//!
//! A picture belongs to the project it was asked for: another project (or New chat) forgets it. Each one that comes
//! back is counted into the session's spend, saved or not (one that can't be read or saved was still paid for).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_chat::sanitize;
use klipolo_model::coded::{CodedError, ErrorParam, Said};
use klipolo_model::ids::{Id, new_id};
use klipolo_model::layout::Layout;
use klipolo_model::ops::{Op, Origin, TrackTarget};
use klipolo_model::project::Asset;
use klipolo_model::query::find_item;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Item, ItemContent, TrackKind};
use klipolo_model::{Project, Transaction};
use serde::Serialize;
use ts_rs::TS;

use super::project::{At, CommitError, Projects};
use super::reply::ChatApplied;
use super::sources::Models;
use super::spend::SpendCounter;
use crate::media::generated::{MadeAsset, MakeError, make_sticker, make_title_card};

/// Said when a picture is no longer there to draw or use: used already, or another project opened.
pub const PICTURE_GONE: &str = "That picture is gone. Ask me for it again.";
/// Said when Use names a drawing the picture doesn't have.
pub const DRAWING_GONE: &str = "That picture isn't there any more. Ask me for it again.";
/// Said when Use comes before the picture was drawn.
pub const NOT_DRAWN: &str = "That picture isn't ready yet.";
/// Said when the part of the video a sticker was for was cut while it was drawn: nothing is placed.
pub const MOMENT_CUT: &str = "The part of the video that sticker was for was cut while it was drawn, so I didn't place \
                              it. Ask me for it again.";
/// Said when a title card's title is gone: its background isn't drawn or placed.
pub const TITLE_GONE: &str =
    "The title for that background is gone, so I left it out. Ask me for it again.";
/// Where a title card's background goes.
pub const BEHIND_THE_TITLE: &str = "Goes behind the title, for as long as it shows.";

/// Where a sticker goes once it is drawn: the compiler's job, read again on the project as it is then.
pub trait StickerPlacement: Send + Sync {
    /// Its start and length on `project`; None when the moment it was for was cut while it was drawn.
    fn placement(&self, project: &Project) -> Option<(Us, Us)>;

    /// The ops that show `asset` from `start_us` for `duration_us`, at the job's size and place in the frame.
    fn ops(&self, project: &Project, asset: &Asset, start_us: Us, duration_us: Us) -> Vec<Op>;
}

/// A picture a message asked for.
#[derive(Clone)]
pub enum PictureAsk {
    Sticker {
        prompt: String,
        /// Where it goes, in one sentence for its card: "Goes in the top right corner, over the moment it was asked
        /// for." No time: it follows its footage.
        place: String,
        placement: Arc<dyn StickerPlacement>,
    },
    /// The background of the title card whose title layer is `title_id`.
    TitleBackground { prompt: String, title_id: Id },
}

impl PictureAsk {
    fn prompt(&self) -> &str {
        match self {
            Self::Sticker { prompt, .. } | Self::TitleBackground { prompt, .. } => prompt,
        }
    }
}

/// A picture a reply asked for: the page draws it (`chat_draw_picture`) and shows it with Use and Try again.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatPicture {
    pub id: String,
    pub kind: ChatPictureKind,
    /// What it shows, as asked, without dashes.
    pub prompt: String,
    /// Where it goes, in one sentence: "Goes in the top right corner, over the moment it was asked for."
    pub place: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChatPictureKind {
    Sticker,
    TitleBackground,
}

/// A picture drawn, for its card: the file (in the project's media folder, not in the project until Use), its size,
/// what made it ("Made with Muse Image"), and a note when it isn't quite what was asked.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatPictureDrawn {
    pub id: String,
    pub path: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub made_with: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub note: Option<String>,
}

/// The pictures asked for and not used yet.
#[derive(Default)]
pub(crate) struct Pictures(Mutex<Vec<Waiting>>);

struct Waiting {
    id: String,
    /// The project it was asked for (its revision aside).
    owner: Option<At>,
    ask: PictureAsk,
    /// Every drawing made for it, oldest first: Use names the one its card shows.
    drawn: Vec<MadeAsset>,
}

fn said(text: &str) -> CommitError {
    CommitError::Refused(Said::Text(text.to_owned()))
}

impl Pictures {
    fn lock(&self) -> MutexGuard<'_, Vec<Waiting>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `ask`, for the project at `owner`, waiting to be drawn.
    pub(crate) fn ask(&self, owner: Option<At>, ask: PictureAsk) -> ChatPicture {
        let card = ChatPicture {
            id: uuid::Uuid::new_v4().to_string(),
            kind: match ask {
                PictureAsk::Sticker { .. } => ChatPictureKind::Sticker,
                PictureAsk::TitleBackground { .. } => ChatPictureKind::TitleBackground,
            },
            prompt: sanitize::reply(ask.prompt().trim()),
            place: match &ask {
                PictureAsk::Sticker { place, .. } => sanitize::reply(place),
                PictureAsk::TitleBackground { .. } => BEHIND_THE_TITLE.to_owned(),
            },
        };
        self.lock().push(Waiting {
            id: card.id.clone(),
            owner,
            ask,
            drawn: Vec::new(),
        });
        card
    }

    /// Every picture is forgotten (another project, New chat).
    pub(crate) fn clear(&self) {
        self.lock().clear();
    }

    fn forget(&self, id: &str) {
        self.lock().retain(|w| w.id != id);
    }

    /// Picture `id` drawn (again) on the open project, if it still belongs to it.
    pub(crate) async fn draw(
        &self,
        id: &str,
        projects: &dyn Projects,
        models: &dyn Models,
        spend: &SpendCounter,
    ) -> Result<ChatPictureDrawn, CommitError> {
        let found = (self.lock().iter())
            .find(|w| w.id == id)
            .map(|w| (w.ask.clone(), w.owner.clone()));
        let Some((ask, owner)) = found else {
            return Err(said(PICTURE_GONE));
        };
        let ours = |at: &At| owner.as_ref().is_some_and(|o| o.same_project(at));
        let Some(now) = projects.now().filter(|now| ours(&now.at)) else {
            self.forget(id);
            return Err(said(PICTURE_GONE));
        };
        if let PictureAsk::TitleBackground { title_id, .. } = &ask
            && find_item(&now.project, title_id).is_none()
        {
            self.forget(id);
            return Err(said(TITLE_GONE));
        }
        let maker = models.images().await.map_err(|why| said(&why))?;
        let made = match &ask {
            PictureAsk::Sticker { prompt, .. } => make_sticker(&*maker, &now.file, prompt).await,
            // The shape the video has now, not when it was asked for.
            PictureAsk::TitleBackground { prompt, .. } => {
                let format = now.project.settings.primary_format;
                make_title_card(&*maker, &now.file, prompt, format).await
            }
        };
        // A picture that came back is paid for, kept or not.
        if made.as_ref().map_or_else(MakeError::billed, |_| true) {
            spend.picture(models.picture_usd());
        }
        let made = made.map_err(|e| {
            tracing::warn!(error = %e, "chat: a picture wasn't made");
            said(&e.user_message())
        })?;
        let drawn = ChatPictureDrawn {
            id: id.to_owned(),
            path: made.asset.path.clone(),
            name: made.asset.name.clone(),
            width: made.asset.width,
            height: made.asset.height,
            made_with: made.made_with.clone(),
            note: made.note.clone(),
        };
        let mut waiting = self.lock();
        match waiting.iter_mut().find(|w| w.id == id) {
            // Try again: the card shows the new picture; the ones before stay, in the folder and for Use.
            Some(w) => {
                w.drawn.push(made);
                Ok(drawn)
            }
            // Forgotten while it was drawn (another project, New chat).
            None => Err(said(PICTURE_GONE)),
        }
    }

    /// Picture `id`'s drawing at `path` (the one its card shows, whatever was drawn since), placed in the open project
    /// as one undo step from chat; what changed and where the project stands now.
    pub(crate) fn place(
        &self,
        id: &str,
        path: &str,
        projects: &dyn Projects,
    ) -> Result<(ChatApplied, At), CommitError> {
        let found = (self.lock().iter())
            .find(|w| w.id == id)
            .map(|w| (w.ask.clone(), w.owner.clone(), w.drawn.clone()));
        let Some((ask, owner, drawn)) = found else {
            return Err(said(PICTURE_GONE));
        };
        if drawn.is_empty() {
            return Err(said(NOT_DRAWN));
        }
        let Some(made) = drawn.into_iter().find(|d| d.asset.path == path) else {
            return Err(said(DRAWING_GONE));
        };
        let ours = |at: &At| owner.as_ref().is_some_and(|o| o.same_project(at));
        let Some(now) = projects.now().filter(|now| ours(&now.at)) else {
            self.forget(id);
            return Err(said(PICTURE_GONE));
        };
        let p = &now.project;
        let (ops, tool) = match &ask {
            PictureAsk::Sticker { placement, .. } => match placement.placement(p) {
                Some((start, length)) => {
                    (placement.ops(p, &made.asset, start, length), "makeSticker")
                }
                None => {
                    self.forget(id);
                    return Err(said(MOMENT_CUT));
                }
            },
            PictureAsk::TitleBackground { title_id, .. } => {
                match background_ops(p, &made.asset, title_id) {
                    Some(ops) => (ops, "titleBackground"),
                    None => {
                        self.forget(id);
                        return Err(said(TITLE_GONE));
                    }
                }
            }
        };
        let undo = CodedError {
            code: format!("chat.{tool}"),
            params: [("prompt".to_owned(), ErrorParam::from(ask.prompt()))].into(),
        };
        let tx = Transaction {
            label: undo.code.clone(),
            params: Some(undo.params.clone()),
            ops,
            origin: Some(Origin::Chat),
        };
        let committed = projects.commit(&now.at, &tx, None)?;
        self.forget(id);
        let applied = ChatApplied {
            change: committed.patch,
            undo,
        };
        Ok((applied, committed.at))
    }
}

/// The ops that put `asset` under the title layer `title_id` on `p`, filling the frame for as long as the title shows
/// (moving with the clip when the title does): a new overlay track just under the title's. None when the title is gone.
pub fn background_ops(p: &Project, asset: &Asset, title_id: &str) -> Option<Vec<Op>> {
    let (track, at) = find_item(p, title_id)?;
    let title = &p.timeline.tracks[track].items[at];
    let under = new_id();
    let mut ops = vec![Op::AddTrack {
        id: Some(under.clone()),
        kind: TrackKind::Overlay,
        index: Some(track),
    }];
    if p.asset(&asset.id).is_none() {
        ops.push(Op::AddAssets {
            assets: vec![asset.clone()],
        });
    }
    ops.push(Op::AddItem {
        track: TrackTarget::Track { id: under },
        attach: title.attach.is_some(),
        item: Item {
            id: new_id(),
            start_us: title.start_us,
            duration_us: title.duration_us,
            attach: None,
            layout: Layout::full_frame(),
            content: ItemContent::Image {
                asset_id: asset.id.clone(),
            },
            effects: Vec::new(),
        },
    });
    Some(ops)
}
