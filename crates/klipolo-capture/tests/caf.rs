mod common;
use common::decode::decode_audio_frames;
use klipolo_capture::caf;
use klipolo_capture::io::OsIo;
use klipolo_capture::media::{AudioFormat, PcmBlock};
use std::io::{Cursor, Write};

const MONO: AudioFormat = AudioFormat {
    rate: 48_000,
    channels: 1,
};

#[test]
fn the_header_says_unknown_length_little_endian_24_bit_pcm() {
    let h = caf::header(MONO);
    assert_eq!(h.len() as u64, caf::HEADER_LEN);
    assert_eq!(&h[..4], b"caff");
    assert_eq!(&h[8..12], b"desc");
    assert_eq!(&h[28..32], b"lpcm");
    assert_eq!(
        u32::from_be_bytes(h[32..36].try_into().unwrap()) & 2,
        2,
        "little-endian flag"
    );
    assert_eq!(&h[52..56], b"data");
    assert_eq!(i64::from_be_bytes(h[56..64].try_into().unwrap()), -1);
    assert_eq!(
        u32::from_be_bytes(h[64..68].try_into().unwrap()),
        0,
        "the edit count"
    );
}

#[test]
fn samples_become_24_bit_and_clip_at_full_scale() {
    let bytes = caf::encode(&PcmBlock {
        host_ns: 0,
        samples: vec![0.0, 1.0, -1.0, 2.0, 0.5],
    });
    let v: Vec<i32> = bytes
        .chunks(3)
        .map(|b| (i32::from_le_bytes([b[0], b[1], b[2], 0]) << 8) >> 8)
        .collect();
    assert_eq!(v, [0, 8_388_607, -8_388_607, 8_388_607, 4_194_304]);
}

#[test]
fn a_crashed_file_reads_to_its_last_whole_frame() {
    let mut bytes = caf::header(AudioFormat {
        rate: 48_000,
        channels: 2,
    });
    bytes.extend(caf::encode(&PcmBlock {
        host_ns: 0,
        samples: vec![0.1; 2 * 480],
    }));
    bytes.truncate(bytes.len() - 4); // half a stereo frame and a bit
    let scan = caf::scan(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
    assert_eq!((scan.frames, scan.sized), (479, false));
}

#[test]
fn a_zero_tail_is_trimmed_only_past_the_durable_mark() {
    let mut bytes = caf::header(MONO);
    bytes.extend(caf::encode(&PcmBlock {
        host_ns: 0,
        samples: vec![0.0; 100],
    })); // real silence
    bytes.extend(caf::encode(&PcmBlock {
        host_ns: 0,
        samples: vec![0.2; 100],
    }));
    bytes.extend(vec![0u8; 3 * 400]); // never written: zeros after a power cut
    let scan = caf::scan(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
    assert_eq!(
        caf::keep_frames(&mut Cursor::new(&bytes), &scan, 200).unwrap(),
        200
    );
    assert_eq!(
        caf::keep_frames(&mut Cursor::new(&bytes), &scan, 600).unwrap(),
        600,
        "zeros before the mark are kept"
    );
}

#[test]
fn finalize_sizes_the_data_and_ffmpeg_reads_every_frame() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run-01.caf");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(&caf::header(MONO)).unwrap();
    for _ in 0..100 {
        f.write_all(&caf::encode(&PcmBlock {
            host_ns: 0,
            samples: vec![0.25; 480],
        }))
        .unwrap();
    }
    assert_eq!(
        decode_audio_frames(&path),
        48_000,
        "readable before finalize"
    );
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let len = file.metadata().unwrap().len();
    let scan = caf::scan(&mut &file, len).unwrap();
    caf::finalize(&OsIo, &file, &path, &scan, scan.frames).unwrap();
    let rescanned = caf::scan(&mut &file, len).unwrap();
    assert_eq!((rescanned.frames, rescanned.sized), (48_000, true));
    assert_eq!(decode_audio_frames(&path), 48_000);
}

/// Recovery's steps on a power-cut file: the scan, the trim past the durable mark, then `finalize`, which cuts only
/// silence and a torn frame, never pads, and finishes on a re-run.
#[test]
fn finalize_cuts_only_a_zero_tail_and_a_torn_frame_and_reruns_cleanly() {
    const STEREO: AudioFormat = AudioFormat {
        rate: 48_000,
        channels: 2,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run-01.caf");
    let mut bytes = caf::header(STEREO);
    bytes.extend(caf::encode(&PcmBlock {
        host_ns: 0,
        samples: vec![0.3; 2 * 200],
    }));
    bytes.extend(vec![0u8; 6 * 100]); // zeros after a power cut
    bytes.extend([7u8; 4]); // a torn frame
    std::fs::write(&path, &bytes).unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let len = bytes.len() as u64;
    let scan = caf::scan(&mut &file, len).unwrap();
    assert_eq!((scan.frames, scan.sized), (300, false));
    let keep = caf::keep_frames(&mut &file, &scan, 150).unwrap();
    assert_eq!(keep, 200);

    let cut_audio = caf::finalize(&OsIo, &file, &path, &scan, 150).unwrap_err();
    assert_eq!(cut_audio.kind(), std::io::ErrorKind::InvalidData);
    let pad = caf::finalize(&OsIo, &file, &path, &scan, 400).unwrap_err();
    assert_eq!(pad.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "a refusal writes nothing"
    );

    caf::finalize(&OsIo, &file, &path, &scan, keep).unwrap();
    caf::finalize(&OsIo, &file, &path, &scan, keep).unwrap();
    let sized_len = caf::HEADER_LEN + 6 * 200;
    assert_eq!(file.metadata().unwrap().len(), sized_len);
    let rescanned = caf::scan(&mut &file, sized_len).unwrap();
    assert_eq!((rescanned.frames, rescanned.sized), (200, true));
    assert_eq!(decode_audio_frames(&path), 200);

    file.set_len(sized_len - 6).unwrap(); // a sized file that lost its last frame
    let short = caf::scan(&mut &file, sized_len - 6).unwrap();
    assert_eq!(
        (short.frames, short.sized),
        (199, true),
        "the size never claims frames the file doesn't hold"
    );
}

/// A crash or a write error inside `finalize`'s 8-byte size write leaves each byte either the new size's or the old
/// −1's. The scan never reads fewer frames than the file holds, and a re-run finishes.
#[test]
fn a_size_torn_at_any_byte_scans_every_frame_and_finalize_reruns() {
    let mut bytes = caf::header(MONO);
    bytes.extend(caf::encode(&PcmBlock {
        host_ns: 0,
        samples: vec![0.4; 300],
    }));
    let size = (4 + 3 * 300i64).to_be_bytes();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run-01.caf");
    let len = bytes.len() as u64;
    for k in 1..8 {
        for landed in [0..k, k..8] {
            let mut torn = bytes.clone();
            for i in landed.clone() {
                torn[56 + i] = size[i];
            }
            std::fs::write(&path, &torn).unwrap();
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let scan = caf::scan(&mut &file, len).unwrap();
            assert_eq!(scan.frames, 300, "size bytes {landed:?} landed");
            caf::finalize(&OsIo, &file, &path, &scan, scan.frames).unwrap();
            let done = caf::scan(&mut &file, len).unwrap();
            assert_eq!((done.frames, done.sized), (300, true), "{landed:?}");
        }
    }
    assert_eq!(decode_audio_frames(&path), 300);
}

/// A sound block must hold whole frames. A partial one would shift every later frame's channels
/// (left becomes right), so it is refused whole and nothing is written.
#[test]
fn a_block_of_whole_frames_is_encoded_and_a_partial_one_refused() {
    let stereo = AudioFormat {
        rate: 48_000,
        channels: 2,
    };
    let block = |n: usize| PcmBlock {
        host_ns: 0,
        samples: vec![0.25; n],
    };
    let mut out = Vec::new();
    assert_eq!(caf::encode_frames(stereo, &block(6), &mut out), Ok(3));
    assert_eq!(out.len(), 18);
    let partial = caf::encode_frames(stereo, &block(5), &mut out).unwrap_err();
    assert_eq!(out.len(), 18, "nothing of a partial block");
    assert_eq!(
        partial.coded().code,
        "capture.note.soundBlock",
        "a milder note"
    );
}
