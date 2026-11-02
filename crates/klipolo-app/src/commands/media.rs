//! The media commands: music, sound effects, pictures and words put on the timeline, and a layer's settings changed.
//! Each is thin: the model builds the ops (`klipolo_model::media`) from the open project as it is, and they commit by the
//! path every edit takes (`project::build_and_apply`: one undo step, the engine, the save, music fitted to a new end),
//! returning what changed, as `project_apply` does. Also the bundled pack, a picture's bytes and a sound to play for the
//! library, and where someone speaks for the Music lane's ducking dips.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use klipolo_engine::audio::decode;
use klipolo_engine::audio::mix::RATE;
use klipolo_engine::render::image_load::decode_rgba;
use klipolo_model::coded::{CodedError, Said};
use klipolo_model::history::SnapshotPatch;
use klipolo_model::media::image::{add_image_ops, background_picture_ops};
use klipolo_model::media::music::{MusicOpts, add_music_ops};
use klipolo_model::media::place::{Onto, place_ops};
use klipolo_model::media::sound::add_sound_ops;
use klipolo_model::media::text::{TextPreset, TitleTemplate, add_template_ops, add_text_ops};
use klipolo_model::ops::{ItemPatch, Op};
use klipolo_model::params::ParamValue;
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::TrackKind;
use serde::{Deserialize, Serialize};
use tauri::State;
use ts_rs::TS;

use crate::media::is_picture;
use crate::pack::{Pack, PackItem};
use crate::project::{
    ProjectSessionState, asset_kind, build_and_apply, build_named_and_apply, servable,
};

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

/// Which track a drop names (`place_ops`'s `Onto`): none, the track it was let go on, or a new track of a kind.
#[derive(Clone, Debug, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum DropOnto {
    Auto,
    Track { id: String },
    NewSound,
    NewPicture,
}

impl DropOnto {
    fn onto(&self) -> Onto {
        match self {
            Self::Auto => Onto::Auto,
            Self::Track { id } => Onto::Track(id.clone()),
            Self::NewSound => Onto::New(TrackKind::Audio),
            Self::NewPicture => Onto::New(TrackKind::Overlay),
        }
    }
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
/// The largest picture file the library decodes for its tile.
const MAX_PICTURE_BYTES: u64 = 40 * 1024 * 1024;

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
    state: &ProjectSessionState,
    label: &str,
    source: &MediaSource,
    base_revision: Option<u64>,
    project_id: Option<String>,
    build: impl FnOnce(&Project, &Asset) -> Result<Vec<Op>, Said>,
) -> Result<SnapshotPatch, Said> {
    let pack = state.pack();
    build_and_apply(
        state,
        project_id.as_deref(),
        base_revision,
        label,
        |project| {
            let asset = resolve(project, &pack, source)?;
            build(project, &asset)
        },
    )
    .map_err(|e| e.said())
}

/// `source` as background music from `start_us` (0 when left out) to the video's end: tiled when shorter, fading in and
/// out, ducking under speech, on an audio track of its own (`add_music_ops`).
#[tauri::command(async)]
pub fn media_add_music(
    state: State<'_, ProjectSessionState>,
    source: MediaSource,
    start_us: Option<Us>,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    place(
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
    state: State<'_, ProjectSessionState>,
    source: MediaSource,
    at_us: Us,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    place(
        &state,
        "media.sound.add",
        &source,
        base_revision,
        project_id,
        |p, asset| add_sound_ops(p, asset, at_us).map_err(coded),
    )
}

/// A file let go on the timeline that came into the project without a layer, and why.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UnplacedFile {
    pub name: String,
    pub reason: CodedError,
}

/// What `media_place` did: the change, and the files that got no layer.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlaceAnswer {
    pub patch: SnapshotPatch,
    pub unplaced: Vec<UnplacedFile>,
}

/// Sounds and pictures let go on the timeline at `at_us` (one from the library, or several from Finder): each as the
/// "+" places it, one after another, on the track `onto` names when it takes them (`place_ops`). One undo step, named
/// for what it adds: a sound, a picture, or files. A file that gets no layer still comes in, and is named with why;
/// when none gets one, the first file's reason refuses it.
#[tauri::command(async)]
pub fn media_place(
    state: State<'_, ProjectSessionState>,
    sources: Vec<MediaSource>,
    at_us: Us,
    onto: DropOnto,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<PlaceAnswer, Said> {
    let pack = state.pack();
    let mut unplaced = Vec::new();
    let patch = build_named_and_apply(&state, project_id.as_deref(), base_revision, |project| {
        let assets = sources
            .iter()
            .map(|source| resolve(project, &pack, source))
            .collect::<Result<Vec<_>, _>>()?;
        let label = match assets.as_slice() {
            [one] if one.kind == AssetKind::Audio => "media.sound.add",
            [_] => "media.image.add",
            _ => "media.import",
        };
        let placed = place_ops(project, &assets, at_us.max(0), &onto.onto()).map_err(coded)?;
        unplaced = placed
            .unplaced
            .into_iter()
            .map(|(index, error)| UnplacedFile {
                name: assets[index].name.clone(),
                reason: error.coded(),
            })
            .collect();
        Ok((label.to_owned(), placed.ops))
    })
    .map_err(|e| e.said())?;
    Ok(PlaceAnswer { patch, unplaced })
}

/// Picture `source` behind the video: the project's background, its file brought in when the project doesn't have it
/// (`background_picture_ops`): a picture the project has, the library's, or one chosen from this Mac.
#[tauri::command(async)]
pub fn media_set_background(
    state: State<'_, ProjectSessionState>,
    source: MediaSource,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    place(
        &state,
        "background.picture.undo",
        &source,
        base_revision,
        project_id,
        |p, asset| background_picture_ops(p, asset).map_err(coded),
    )
}

/// Picture `source` at `at_us`: medium, centred, for 5 s, moving with the clip under it (`add_image_ops`).
#[tauri::command(async)]
pub fn media_add_image(
    state: State<'_, ProjectSessionState>,
    source: MediaSource,
    at_us: Us,
    base_revision: Option<u64>,
    project_id: Option<String>,
) -> Result<SnapshotPatch, Said> {
    place(
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
pub fn library_pack(state: State<'_, ProjectSessionState>) -> Vec<PackItem> {
    state.pack().items()
}

/// The longest side of a picture the library shows: its tiles are about 75 to 125 px wide (three to a row), so 256
/// covers them on a 2x screen.
const PICTURE_SIDE: u32 = 256;
const PICTURE_UNREADABLE: &str = "That picture couldn't be read.";

/// A picture for the library, as a PNG at most `PICTURE_SIDE` px on its long side: a pack sticker by its id, or a
/// picture the open project uses (its path), never any other file. Only a decoded picture goes back, never a file's
/// bytes, so a path that leads elsewhere gives nothing away.
#[tauri::command(async)]
pub fn media_picture(
    state: State<'_, ProjectSessionState>,
    pack_id: Option<String>,
    path: Option<String>,
) -> Result<tauri::ipc::Response, String> {
    let file = match (pack_id, path) {
        (Some(id), _) => {
            let pack = state.pack();
            pack.asset(&id)
                .filter(|asset| asset.kind == AssetKind::Image)
                .and(pack.file(&id))
                .ok_or(NOT_IN_PACK)?
                .to_path_buf()
        }
        (None, Some(path)) => picture_file(&state, &path)?,
        (None, None) => return Err(NOT_IN_PROJECT.into()),
    };
    thumbnail(&file).map(tauri::ipc::Response::new)
}

/// The file behind `path`, a picture the open project uses, with every link followed: refused unless what it leads to
/// is a regular file Klipolo imports as a picture (a `logo.png` linked to `~/.ssh/id_rsa` is not).
pub(crate) fn picture_file(state: &ProjectSessionState, path: &str) -> Result<PathBuf, String> {
    if asset_kind(state, path) != Some(AssetKind::Image) {
        return Err(NOT_IN_PROJECT.into());
    }
    let real = std::fs::canonicalize(path).map_err(|_| "That picture isn't there anymore.")?;
    let regular = std::fs::metadata(&real).is_ok_and(|meta| meta.is_file());
    if !regular || !servable(&real.to_string_lossy()) || !is_picture(&real) {
        return Err(NOT_IN_PROJECT.into());
    }
    Ok(real)
}

/// `file` decoded and scaled to at most `PICTURE_SIDE` px on its long side, as a PNG.
pub(crate) fn thumbnail(file: &Path) -> Result<Vec<u8>, String> {
    let size = std::fs::metadata(file)
        .map_err(|_| PICTURE_UNREADABLE.to_string())?
        .len();
    if size > MAX_PICTURE_BYTES {
        return Err("That picture is too large to show here.".into());
    }
    let picture = decode_rgba(file, PICTURE_SIDE).map_err(|error| {
        tracing::info!(%error, "a picture couldn't be shown in the library");
        PICTURE_UNREADABLE.to_string()
    })?;
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            picture.as_raw(),
            picture.width(),
            picture.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|_| PICTURE_UNREADABLE.to_string())?;
    Ok(png)
}

/// How much of a sound the library plays before it is added: a song's first 30 s says what it is, and a long file never
/// goes to the page whole.
const SOUND_PREVIEW_FRAMES: u64 = 30 * RATE as u64;
const SOUND_UNREADABLE: &str = "That sound couldn't be played.";

/// A sound for the library to play before it is added, as a WAV of its first 30 s at most (16-bit, 48 kHz): a pack sound
/// or music bed by its id, or a sound the open project uses (its path), never any other file. Only decoded sound goes
/// back, never a file's bytes, so a path that leads elsewhere gives nothing away.
#[tauri::command(async)]
pub fn media_sound(
    state: State<'_, ProjectSessionState>,
    pack_id: Option<String>,
    path: Option<String>,
) -> Result<tauri::ipc::Response, String> {
    let file = match (pack_id, path) {
        (Some(id), _) => pack_sound(&state.pack(), &id)?,
        (None, Some(path)) => sound_file(&state, &path)?,
        (None, None) => return Err(NOT_IN_PROJECT.into()),
    };
    sound_preview(&file).map(tauri::ipc::Response::new)
}

/// Pack item `id`'s file, when it is a sound or a music bed.
pub(crate) fn pack_sound(pack: &Pack, id: &str) -> Result<PathBuf, String> {
    pack.asset(id)
        .filter(|asset| asset.kind == AssetKind::Audio)
        .and(pack.file(id))
        .map(Path::to_path_buf)
        .ok_or_else(|| NOT_IN_PACK.into())
}

/// The file behind `path`, a sound the open project uses, with every link followed: refused unless what it leads to is a
/// regular file Klipolo imports, or a recording's own sound, and not a picture (a `song.mp3` linked to `~/.ssh/id_rsa`
/// or to `logo.png` is not).
pub(crate) fn sound_file(state: &ProjectSessionState, path: &str) -> Result<PathBuf, String> {
    if asset_kind(state, path) != Some(AssetKind::Audio) {
        return Err(NOT_IN_PROJECT.into());
    }
    let real = std::fs::canonicalize(path).map_err(|_| "That sound isn't there anymore.")?;
    let regular = std::fs::metadata(&real).is_ok_and(|meta| meta.is_file());
    let media = servable(&real.to_string_lossy()) || is_recorded_sound(&real);
    if !regular || !media || is_picture(&real) {
        return Err(NOT_IN_PROJECT.into());
    }
    Ok(real)
}

/// A recording's own sound, by its extension: a mic's runs are CAF files (`mic/run-01.caf`, a separated sound's file),
/// which Klipolo makes but doesn't import.
fn is_recorded_sound(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("caf"))
}

/// `file`'s sound from its start, decoded to 48 kHz (mono kept mono, more channels made stereo) for at most
/// `SOUND_PREVIEW_FRAMES`, as a 16-bit PCM WAV.
pub(crate) fn sound_preview(file: &Path) -> Result<Vec<u8>, String> {
    let unreadable = |error: String| {
        tracing::info!(%error, "a sound couldn't be played in the library");
        SOUND_UNREADABLE.to_string()
    };
    let channels = decode::probe(file).map_err(unreadable)?.channels;
    let mut samples = Vec::new();
    decode::decode(
        file,
        0,
        Some(SOUND_PREVIEW_FRAMES),
        &mut |_, block: &[f32]| {
            samples.extend_from_slice(block);
            Ok(true)
        },
    )
    .map_err(|error| unreadable(format!("{error:?}")))?;
    Ok(wav(&samples, channels))
}

/// Interleaved `samples` of `channels` channels at `RATE`, as a 16-bit PCM WAV.
fn wav(samples: &[f32], channels: u16) -> Vec<u8> {
    let data = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let block = channels * 2;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&data.saturating_add(36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Where someone speaks on the open project's timeline, as the mix ducks music under it: the Music lane draws its dips
/// there.
#[tauri::command(async)]
pub fn media_speech_ranges(state: State<'_, ProjectSessionState>) -> Vec<RangeUs> {
    state.ducking_speech()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::create_in;

    /// A session with a project open that uses each of `pictures` as a picture.
    fn open_with(dir: &Path, pictures: &[&Path]) -> ProjectSessionState {
        let files: Vec<_> = pictures.iter().map(|p| (*p, AssetKind::Image)).collect();
        open_using(dir, &files)
    }

    /// A session with a project open that uses each file as its kind.
    fn open_using(dir: &Path, files: &[(&Path, AssetKind)]) -> ProjectSessionState {
        let state = ProjectSessionState::default();
        let mut project = Project::new("Pictures");
        project.assets = files
            .iter()
            .enumerate()
            .map(|(n, (path, kind))| Asset {
                id: format!("p{n}"),
                kind: *kind,
                path: path.to_string_lossy().into_owned(),
                name: format!("p{n}"),
                duration_us: 0,
                width: 10,
                height: 10,
                fps: 0.0,
                rotation: None,
                has_audio: *kind == AssetKind::Audio,
                audio_lead_us: None,
                made_with: None,
            })
            .collect();
        create_in(&state, dir, project).unwrap();
        state
    }

    /// A picture the project uses through a link is read from what the link leads to, and only when that is a
    /// picture: a `logo.png` linked to a key file gives nothing back. What goes back is the picture at 256 px at most.
    #[test]
    fn a_linked_picture_is_followed_and_must_lead_to_a_picture() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.png");
        image::RgbaImage::from_pixel(2000, 1000, image::Rgba([255, 0, 0, 255]))
            .save(&real)
            .unwrap();
        let secret = dir.path().join("id_rsa");
        std::fs::write(&secret, b"-----BEGIN KEY-----").unwrap();
        let (linked, sneaky) = (dir.path().join("linked.png"), dir.path().join("logo.png"));
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        std::os::unix::fs::symlink(&secret, &sneaky).unwrap();
        let state = open_with(dir.path(), &[&linked, &sneaky]);
        let file = picture_file(&state, &linked.to_string_lossy()).unwrap();
        assert_eq!(file, std::fs::canonicalize(&real).unwrap());
        assert_eq!(
            picture_file(&state, &sneaky.to_string_lossy()).unwrap_err(),
            NOT_IN_PROJECT
        );
        // The file behind the link isn't one the project uses by that name.
        assert_eq!(
            picture_file(&state, &real.to_string_lossy()).unwrap_err(),
            NOT_IN_PROJECT
        );
        let shown = image::load_from_memory(&thumbnail(&file).unwrap()).unwrap();
        assert_eq!((shown.width(), shown.height()), (256, 128));
    }

    /// `seconds` of a quiet 440 Hz tone, mono, as a WAV at 48 kHz.
    fn tone(seconds: u64) -> Vec<u8> {
        let samples: Vec<f32> = (0..seconds * u64::from(RATE))
            .map(|n| 0.25 * (n as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin())
            .collect();
        wav(&samples, 1)
    }

    /// The library plays a pack sound or music bed by its id, and nothing else from the pack: an id it doesn't have, or a
    /// sticker's, is refused.
    #[test]
    fn a_pack_sound_is_refused_unless_the_pack_has_it_as_a_sound() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pop.wav"), tone(1)).unwrap();
        std::fs::write(dir.path().join("star.png"), b"PNG").unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{"items": [
                {"id": "pop", "name": "Pop", "category": "sound", "file": "pop.wav", "duration_ms": 1000},
                {"id": "star", "name": "Star", "category": "sticker", "file": "star.png", "width": 8, "height": 8}
            ]}"#,
        )
        .unwrap();
        let pack = Pack::read(dir.path());
        assert_eq!(
            pack_sound(&pack, "pop").unwrap(),
            dir.path().join("pop.wav")
        );
        assert_eq!(pack_sound(&pack, "nope").unwrap_err(), NOT_IN_PACK);
        assert_eq!(pack_sound(&pack, "star").unwrap_err(), NOT_IN_PACK);
        assert_eq!(pack_sound(&pack, "../pop").unwrap_err(), NOT_IN_PACK);
    }

    /// A sound by its path plays only when the open project uses that path as a sound, and what it leads to is a media
    /// file: any other file on disk, a `tune.mp3` linked to a key file, or a picture the project uses is refused.
    #[test]
    fn a_sound_by_path_is_refused_unless_the_project_uses_it_as_a_sound() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.wav");
        std::fs::write(&song, tone(1)).unwrap();
        let secret = dir.path().join("id_rsa");
        std::fs::write(&secret, b"-----BEGIN KEY-----").unwrap();
        let sneaky = dir.path().join("tune.mp3");
        std::os::unix::fs::symlink(&secret, &sneaky).unwrap();
        let picture = dir.path().join("logo.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]))
            .save(&picture)
            .unwrap();
        // A sound by name that leads to a picture.
        let pictured = dir.path().join("cover.mp3");
        std::os::unix::fs::symlink(&picture, &pictured).unwrap();
        let state = open_using(
            dir.path(),
            &[
                (&song, AssetKind::Audio),
                (&sneaky, AssetKind::Audio),
                (&pictured, AssetKind::Audio),
                (&picture, AssetKind::Image),
            ],
        );
        let path = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(
            sound_file(&state, &path(&song)).unwrap(),
            std::fs::canonicalize(&song).unwrap()
        );
        for refused in [
            &secret,
            &sneaky,
            &pictured,
            &picture,
            Path::new("/etc/passwd"),
        ] {
            assert_eq!(
                sound_file(&state, &path(refused)).unwrap_err(),
                NOT_IN_PROJECT,
                "{}",
                refused.display()
            );
        }
        // With no project open, no path plays.
        assert_eq!(
            sound_file(&ProjectSessionState::default(), &path(&song)).unwrap_err(),
            NOT_IN_PROJECT
        );
    }

    /// `seconds` of the same tone as a CAF file of 16-bit little-endian PCM at 48 kHz, as a recording's mic writes.
    fn caf_tone(seconds: u64) -> Vec<u8> {
        let pcm = &tone(seconds)[44..];
        let mut caf = b"caff\x00\x01\x00\x00desc".to_vec();
        caf.extend_from_slice(&32i64.to_be_bytes());
        caf.extend_from_slice(&48_000f64.to_be_bytes());
        caf.extend_from_slice(b"lpcm");
        // Flags (little-endian integers), bytes per packet, frames per packet, channels, bits per channel.
        for field in [2u32, 2, 1, 1, 16] {
            caf.extend_from_slice(&field.to_be_bytes());
        }
        caf.extend_from_slice(b"data");
        caf.extend_from_slice(&(pcm.len() as i64 + 4).to_be_bytes());
        caf.extend_from_slice(&0u32.to_be_bytes());
        caf.extend_from_slice(pcm);
        caf
    }

    /// A recording's separated sound, its mic's `mic/run-01.caf`, plays: Klipolo makes CAF files but doesn't import
    /// them, and the project uses this one as a sound.
    #[test]
    fn a_recordings_mic_run_plays() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("mic")).unwrap();
        let run = dir.path().join("mic/run-01.caf");
        std::fs::write(&run, caf_tone(1)).unwrap();
        let state = open_using(dir.path(), &[(&run, AssetKind::Audio)]);
        let file = sound_file(&state, &run.to_string_lossy()).unwrap();
        assert_eq!(file, std::fs::canonicalize(&run).unwrap());
        let wav = sound_preview(&file).unwrap();
        let data = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(
            (u16::from_le_bytes([wav[22], wav[23]]), data),
            (1, 48_000 * 2)
        );
    }

    /// What plays is the sound decoded from its start, 30 s at most, as a 16-bit WAV at 48 kHz: a long song never goes to
    /// the page whole.
    #[test]
    fn a_sound_plays_as_its_first_30_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let (short, long) = (dir.path().join("short.wav"), dir.path().join("long.wav"));
        std::fs::write(&short, tone(2)).unwrap();
        std::fs::write(&long, tone(31)).unwrap();
        /// A WAV's tags, channels, rate, data length and whole length.
        fn header(wav: &[u8]) -> (&[u8], &[u8], u16, u32, usize, usize) {
            let channels = u16::from_le_bytes([wav[22], wav[23]]);
            let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
            let data = u32::from_le_bytes(wav[40..44].try_into().unwrap());
            (
                &wav[..4],
                &wav[8..16],
                channels,
                rate,
                data as usize,
                wav.len(),
            )
        }
        let played = sound_preview(&short).unwrap();
        assert_eq!(
            header(&played),
            (
                &b"RIFF"[..],
                &b"WAVEfmt "[..],
                1,
                RATE,
                2 * 48_000 * 2,
                44 + 2 * 48_000 * 2
            )
        );
        let (.., data, _) = header(&sound_preview(&long).unwrap());
        assert_eq!(data, 30 * 48_000 * 2);
        let bad = dir.path().join("bad.wav");
        std::fs::write(&bad, b"not a sound").unwrap();
        assert_eq!(sound_preview(&bad).unwrap_err(), SOUND_UNREADABLE);
    }
}
