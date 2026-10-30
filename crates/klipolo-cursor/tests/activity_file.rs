//! The activity file: 60-second chunks with checksums, read back whole or as unknown, and a
//! pass that stops anywhere resumes at the chunk it was writing.

mod common;

use common::activity::{S, activity_of, file_of, numbered};
use klipolo_cursor::Us;
use klipolo_cursor::activity::{
    CHUNK_TICKS, TICK_BYTES, Tick, decode, encode_chunk, header, missing, stored_duration,
};

/// A whole chunk's bytes: index, count, the ticks and the checksum.
const CHUNK_BYTES: usize = 4 + 2 + 600 * TICK_BYTES + 4;

#[test]
fn a_damaged_chunk_reads_as_unknown_and_is_missing_again() {
    // 150 s: chunks 0 and 1 of 600 ticks, chunk 2 of 300.
    let ticks = numbered(1_500);
    let bytes = file_of(&ticks);
    let whole = decode(&bytes, 150 * S);
    assert_eq!(whole.duration_us(), 150 * S);
    assert!(whole.is_complete() && missing(&whole).is_empty());
    assert_eq!(whole.complete_through(), 150 * S);
    assert_eq!(whole.tick_at(70 * S), Some(ticks[700]));
    assert_eq!(whole.append_at(), bytes.len());

    let head = bytes.len() - 2 * CHUNK_BYTES - (4 + 2 + 300 * TICK_BYTES + 4);
    let chunk1 = head + CHUNK_BYTES;
    let mut damaged = bytes.clone();
    damaged[chunk1 + 6 + 100 * TICK_BYTES] ^= 0x10;
    let a = decode(&damaged, 150 * S);
    assert_eq!(missing(&a), vec![1]);
    assert_eq!(a.tick_at(70 * S), None);
    assert_eq!(a.tick_at(10 * S), Some(ticks[100]));
    assert_eq!(a.tick_at(130 * S), Some(ticks[1_300]));
    assert_eq!(a.complete_through(), 60 * S);
    assert!(!a.is_complete());
    // Chunk 2 is whole behind it, so the pass appends chunk 1 at the end: the later copy is read.
    assert_eq!(a.append_at(), bytes.len());
    encode_chunk(1, &ticks[600..1_200], &mut damaged);
    assert!(decode(&damaged, 150 * S).is_complete());

    // A damaged index (chunk 1 reading as 0) never places a chunk at the wrong time.
    let mut moved = bytes.clone();
    moved[chunk1] ^= 0x01;
    let moved = decode(&moved, 150 * S);
    assert_eq!(missing(&moved), vec![1]);
    assert_eq!(moved.tick_at(10 * S), Some(ticks[100]));
    // A write cut off: the short tail is ignored, so its chunk is missing.
    assert_eq!(
        missing(&decode(&bytes[..bytes.len() - 5], 150 * S)),
        vec![2]
    );
    // A damaged header, or a file for another duration, reads as all missing, to be written again from the start.
    let mut head_damaged = bytes.clone();
    head_damaged[9] ^= 0x01;
    for a in [decode(&head_damaged, 150 * S), decode(&bytes, 151 * S)] {
        assert_eq!((a.append_at(), missing(&a)), (0, vec![0, 1, 2]));
        assert!(!a.is_complete() && a.complete_through() == 0);
    }
    // The screen file's own duration, as its header stores it (reading a finished file uses it); None when the
    // header can't be read.
    assert_eq!(stored_duration(&bytes), Some(150 * S));
    assert_eq!(stored_duration(&head_damaged), None);
    assert_eq!(stored_duration(&bytes[..10]), None);
    // A last chunk with a tick more than the duration holds is still made: the extra tick is left out.
    let mut long = Vec::new();
    header(150 * S, &mut long);
    let extra = numbered(1_501);
    for (i, chunk) in extra.chunks(600).enumerate() {
        encode_chunk(i as u32, chunk, &mut long);
    }
    let long = decode(&long, 150 * S);
    assert!(long.is_complete());
    assert_eq!(long.tick_at(149_950_000), Some(extra[1_499]));
}

/// A resumed pass over `file`, for `ticks` of a video: truncate to `append_at` (a new header at 0), then append every
/// missing chunk.
fn resume(mut file: Vec<u8>, ticks: &[Tick]) -> Vec<u8> {
    let duration = ticks.len() as Us * 100_000;
    let a = decode(&file, duration);
    file.truncate(a.append_at());
    if file.is_empty() {
        header(duration, &mut file);
    }
    for c in missing(&a) {
        let from = c as usize * 600;
        encode_chunk(c, &ticks[from..(from + 600).min(ticks.len())], &mut file);
    }
    file
}

#[test]
fn a_pass_cut_off_mid_chunk_resumes_whole_after_one_more_pass() {
    // 180 s: three whole chunks; the pass is cut off while writing chunk 2.
    let ticks = numbered(1_800);
    let bytes = file_of(&ticks);
    let chunk2 = bytes.len() - CHUNK_BYTES;
    // Inside its index and count, inside its ticks, and inside its checksum.
    for cut in [chunk2 + 3, chunk2 + 6 + 1_000, bytes.len() - 2] {
        let file = resume(bytes[..cut].to_vec(), &ticks);
        assert!(decode(&file, 180 * S).is_complete(), "cut at {cut}");
        assert_eq!(
            file, bytes,
            "chunk 2 is written where it was cut (cut at {cut})"
        );
        assert_eq!(
            resume(file.clone(), &ticks),
            file,
            "the next pass changes nothing"
        );
    }
    // Chunk 1's count damaged past a minute: the chunks behind it are lost once, made again in one pass.
    let mut damaged = bytes.clone();
    let count1 = bytes.len() - 2 * CHUNK_BYTES + 4;
    damaged[count1..count1 + 2].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(missing(&decode(&damaged, 180 * S)), vec![1, 2]);
    let file = resume(damaged, &ticks);
    assert_eq!(file, bytes);
    assert_eq!(resume(file.clone(), &ticks), file);
    // A damaged header: the whole file is written again, once.
    let mut head_damaged = bytes.clone();
    head_damaged[0] ^= 0x01;
    let file = resume(head_damaged, &ticks);
    assert_eq!(file, bytes);
    assert_eq!(resume(file.clone(), &ticks), file);
}

#[test]
fn ticks_cost_fourteen_bytes() {
    assert_eq!(TICK_BYTES, 14);
    assert_eq!(std::mem::size_of::<Tick>(), TICK_BYTES);
    let mut chunk = Vec::new();
    encode_chunk(0, &[Tick::default(); CHUNK_TICKS as usize], &mut chunk);
    assert_eq!(chunk.len(), 4 + 2 + 600 * 14 + 4);
    // An hour in memory: 14 bytes a tick, and little else (≤ 0.5 MB an hour).
    let hour = activity_of(&vec![Tick::default(); 36_000]);
    assert!(hour.bytes() <= 36_000 * 14 + 1_024, "{}", hour.bytes());
}
