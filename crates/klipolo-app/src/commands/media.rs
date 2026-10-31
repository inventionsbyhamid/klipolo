//! The media commands: music, sound effects, pictures and words put on the timeline, and a layer's settings changed.
//! Each is thin: the model builds the ops (`klipolo_model::media`) from the open project as it is, and they commit by the
//! path every edit takes (`project::build_and_apply`: one undo step, the engine, the save, music fitted to a new end),
//! returning what changed, as `project_apply` does. Also the bundled pack, a picture's bytes for the library, and where
//! someone speaks for the Music lane's ducking dips.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use klipolo_model::coded::Said;
use klipolo_model::history::SnapshotPatch;
use klipolo_model::media::music::{MusicOpts, add_music_ops};
use klipolo_model::media::text::{TextPreset, TitleTemplate, add_template_ops, add_text_ops};
use klipolo_model::media::{image::add_image_ops, sound::add_sound_ops};
use klipolo_model::ops::{ItemPatch, Op};
use klipolo_model::params::ParamValue;
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::time::{RangeUs, Us};
use serde::Deserialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::pack::{Pack, PackItem, pack_dir};
use crate::project::{ProjectSessionState, asset_kind, build_and_apply, servable};

/// The file a media command places.
#[derive(Clone, Debug, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum MediaSource {
    /// A file the project has.
    Asset { id: String },
    /// A file just imported (`media_import`) and not in the project yet: it comes in with its layer, in one step.
    File { asset: Asset },
    /// An item of Klipolo's pack, by its id (`library_pack`).
    Pack { id: String },
}

/// A text layer of a preset, or a title template (a motion graphic that fills the frame).
#[derive(Clone, Debug, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum TextKind {
    Text { preset: TextPresetWord },
    Template { template: TemplateWord },
}

#[derive(Clone, Copy, Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TextPresetWord {
    Title,
    LowerThird,
    Plain,
    Caption,
}

#[derive(Clone, Copy, Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TemplateWord {
    LowerThird,
    TitleCard,
}

const NOT_IN_PROJECT: &str = "That file isn't part of this project.";
const NOT_IN_PACK: &str = "That item isn't in Klipolo's pack.";
/// The largest picture the library shows from its own file.
const MAX_PICTURE_BYTES: u64 = 40 * 1024 * 1024;

/// The pack, read once.
fn pack(app: &AppHandle) -> &'static Pack {
    static PACK: OnceLock<Pack> = OnceLock::new();
    PACK.get_or_init(|| {
        pack_dir(app.path().resource_dir().ok())
            .map(|dir| Pack::read(&dir))
            .unwrap_or_default()
    })
}

/// `source` as an asset of `project`, or why not.
fn resolve(project: &Project, pack: &Pack, source: &MediaSource) -> Result<Asset, Said> {
    match source {
        MediaSource::Asset { id } => project
            .asset(id)
            .cloned()
            .ok_or_else(|| Said::Text(NOT_IN_PROJECT.into())),
        MediaSource::File { asset } => Ok(project
            .asset(&asset.id)
            .cloned()
            .unwrap_or_else(|| asset.clone())),
        MediaSource::Pack { id } => {
            let asset = pack
                .asset(id)
                .ok_or_else(|| Said::Text(NOT_IN_PACK.into()))?;
            // Added before: the project's own copy of it.
            Ok(project.asset(&asset.id).cloned().unwrap_or(asset))
        }
    }
}

fn coded(e: klipolo_model::ModelError) -> Said {
    Said::Coded(e.coded())
}

/// Commits what `build` makes of the open project and `source`'s asset, labelled `label`.
fn place(
    app: &AppHandle,
    state: &ProjectSessionState,
    label: &str,
    source: &MediaSource,
    base_revision: Option<u64>,
    project_id: Option<String>,
    build: impl FnOnce(&Project, &Asset) -> Result<Vec<Op>, Said>,
) -> Result<SnapshotPatch, Said> {
    let pack = pack(app);
    build_and_apply(
        state,
        project_id.as_deref(),
        base_revision,
        label,
        |project| {
            let asset = resolve(project, pack, source)?;
            build(project, &asset)
        },
    )
    .map_err(|e| e.said())
}

/// `source` as background music from `start_us` (0 when left out) to the video's end: tiled when shorter, fading in and
/// out, ducking under speech, on an audio track of its own (`add_music_ops`).
#[tauri::command(async)]
pub fn media_add_music(
    app: AppHandle,
    state: State<'_, ProjectSessionState>,
    source: MediaSource,
    start_us: Option<Us>,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    place(
        &app,
        &state,
        "media.music.add",
        &source,
        base_revision,
        project_id,
        |p, asset| {
            let opts = MusicOpts {
                start_us: start_us.unwrap_or(0).max(0),
                ..MusicOpts::default()
            };
            add_music_ops(p, asset, opts).map_err(coded)
        },
    )
}

/// `source` as a sound effect at `at_us`: the whole file at full volume, moving with the clip under it, cut at the
/// video's end (`add_sound_ops`).
#[tauri::command(async)]
pub fn media_add_sound(
    app: AppHandle,
    state: State<'_, ProjectSessionState>,
    source: MediaSource,
    at_us: Us,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    place(
        &app,
        &state,
        "media.sound.add",
        &source,
        base_revision,
        project_id,
        |p, asset| add_sound_ops(p, asset, at_us).map_err(coded),
    )
}

/// Picture `source` at `at_us`: medium, centred, for 5 s, moving with the clip under it (`add_image_ops`).
#[tauri::command(async)]
pub fn media_add_image(
    app: AppHandle,
    state: State<'_, ProjectSessionState>,
    source: MediaSource,
    at_us: Us,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    place(
        &app,
        &state,
        "media.image.add",
        &source,
        base_revision,
        project_id,
        |p, asset| add_image_ops(p, asset, at_us).map_err(coded),
    )
}

/// Words at `at_us`: a text layer of a preset reading `text` (the preset's own words when left out), or a title template
/// whose title reads `text` (its own default when left out).
#[tauri::command(async)]
pub fn media_add_text(
    state: State<'_, ProjectSessionState>,
    kind: TextKind,
    at_us: Us,
    text: Option<String>,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    let label = match kind {
        TextKind::Text { .. } => "media.text.add",
        TextKind::Template { .. } => "media.title.add",
    };
    build_and_apply(
        &state,
        project_id.as_deref(),
        base_revision,
        label,
        |p| match kind {
            TextKind::Text { preset } => {
                let preset = match preset {
                    TextPresetWord::Title => TextPreset::Title,
                    TextPresetWord::LowerThird => TextPreset::LowerThird,
                    TextPresetWord::Plain => TextPreset::Plain,
                    TextPresetWord::Caption => TextPreset::Caption,
                };
                let words = text.as_deref().unwrap_or("Your text");
                add_text_ops(p, preset, words, at_us).map_err(coded)
            }
            TextKind::Template { template } => {
                let template = match template {
                    TemplateWord::LowerThird => TitleTemplate::LowerThird,
                    TemplateWord::TitleCard => TitleTemplate::TitleCard,
                };
                let mut params = BTreeMap::new();
                if let Some(title) = text.filter(|t| !t.trim().is_empty()) {
                    params.insert("title".to_owned(), ParamValue::Text(title));
                }
                Ok(add_template_ops(p, template, params, at_us))
            }
        },
    )
    .map_err(|e| e.said())
}

/// Layer `id`'s fields in `patch` (its volume, ducking, layout, words), as one step labelled `label` (an undo label the
/// page words, `music.volume.undo`).
#[tauri::command(async)]
pub fn media_update_item(
    state: State<'_, ProjectSessionState>,
    id: String,
    patch: ItemPatch,
    label: String,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    build_and_apply(&state, project_id.as_deref(), base_revision, &label, |_| {
        Ok(vec![Op::UpdateItem { id, patch }])
    })
    .map_err(|e| e.said())
}

/// Klipolo's pack, as the library lists it.
#[tauri::command(async)]
pub fn library_pack(app: AppHandle) -> Vec<PackItem> {
    pack(&app).items()
}

/// A picture's bytes for the library: a pack sticker by its id, or a picture the open project uses (its path), never
/// any other file.
#[tauri::command(async)]
pub fn media_picture(
    app: AppHandle,
    state: State<'_, ProjectSessionState>,
    pack_id: Option<String>,
    path: Option<String>,
) -> Result<tauri::ipc::Response, String> {
    let file = match (pack_id, path) {
        (Some(id), _) => {
            let pack = pack(&app);
            pack.asset(&id)
                .filter(|asset| asset.kind == AssetKind::Image)
                .and(pack.file(&id))
                .ok_or(NOT_IN_PACK)?
                .to_path_buf()
        }
        (None, Some(path)) => {
            if asset_kind(&state, &path) != Some(AssetKind::Image) || !servable(&path) {
                return Err(NOT_IN_PROJECT.into());
            }
            path.into()
        }
        (None, None) => return Err(NOT_IN_PROJECT.into()),
    };
    let size = std::fs::metadata(&file).map_err(|e| e.to_string())?.len();
    if size > MAX_PICTURE_BYTES {
        return Err("That picture is too large to show here.".into());
    }
    std::fs::read(&file)
        .map(tauri::ipc::Response::new)
        .map_err(|e| e.to_string())
}

/// Where someone speaks on the open project's timeline, as the mix ducks music under it: the Music lane draws its dips
/// there.
#[tauri::command(async)]
pub fn media_speech_ranges(state: State<'_, ProjectSessionState>) -> Vec<RangeUs> {
    state.ducking_speech()
}
