//! A file's thumbnail sprites on disk: `<root>/<stem>/` holds `strip.json` (the tile size), one JPEG per finished
//! chunk (`c-00012.k.jpg` from keyframes, `c-00012.f.jpg` from an editing copy's full decode), the keyframe index
//! (`keyframes::KEYS_FILE`) and the poster (`poster-480x270.jpg`: the first picture, bigger than its tile, for the
//! start screen's cards). Every file is written atomically, so a crash leaves whole chunks only.

use std::io;
use std::path::{Path, PathBuf};

use klipolo_model::time::Us;
use klipolo_platform::durable::{Durability, Steps, write_atomic_with};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::key::{CacheKind, stem};
use super::sprite::{COLS, PER_CHUNK, SLOT_US, Sprite};
use super::still::{encode_jpeg, encode_planes};

const STRIP_FILE: &str = "strip.json";
const VERSION: u32 = 1;

/// The thumbnails' folder in a media cache (`GpuBackend::with_media_cache`'s folder): the one place this is named, so
/// the engine that writes them and the app that serves them read the same layout.
pub fn root_in(media_cache: &Path) -> PathBuf {
    media_cache.join("thumbs")
}

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

/// The box a poster fits in: twice a start screen card's 240 × 135 point picture, so it is sharp on a Retina screen.
pub const POSTER_BOX: (u32, u32) = (480, 270);

/// The poster for a picture shown `w × h`: fitted inside `POSTER_BOX`, never bigger than the picture itself, with even
/// sides (4:2:0). A picture of unknown size gets the whole box.
pub fn poster_size(w: u32, h: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return POSTER_BOX;
    }
    let (box_w, box_h) = POSTER_BOX;
    let scale = (f64::from(box_w) / f64::from(w))
        .min(f64::from(box_h) / f64::from(h))
        .min(1.0);
    let even = |side: u32| ((f64::from(side) * scale / 2.0).round() as u32 * 2).max(2);
    (even(w), even(h))
}

/// A file's poster on disk: one JPEG, `w × h`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Poster {
    pub path: String,
    pub w: u32,
    pub h: u32,
}

/// `<root>/<stem>/poster-480x270.jpg`: the size is in its name, so one atomic write holds both.
pub fn poster_path(root: &Path, stem: &str, (w, h): (u32, u32)) -> PathBuf {
    root.join(stem).join(format!("poster-{w}x{h}.jpg"))
}

/// The size in a poster's file name (`poster-480x270.jpg`).
fn parse_poster(name: &str) -> Option<(u32, u32)> {
    let (w, h) = name
        .strip_prefix("poster-")?
        .strip_suffix(".jpg")?
        .split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// The poster in `<root>/<stem>/`; None until a scan has written it.
pub fn poster_in(root: &Path, stem: &str) -> Option<Poster> {
    std::fs::read_dir(root.join(stem))
        .ok()?
        .flatten()
        .find_map(|entry| {
            let (w, h) = parse_poster(entry.file_name().to_str()?)?;
            Some(Poster {
                path: entry.path().to_string_lossy().into_owned(),
                w,
                h,
            })
        })
}

/// `path`'s poster; None until a scan has written it.
pub fn poster(root: &Path, path: &str) -> Option<Poster> {
    poster_in(root, &stem(path, &CacheKind::Thumbs)?)
}

/// Writes the poster: a full-range 4:2:0 picture of `size`, its Y, U and V rows `strides` apart, atomically through
/// `steps`.
pub fn write_poster(
    steps: &dyn Steps,
    root: &Path,
    stem: &str,
    size: (u32, u32),
    planes: [&[u8]; 3],
    strides: [usize; 3],
) -> io::Result<()> {
    std::fs::create_dir_all(root.join(stem))?;
    let jpeg = encode_planes(size, planes, strides).map_err(io::Error::other)?;
    write_atomic_with(
        steps,
        &poster_path(root, stem, size),
        &jpeg,
        Durability::Ordered,
    )
}

#[cfg(test)]
mod tests {
    use klipolo_platform::durable::Os;

    use super::*;

    #[test]
    fn a_poster_fits_480_by_270_and_never_grows() {
        assert_eq!(poster_size(1920, 1080), (480, 270));
        assert_eq!(poster_size(640, 360), (480, 270));
        // A phone picture, shown upright: as tall as the box.
        assert_eq!(poster_size(720, 1280), (152, 270));
        assert_eq!(poster_size(2560, 1600), (432, 270));
        // Wider than 16:9: as wide as the box, with even sides.
        assert_eq!(poster_size(3840, 1080), (480, 136));
        // A small picture keeps its own size.
        assert_eq!(poster_size(320, 180), (320, 180));
        assert_eq!(poster_size(1, 1), (2, 2));
        assert_eq!(poster_size(0, 0), (480, 270));
    }

    #[test]
    fn a_written_poster_reads_back_with_its_size() {
        let root = tempfile::tempdir().unwrap();
        let (w, h) = (16, 8);
        let luma = vec![128; 16 * 8];
        let chroma = vec![128; 8 * 4];
        assert_eq!(poster_in(root.path(), "clip"), None);
        // A poster cut off mid-write (a crash) isn't one.
        std::fs::create_dir_all(root.path().join("clip")).unwrap();
        std::fs::write(
            root.path().join("clip").join("poster-16x8.jpg.42.tmp"),
            b"half",
        )
        .unwrap();
        assert_eq!(poster_in(root.path(), "clip"), None);

        write_poster(
            &Os,
            root.path(),
            "clip",
            (w, h),
            [&luma, &chroma, &chroma],
            [16, 8, 8],
        )
        .unwrap();
        let found = poster_in(root.path(), "clip").expect("a poster");
        assert_eq!((found.w, found.h), (16, 8));
        assert_eq!(
            Path::new(&found.path),
            poster_path(root.path(), "clip", (16, 8))
        );
        let bytes = std::fs::read(&found.path).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "a JPEG");
        // The chunks beside it are read as before.
        assert_eq!(parse_chunk("poster-16x8.jpg"), None);
    }
}
