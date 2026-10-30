//! Intra-only video (ProRes, MJPEG, DNxHD…): every frame is a keyframe, so a scan seeks once per one-second slot and
//! decodes the frame there instead of reading every frame; the keyframe index comes from the demuxer's sample table
//! when that covers every frame.

use super::cut::Cutter;
use super::{ScanEnd, next_packet, start_us};
use crate::media::sprite::{PER_CHUNK, SLOT_US};

/// Intra-only video: one seek and one decoded frame per slot of each missing chunk.
pub(super) fn slots(
    cutter: &mut Cutter,
    input: &mut ffmpeg::format::context::Input,
    index: usize,
    missing: &[u32],
    proceed: &dyn Fn() -> bool,
) -> ScanEnd {
    if cutter.want_keys() {
        cutter.store_index(index_keys(input, index));
    }
    let start = start_us(input);
    let slots_in_file = (input.duration().max(0) as u64).div_ceil(SLOT_US as u64);
    cutter.begin_slots();
    for &chunk in missing {
        let first = u64::from(chunk) * u64::from(PER_CHUNK);
        let last = (first + u64::from(PER_CHUNK)).min(slots_in_file.max(first + 1));
        let mut before: Option<i64> = None;
        for slot in first..last {
            if !proceed() {
                return ScanEnd::Stopped;
            }
            let ts = slot as i64 * SLOT_US + start;
            if input.seek(ts, ..ts).is_err() {
                continue;
            }
            let packet = loop {
                match next_packet(input) {
                    Ok(Some((stream, packet))) if stream == index => break Some(packet),
                    Ok(Some(_)) => {}
                    Ok(None) => break None,
                    Err(e) => return ScanEnd::Unreadable(e.to_string()),
                }
            };
            let Some(packet) = packet else { continue };
            let same = packet.pts().is_some() && packet.pts() == before;
            before = packet.pts();
            cutter.slot(&packet, slot, same);
            if cutter.stopped() {
                return ScanEnd::Done;
            }
        }
        cutter.end_slots();
    }
    ScanEnd::Done
}

/// The keyframes of stream `index` from the demuxer's sample table (MOV, MP4, AVI and the like know every sample
/// without reading it). Intra-only frames are never reordered, so a sample's time is its pts, as a demux reads it.
/// None when the table can't be shown to hold every frame: an intra-only stream's keyframes are all its frames, so
/// their count must be the frame count the container states. Matroska (no stated count; its table grows from the Cues
/// and the packets read so far) and the like have decoders build their own.
fn index_keys(input: &mut ffmpeg::format::context::Input, index: usize) -> Option<Vec<i64>> {
    use ffmpeg::ffi::{
        AVINDEX_KEYFRAME, avformat_index_get_entries_count, avformat_index_get_entry,
    };
    let frames = input.stream(index)?.frames();
    let mut stream = input.stream_mut(index)?;
    // SAFETY: the stream and its index belong to `input`, which outlives these reads; FFmpeg returns null past the
    // end.
    unsafe {
        let st = stream.as_mut_ptr();
        let count = avformat_index_get_entries_count(st);
        let mut keys = Vec::with_capacity(usize::try_from(count).ok()?);
        for i in 0..count {
            let entry = avformat_index_get_entry(st, i);
            if !entry.is_null() && (*entry).flags() & AVINDEX_KEYFRAME != 0 {
                keys.push((*entry).timestamp);
            }
        }
        (frames > 0 && keys.len() as i64 == frames).then_some(keys)
    }
}
