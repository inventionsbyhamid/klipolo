//! Stickers and title card backgrounds drawn on request by an `ImageMaker` (Muse Image in the app) and saved in the
//! project's media folder (`media_folder`). A sticker is cut out of its white background (`sticker`); a title card
//! background stays as drawn, with no lettering of its own: Klipolo's title-card template draws the exact title on top,
//! so its words, spelling, type and copy rules are Klipolo's. What comes back is a new image asset for the caller to
//! place, with the label for the library ("Made with Muse Image"); nothing is added to the project here. Any failure
//! says why in one plain sentence and leaves no file, so Try again simply asks again.

use std::fmt;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::ImageFormat;
use image::io::{Limits, Reader as ImageReader};
use klipolo_analysis::providers::images::{ImageError, ImageMaker, ImageSize, MadeImage};
use klipolo_model::project::{Asset, AssetKind, FormatKind};
use klipolo_model::time::Us;
use klipolo_platform::durable::{self, DiskReason};

use super::media_folder::{made_folder, save_new_png, slug};
use super::sticker::{CutOutError, cut_out_white};

/// A still image's length as an asset, as import gives one.
const IMAGE_DURATION_US: Us = 5_000_000;
/// The most words of a prompt kept in an asset's name.
const NAME_WORDS: usize = 6;
/// The longest name taken from a prompt, before " sticker" or " title card background".
const NAME_CHARS: usize = 48;
/// The biggest picture accepted: Muse Image's are 1600 px on their long side.
const MAX_SIDE: u32 = 4096;

/// The words Muse Image gets for a sticker of `prompt`: one subject, a bold dark outline the cut-out stops at, and
/// plain white around it for the cut-out to take away.
pub fn sticker_prompt(prompt: &str) -> String {
    format!(
        "A single {prompt} as a sticker illustration with a bold dark outline, centred with empty margin on all sides, \
         on a plain white background, nothing else"
    )
}

/// The words Muse Image gets for a title card about `prompt`: a background only. Muse Image's own lettering can't be
/// held to the title's words, spelling or type, so it is asked for none, and for a calm middle third where Klipolo's
/// template puts the title.
pub fn title_card_prompt(prompt: &str) -> String {
    format!(
        "A background for a title card about {prompt}, with no text, letters, numbers or signs anywhere, and calm, \
         uncluttered space in the middle third for a title"
    )
}

/// A picture made for the project: its new asset (a PNG in the project's media folder, not yet in the project), the
/// model that made it, for the library's label, and a note for the person when the picture isn't quite what was asked
/// (the white was kept).
#[derive(Clone, Debug, PartialEq)]
pub struct MadeAsset {
    pub asset: Asset,
    /// "Muse Image": the library shows "Made with Muse Image".
    pub made_with: String,
    /// A short note to show beside the picture, when there is one.
    pub note: Option<String>,
    /// The prompt the model actually drew from, when it said.
    pub revised_prompt: Option<String>,
}

/// Why no picture was made. Nothing was saved, and nothing is added to the project.
#[derive(Clone, Debug, PartialEq)]
pub enum MakeError {
    /// The prompt was empty; nothing was asked for.
    NoPrompt,
    /// The model failed (busy, out of credit, refused, timed out...), with its one sentence.
    Maker(ImageError),
    /// The picture came back but couldn't be read.
    Unreadable { made_with: String, detail: String },
    /// The picture couldn't be saved in the project's media folder.
    Save { reason: DiskReason, detail: String },
}

impl MakeError {
    /// One plain sentence for the person.
    pub fn user_message(&self) -> String {
        match self {
            Self::NoPrompt => "Say what the picture should show.".into(),
            Self::Maker(e) => e.user_message().to_owned(),
            Self::Unreadable { made_with, .. } => {
                format!("{made_with} sent a picture Klipolo couldn't read, so try again.")
            }
            Self::Save { reason, .. } => match reason {
                DiskReason::Full => "Your disk is full, so the picture couldn't be saved.".into(),
                DiskReason::ReadOnly | DiskReason::Denied | DiskReason::Locked => {
                    "Klipolo can't write to the project's folder, so the picture couldn't be saved."
                        .into()
                }
                DiskReason::Other => "Klipolo couldn't save the picture, so try again.".into(),
            },
        }
    }
}

/// For logs: the details, never the person's sentence alone.
impl fmt::Display for MakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPrompt => f.write_str("no prompt"),
            Self::Maker(e) => write!(f, "{e}"),
            Self::Unreadable { detail, .. } => write!(f, "unreadable picture: {detail}"),
            Self::Save { detail, .. } => write!(f, "couldn't save the picture: {detail}"),
        }
    }
}

impl std::error::Error for MakeError {}

/// A sticker of `prompt` for the project saved in `project_file`: drawn by `maker` on white, cut out, and saved as a
/// PNG in the project's media folder. When the cut-out would erase most of the picture (or find nothing to cut), the
/// picture is kept whole with a note.
pub async fn make_sticker(
    maker: &dyn ImageMaker,
    project_file: &Path,
    prompt: &str,
) -> Result<MadeAsset, MakeError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(MakeError::NoPrompt);
    }
    let drawn = draw(maker, &sticker_prompt(prompt), ImageSize::Square).await?;
    let made_with = maker.made_with().to_owned();
    let unreadable = |detail: String| MakeError::Unreadable {
        made_with: made_with.clone(),
        detail,
    };
    let opaque = drawn.png;
    let (png, note) = tokio::task::spawn_blocking(move || match cut_out_white(&opaque) {
        Ok(keyed) => Ok((keyed.png, None)),
        Err(CutOutError::Unreadable(detail)) => Err(detail),
        Err(kept) => Ok((opaque, kept.note().map(str::to_owned))),
    })
    .await
    .map_err(|e| unreadable(e.to_string()))?
    .map_err(unreadable)?;
    let folder = made_folder(project_file, &made_with);
    let name = format!("{} sticker", name(prompt));
    let asset = save(folder, slug(prompt, "sticker"), png, name, &made_with).await?;
    Ok(MadeAsset {
        asset,
        made_with,
        note,
        revised_prompt: drawn.revised_prompt,
    })
}

/// A title card background about `prompt` in the shape of `format`, for the project saved in `project_file`: drawn by
/// `maker` with no lettering (`title_card_prompt`) and saved as drawn (opaque) in the project's media folder. The title
/// itself is drawn on top by Klipolo's title-card template when the background is placed.
pub async fn make_title_card(
    maker: &dyn ImageMaker,
    project_file: &Path,
    prompt: &str,
    format: FormatKind,
) -> Result<MadeAsset, MakeError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(MakeError::NoPrompt);
    }
    let size = match format {
        FormatKind::Landscape => ImageSize::Landscape,
        FormatKind::Portrait => ImageSize::Portrait,
        FormatKind::Square => ImageSize::Square,
    };
    let drawn = draw(maker, &title_card_prompt(prompt), size).await?;
    let made_with = maker.made_with().to_owned();
    let folder = made_folder(project_file, &made_with);
    let file_slug = format!("title-{}", slug(prompt, "card"));
    let name = format!("{} title card background", name(prompt));
    let asset = save(folder, file_slug, drawn.png, name, &made_with).await?;
    Ok(MadeAsset {
        asset,
        made_with,
        note: None,
        revised_prompt: drawn.revised_prompt,
    })
}

/// The first picture `maker` draws of `prompt`.
async fn draw(
    maker: &dyn ImageMaker,
    prompt: &str,
    size: ImageSize,
) -> Result<MadeImage, MakeError> {
    let images = maker
        .make(prompt, size, 1)
        .await
        .map_err(MakeError::Maker)?;
    images
        .into_iter()
        .next()
        .ok_or_else(|| MakeError::Unreadable {
            made_with: maker.made_with().to_owned(),
            detail: "the answer held no picture".into(),
        })
}

/// Saves `png` under a new name in `folder` and describes it as an image asset at its absolute path, as import does.
/// The picture is decoded whole first, so one that can't be shown is never saved.
async fn save(
    folder: PathBuf,
    file_slug: String,
    png: Vec<u8>,
    name: String,
    made_with: &str,
) -> Result<Asset, MakeError> {
    let made_with = made_with.to_owned();
    let (file, width, height) = tokio::task::spawn_blocking(move || {
        let (width, height) =
            decoded_size(&png).map_err(|detail| MakeError::Unreadable { made_with, detail })?;
        let file = save_new_png(&folder, &file_slug, &png).map_err(|e| MakeError::Save {
            reason: durable::classify(&e),
            detail: e.to_string(),
        })?;
        Ok::<_, MakeError>((file, width, height))
    })
    .await
    .map_err(|e| MakeError::Save {
        reason: DiskReason::Other,
        detail: e.to_string(),
    })??;
    Ok(Asset {
        id: uuid::Uuid::new_v4().to_string(),
        kind: AssetKind::Image,
        path: file.to_string_lossy().into_owned(),
        name,
        duration_us: IMAGE_DURATION_US,
        width,
        height,
        fps: 0.0,
        rotation: None,
        has_audio: false,
        audio_lead_us: None,
    })
}

/// A PNG's width and height, once the whole picture decodes.
fn decoded_size(png: &[u8]) -> Result<(u32, u32), String> {
    let mut reader = ImageReader::with_format(Cursor::new(png), ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    reader.limits(limits);
    let picture = reader.decode().map_err(|e| e.to_string())?;
    match (picture.width(), picture.height()) {
        (0, _) | (_, 0) => Err("the picture is empty".into()),
        size => Ok(size),
    }
}

/// An asset name from a prompt: its first words, starting with a capital.
fn name(prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().take(NAME_WORDS).collect();
    let mut name: String = words.join(" ").chars().take(NAME_CHARS).collect();
    if let Some(first) = name.chars().next() {
        let upper: String = first.to_uppercase().collect();
        name.replace_range(..first.len_utf8(), &upper);
    }
    name.trim_end().to_owned()
}
