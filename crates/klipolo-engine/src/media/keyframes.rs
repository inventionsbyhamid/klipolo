//! The keyframe index, recorded by the thumbnail scan. A scan already reads every keyframe, so it keeps
//! their timestamps and stores them beside the file's sprites (`<thumbs>/<stem>/keys.bin`, written atomically once a
//! scan has seen them all; for intra-only video, from the demuxer's sample table). A preview decoder that seeks by keyframe (macOS: the vendored AVAssetReader's
//! `KeyframeIndex::load_or_build`) reads them instead of demuxing the whole file at open; without a stored index it
//! builds one as before.
//!
//! An editing copy carries its own index beside it (`<proxies>/<stem>.keys`), written by the worker
//! as it puts the copy in place, from the copy's sample table (no packet is read): a decoder of the copy never demuxes
//! it whole. It names the copy by what it is (its stem, which is its job's, and its size and file number), not its
//! modification time, which the copies' least-recently-used order touches.
//!
//! File layout (little-endian): magic `KLPOKEY3`, time base (i32 num, i32 den), the stream's start (i64 pts), fps f64,
//! duration in seconds f64, width u32, height u32, the decoder's pixel format name (u8 length, bytes), the codec's name
//! (u8 length, bytes), a count u32, then each keyframe's pts (i64, the stream's time base), in order. (Older files,
//! without the codec or the start, no longer read: their index is built again.)

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

use klipolo_platform::durable::{Durability, Steps, write_atomic_with};

use super::key::{CacheKind, stem};

mod copy_index;
mod recorder;
pub use copy_index::{beside, from_sample_table, load_beside, write_beside};
pub(crate) use recorder::KeyRecorder;

pub const KEYS_FILE: &str = "keys.bin";
/// Marks a file whose index can't be had without reading every packet (intra-only video in a container without a
/// sample table): decoders build theirs as before, and no scan tries again.
const NONE_FILE: &str = "keys.none";
const MAGIC: &[u8; 8] = b"KLPOKEY3";

/// The thumbnails folder of this process's media cache (`GpuBackend::with_media_cache`). None (the export worker,
/// tests): decoders build their index as before.
static ROOT: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Where stored indexes are looked up (the media cache's thumbnails folder).
pub fn set_root(root: Option<PathBuf>) {
    *ROOT.write().unwrap_or_else(PoisonError::into_inner) = root;
}

/// What the keyframe index is rebuilt from without a demux: the best video stream's keyframes and what the index
/// keeps about the stream.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredKeys {
    pub pts: Vec<i64>,
    pub time_base: (i32, i32),
    /// Where the stream starts (its first pts; 0 when the file doesn't say): every decoder's frame 0.
    pub start: i64,
    pub fps: f64,
    pub duration_secs: f64,
    /// FFmpeg's name for the decoder's pixel format (`yuv420p`); empty when unknown.
    pub pixel_format: String,
    /// FFmpeg's name for the codec (`hevc`): with the pixel format, it decides what the preview's reader asks for
    /// (10-bit HEVC is read as 8-bit through VideoToolbox), once per file.
    pub codec: String,
    pub width: u32,
    pub height: u32,
}

/// The stored index of `path` as it is now: the one beside it (an editing copy's, still that file), else the one a
/// thumbnail pass recorded (its size and modification time are in the folder's name).
pub fn load(path: &Path) -> Option<StoredKeys> {
    if let Some(keys) = load_beside(path) {
        return Some(keys);
    }
    let root = ROOT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    read(&root, &stem(path.to_str()?, &CacheKind::Thumbs)?)
}

/// The index stored in `<root>/<stem>/`.
pub(crate) fn read(root: &Path, stem: &str) -> Option<StoredKeys> {
    decode(&std::fs::read(root.join(stem).join(KEYS_FILE)).ok()?)
}

/// The index is stored in a form this build reads (or known not to be worth storing) for `<root>/<stem>/`. An index
/// that doesn't load (an older format, a damaged file) counts as missing, so the next scan writes it again once,
/// rather than every preview decoder of the file reading the whole file to build its own.
pub(crate) fn has(root: &Path, stem: &str) -> bool {
    root.join(stem).join(NONE_FILE).exists() || read(root, stem).is_some()
}

/// Marks `<root>/<stem>/` as having no stored index (see `NONE_FILE`).
pub(crate) fn write_none(steps: &dyn Steps, root: &Path, stem: &str) -> io::Result<()> {
    let dir = root.join(stem);
    std::fs::create_dir_all(&dir)?;
    write_atomic_with(steps, &dir.join(NONE_FILE), &[], Durability::Ordered)
}

pub(crate) fn write(
    steps: &dyn Steps,
    root: &Path,
    stem: &str,
    keys: &StoredKeys,
) -> io::Result<()> {
    let dir = root.join(stem);
    std::fs::create_dir_all(&dir)?;
    write_atomic_with(
        steps,
        &dir.join(KEYS_FILE),
        &encode(keys),
        Durability::Ordered,
    )
}

fn encode(k: &StoredKeys) -> Vec<u8> {
    let name = &k.pixel_format.as_bytes()[..k.pixel_format.len().min(255)];
    let codec = &k.codec.as_bytes()[..k.codec.len().min(255)];
    let mut out = Vec::with_capacity(64 + name.len() + codec.len() + k.pts.len() * 8);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&k.time_base.0.to_le_bytes());
    out.extend_from_slice(&k.time_base.1.to_le_bytes());
    out.extend_from_slice(&k.start.to_le_bytes());
    out.extend_from_slice(&k.fps.to_le_bytes());
    out.extend_from_slice(&k.duration_secs.to_le_bytes());
    out.extend_from_slice(&k.width.to_le_bytes());
    out.extend_from_slice(&k.height.to_le_bytes());
    out.push(name.len() as u8);
    out.extend_from_slice(name);
    out.push(codec.len() as u8);
    out.extend_from_slice(codec);
    out.extend_from_slice(&(k.pts.len() as u32).to_le_bytes());
    for pts in &k.pts {
        out.extend_from_slice(&pts.to_le_bytes());
    }
    out
}

/// The next `N` bytes of `rest`.
fn take<const N: usize>(rest: &mut &[u8]) -> Option<[u8; N]> {
    let (head, tail) = rest.split_first_chunk::<N>()?;
    *rest = tail;
    Some(*head)
}

fn decode(bytes: &[u8]) -> Option<StoredKeys> {
    let mut rest = bytes.strip_prefix(MAGIC.as_slice())?;
    let time_base = (
        i32::from_le_bytes(take(&mut rest)?),
        i32::from_le_bytes(take(&mut rest)?),
    );
    let start = i64::from_le_bytes(take(&mut rest)?);
    let fps = f64::from_le_bytes(take(&mut rest)?);
    let duration_secs = f64::from_le_bytes(take(&mut rest)?);
    let width = u32::from_le_bytes(take(&mut rest)?);
    let height = u32::from_le_bytes(take(&mut rest)?);
    let [name_len] = take::<1>(&mut rest)?;
    let (name, tail) = rest.split_at_checked(usize::from(name_len))?;
    rest = tail;
    let pixel_format = std::str::from_utf8(name).ok()?.to_owned();
    let [codec_len] = take::<1>(&mut rest)?;
    let (codec, tail) = rest.split_at_checked(usize::from(codec_len))?;
    rest = tail;
    let codec = std::str::from_utf8(codec).ok()?.to_owned();
    let count = u32::from_le_bytes(take(&mut rest)?) as usize;
    let pts = rest
        .get(..count.checked_mul(8)?)?
        .chunks_exact(8)
        .map(|b| i64::from_le_bytes(b.try_into().unwrap_or_default()))
        .collect();
    (time_base.1 != 0).then_some(StoredKeys {
        pts,
        time_base,
        start,
        fps,
        duration_secs,
        pixel_format,
        codec,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_reads_back_as_it_was_written() {
        let keys = StoredKeys {
            pts: vec![2_100, 2_101, 2_143, 2_144, 2_200],
            time_base: (1, 30_000),
            start: 2_100,
            fps: 30.0,
            duration_secs: 130.0,
            pixel_format: "yuv420p".into(),
            codec: "h264".into(),
            width: 1280,
            height: 720,
        };
        assert_eq!(decode(&encode(&keys)), Some(keys));
    }
}
