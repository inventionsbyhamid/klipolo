//! A file's thumbnail sprites on disk: `<root>/<stem>/` holds `strip.json` (the tile size), one JPEG per finished
//! chunk (`c-00012.k.jpg` from keyframes, `c-00012.f.jpg` from an editing copy's full decode) and the
//! keyframe index (`keyframes::KEYS_FILE`). Every file is written atomically, so a crash leaves whole chunks only.

use std::io;
use std::path::{Path, PathBuf};

use klipolo_model::time::Us;
use klipolo_platform::durable::{Durability, Steps, write_atomic_with};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::key::{CacheKind, stem};
use super::sprite::{COLS, PER_CHUNK, SLOT_US, Sprite};
use super::still::encode_jpeg;

const STRIP_FILE: &str = "strip.json";
const VERSION: u32 = 1;

/// Where a chunk's tiles came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Keyframes only (the audio pass or a thumbnails-only scan): a tile shows the nearest earlier keyframe.
    Keys,
    /// One tile per second from a full decode (an editing copy); preferred over `Keys`.
    Full,
}

impl Fill {
    fn tag(self) -> &'static str {
        match self {
            Self::Keys => "k",
            Self::Full => "f",
        }
    }
}

/// What the timeline draws a file's filmstrip from (`media_thumbs`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThumbStrip {
    pub tile_w: u32,
    pub tile_h: u32,
    /// Tiles per sprite row.
    pub cols: u32,
    /// Slots per chunk (one per `slot_us`).
    pub per_chunk: u32,
    pub slot_us: Us,
    /// The finished chunks, by index; a missing index isn't scanned yet.
    pub chunks: Vec<ThumbChunk>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThumbChunk {
    pub index: u32,
    /// The sprite JPEG (served through the asset protocol).
    pub path: String,
    /// Every second has its own tile (an editing copy's full decode), not the nearest keyframe's.
    pub full: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StripFile {
    tile_w: u32,
    tile_h: u32,
    version: u32,
}

/// `<root>/<stem>/c-00012.k.jpg` (`Fill::Full`: `.f.jpg`).
pub fn chunk_path(root: &Path, stem: &str, chunk: u32, fill: Fill) -> PathBuf {
    root.join(stem)
        .join(format!("c-{chunk:05}.{}.jpg", fill.tag()))
}

/// Chunk `chunk` is on disk (from either fill).
pub fn has_chunk(root: &Path, stem: &str, chunk: u32) -> bool {
    [Fill::Keys, Fill::Full]
        .into_iter()
        .any(|fill| chunk_path(root, stem, chunk, fill).exists())
}

/// Writes one finished chunk (and `strip.json` the first time), each atomically through `steps`.
pub fn write_chunk(
    steps: &dyn Steps,
    root: &Path,
    stem: &str,
    sprite: &Sprite,
    fill: Fill,
) -> io::Result<()> {
    let dir = root.join(stem);
    std::fs::create_dir_all(&dir)?;
    let strip = dir.join(STRIP_FILE);
    if !strip.exists() {
        let (tile_w, tile_h) = sprite.tile();
        let json = serde_json::to_vec(&StripFile {
            tile_w,
            tile_h,
            version: VERSION,
        })
        .map_err(io::Error::other)?;
        write_atomic_with(steps, &strip, &json, Durability::Ordered)?;
    }
    let jpeg = encode_jpeg(sprite).map_err(io::Error::other)?;
    write_atomic_with(
        steps,
        &chunk_path(root, stem, sprite.chunk, fill),
        &jpeg,
        Durability::Ordered,
    )
}

/// The chunk index and fill of a file name `c-00012.k.jpg`.
fn parse_chunk(name: &str) -> Option<(u32, Fill)> {
    let rest = name.strip_prefix("c-")?.strip_suffix(".jpg")?;
    let (index, tag) = rest.split_once('.')?;
    let fill = match tag {
        "k" => Fill::Keys,
        "f" => Fill::Full,
        _ => return None,
    };
    Some((index.parse().ok()?, fill))
}

/// `path`'s finished chunks, `.f` preferred over `.k`, by index; None until its first chunk is written.
pub fn strip(root: &Path, path: &str) -> Option<ThumbStrip> {
    let dir = root.join(stem(path, &CacheKind::Thumbs)?);
    let file: StripFile =
        serde_json::from_slice(&std::fs::read(dir.join(STRIP_FILE)).ok()?).ok()?;
    let mut chunks: Vec<ThumbChunk> = Vec::new();
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some((index, fill)) = name.to_str().and_then(parse_chunk) else {
            continue;
        };
        let full = fill == Fill::Full;
        let chunk = ThumbChunk {
            index,
            path: entry.path().to_string_lossy().into_owned(),
            full,
        };
        match chunks.iter_mut().find(|c| c.index == index) {
            Some(known) if full => *known = chunk,
            Some(_) => {}
            None => chunks.push(chunk),
        }
    }
    chunks.sort_by_key(|c| c.index);
    Some(ThumbStrip {
        tile_w: file.tile_w,
        tile_h: file.tile_h,
        cols: COLS,
        per_chunk: PER_CHUNK,
        slot_us: SLOT_US,
        chunks,
    })
}
