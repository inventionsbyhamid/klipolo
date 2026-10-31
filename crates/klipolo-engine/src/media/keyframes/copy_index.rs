//! An editing copy's own keyframe index, beside it (`<proxies>/<stem>.keys`): written from the copy's sample table as
//! the worker puts the copy in place, and named by the copy's size and file number (see `keyframes`).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use klipolo_platform::durable::Durability;

use super::{StoredKeys, decode, encode, take};
use crate::media::key::SourceId;

/// An index beside its own file: this magic, the file's size u64 and number u64, then a `KLPOKEY3` index.
const BESIDE_MAGIC: &[u8; 8] = b"KLPOCPY1";

/// Where a file's own index lies beside it (`clip.mp4` → `clip.keys`).
pub fn beside(path: &Path) -> PathBuf {
    path.with_extension("keys")
}

/// The index beside `path`, if it was written for this very file (same size and number). Only its 24-byte header is
/// read until it shows that (decoders ask this of every file, and a user's folder may hold its own `.keys`). One
/// written for this file that doesn't read (under another build's magic or layout, or damaged) is made again
/// (`made_again`).
pub fn load_beside(path: &Path) -> Option<StoredKeys> {
    use std::io::Read;
    let mut file = std::fs::File::open(beside(path)).ok()?;
    let mut header = [0u8; 24];
    file.read_exact(&mut header).ok()?;
    let (magic, mut rest) = header.split_at(8);
    let len = u64::from_le_bytes(take(&mut rest)?);
    let file_id = u64::from_le_bytes(take(&mut rest)?);
    let now = SourceId::of(&std::fs::metadata(path).ok()?);
    if (now.len, now.file_id) != (len, file_id) {
        return None;
    }
    if magic != BESIDE_MAGIC.as_slice() {
        drop(file);
        return made_again(path);
    }
    let mut body = Vec::new();
    file.read_to_end(&mut body).ok()?;
    drop(file);
    decode(&body).or_else(|| made_again(path))
}

/// The index of `path`, whose own index beside it is ours (its header names this very file) but doesn't read: made
/// again from the file's sample table, as the copy worker made it (no packet is read), and written over the old one
/// (an editing copy whose index didn't read was otherwise demuxed whole at each decoder open).
fn made_again(path: &Path) -> Option<StoredKeys> {
    let keys = from_sample_table(path)?;
    if let Err(e) = write_beside(path, &keys) {
        tracing::warn!(file = %crate::log_name(path), %e, "an editing copy's index couldn't be written again");
    }
    Some(keys)
}

/// Writes `keys` beside `path` (`beside`), naming the file by its size and number. One write at a time in a process:
/// decoder threads may make the same index again at once, and an atomic write's temporary name is the process's.
pub fn write_beside(path: &Path, keys: &StoredKeys) -> io::Result<()> {
    static WRITING: Mutex<()> = Mutex::new(());
    let _one = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let id = SourceId::of(&std::fs::metadata(path)?);
    let mut bytes = Vec::with_capacity(24 + 64 + keys.pts.len() * 8);
    bytes.extend_from_slice(BESIDE_MAGIC);
    bytes.extend_from_slice(&id.len.to_le_bytes());
    bytes.extend_from_slice(&id.file_id.to_le_bytes());
    bytes.extend_from_slice(&encode(keys));
    klipolo_platform::durable::write_atomic(&beside(path), &bytes, Durability::Ordered)
}

/// `path`'s video keyframes from its container's sample table, without reading a packet (MP4 and MOV list every
/// sample), with what the index keeps about the stream. Only for a stream whose frames aren't reordered (an editing
/// copy: no B-frames), where a sample's time is its pts. None for one with B-frames, or when the table doesn't hold
/// every frame the stream states.
pub fn from_sample_table(path: &Path) -> Option<StoredKeys> {
    use ffmpeg::ffi::{
        AVINDEX_KEYFRAME, avformat_index_get_entries_count, avformat_index_get_entry,
    };
    let mut input = crate::media::open::input(path).ok()?;
    let (index, mut keys) = {
        let stream = input.streams().best(ffmpeg::media::Type::Video)?;
        let decoder = ffmpeg::codec::Context::from_parameters(stream.parameters())
            .and_then(|c| c.decoder().video())
            .ok()?;
        // A sample's time is its decode time: it is its pts only when frames aren't reordered (no B-frames).
        // SAFETY: the parameters belong to `input`, which outlives this read; a plain field.
        if unsafe { (*stream.parameters().as_ptr()).video_delay } > 0 {
            return None;
        }
        let tb = stream.time_base();
        let rate = stream.avg_frame_rate();
        let keys = StoredKeys {
            pts: Vec::new(),
            time_base: (tb.numerator(), tb.denominator()),
            start: crate::render::probe::stream_start(&stream),
            fps: if rate.denominator() == 0 {
                30.0
            } else {
                f64::from(rate.numerator()) / f64::from(rate.denominator())
            },
            duration_secs: if stream.duration() > 0 {
                stream.duration() as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
            } else {
                0.0
            },
            pixel_format: decoder
                .format()
                .descriptor()
                .map_or("", |d| d.name())
                .to_owned(),
            codec: stream.parameters().id().name().to_owned(),
            width: decoder.width(),
            height: decoder.height(),
        };
        (stream.index(), keys)
    };
    let frames = input.stream(index)?.frames();
    let mut stream = input.stream_mut(index)?;
    // SAFETY: the stream and its index belong to `input`, which outlives these reads; FFmpeg returns null past the end.
    unsafe {
        let st = stream.as_mut_ptr();
        let count = avformat_index_get_entries_count(st);
        for i in 0..count {
            let entry = avformat_index_get_entry(st, i);
            if !entry.is_null() && (*entry).flags() & AVINDEX_KEYFRAME != 0 {
                keys.pts.push((*entry).timestamp);
            }
        }
        (frames > 0 && i64::from(count) == frames && !keys.pts.is_empty()).then_some(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An index beside its copy that doesn't read (another build's magic, outside or inside)
    /// is made again from the copy's sample table and written over, so the next open loads it as it is and no decoder demuxes the copy
    /// whole. A `.keys` that isn't this copy's (someone's own file, or one for another version of the copy) is left as
    /// it is.
    #[test]
    fn an_index_beside_its_copy_that_doesnt_read_is_made_again_once_and_others_are_left_alone() {
        let _ = ffmpeg::init();
        let dir = tempfile::tempdir().unwrap();
        // An editing copy as the worker leaves it: frames in order, its index beside it.
        let copy = dir.path().join("0123456789abcdef.mov");
        std::fs::copy(
            crate::fixture_set::fixture_dir().join("intra-6s-mjpeg.mov"),
            &copy,
        )
        .unwrap();
        let keys = from_sample_table(&copy).expect("a MOV's sample table lists every frame");
        write_beside(&copy, &keys).unwrap();
        let index = beside(&copy);
        let mut old = std::fs::read(&index).unwrap();
        old[24..32].copy_from_slice(b"XXXXKEY3");
        std::fs::write(&index, &old).unwrap();

        assert_eq!(load_beside(&copy), Some(keys.clone()), "made again");
        // Under another build's outer magic, for this very file: made again too.
        let mut other_magic = std::fs::read(&index).unwrap();
        other_magic[..8].copy_from_slice(b"XXXXCPY1");
        std::fs::write(&index, &other_magic).unwrap();
        assert_eq!(
            load_beside(&copy),
            Some(keys.clone()),
            "made again from another magic"
        );
        assert_eq!(
            &std::fs::read(&index).unwrap()[..8],
            BESIDE_MAGIC,
            "written over"
        );
        let now = std::fs::read(&index).unwrap();
        assert_eq!(decode(&now[24..]), Some(keys.clone()), "written over");
        // Decoder threads finding an older index at once all get it, and it is left whole.
        std::fs::write(&index, &old).unwrap();
        std::thread::scope(|s| {
            let opens: Vec<_> = (0..8).map(|_| s.spawn(|| load_beside(&copy))).collect();
            for open in opens {
                assert_eq!(open.join().unwrap(), Some(keys.clone()));
            }
        });
        assert_eq!(decode(&std::fs::read(&index).unwrap()[24..]), Some(keys));

        let mut other_version = old.clone();
        other_version[8..16].copy_from_slice(&1u64.to_le_bytes());
        for not_this_copys in [b"my own notes about this clip".to_vec(), other_version] {
            std::fs::write(&index, &not_this_copys).unwrap();
            assert_eq!(load_beside(&copy), None);
            assert_eq!(
                std::fs::read(&index).unwrap(),
                not_this_copys,
                "left as it is"
            );
        }
    }
}
