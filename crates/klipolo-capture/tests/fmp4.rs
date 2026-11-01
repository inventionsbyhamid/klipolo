#![cfg(feature = "synthetic")]
mod common;
use common::decode::decode_all;
use klipolo_capture::fmp4::{Fmp4Writer, FragIndex, SampleIndex, finalize, read};
use klipolo_capture::io::OsIo;
use klipolo_capture::synthetic::FixtureVideo;
use klipolo_platform::file::PositionedIo;
use std::io::{Cursor, ErrorKind, Write};

fn fixture(name: &str) -> FixtureVideo {
    FixtureVideo::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
}

/// Writes `count` frames of the fixture at 30 fps; returns the bytes and the writer.
fn written(f: &FixtureVideo, count: u64) -> (Vec<u8>, Fmp4Writer) {
    let mut w = Fmp4Writer::new(&f.info);
    let mut bytes = w.head();
    for (i, p) in f.packets(count, 0, 30).iter().enumerate() {
        if let Some(frag) = w.push(i as i64 * 1_000_000 / 30, p) {
            bytes.extend(frag.bytes);
        }
    }
    if let Some(frag) = w.flush() {
        bytes.extend(frag.bytes);
    }
    (bytes, w)
}

#[test]
fn a_written_file_reads_back_sample_for_sample() {
    let f = fixture("gop-h264.mp4");
    let (bytes, _) = written(&f, 60);
    let (track, samples) = read::samples(&bytes).unwrap();
    assert_eq!(track.info, f.info);
    assert_eq!(samples.len(), 60);
    assert!(
        samples
            .iter()
            .zip(&f.frames)
            .all(|(s, (key, data))| s.2 == *key && &s.3 == data)
    );
}

#[test]
fn fragments_close_every_half_second_without_waiting_for_a_keyframe() {
    let f = fixture("gop-h264.mp4");
    let (bytes, w) = written(&f, 60);
    assert_eq!(
        w.index()
            .fragments
            .iter()
            .map(|g| g.samples.len())
            .collect::<Vec<_>>(),
        [15, 15, 15, 15]
    );
    let walk = read::walk(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
    assert_eq!(
        (walk.index.fragments.len(), walk.valid_len),
        (4, bytes.len() as u64)
    );
}

#[test]
fn every_truncation_keeps_the_whole_fragments_before_it() {
    let f = fixture("gop-h264.mp4");
    let (bytes, w) = written(&f, 60);
    let ends: Vec<u64> = w
        .index()
        .fragments
        .iter()
        .map(|g| g.data_at + g.samples.iter().map(|s| s.size as u64).sum::<u64>())
        .collect();
    for cut in (0..bytes.len() as u64).step_by(97) {
        let walk = read::walk(&mut Cursor::new(&bytes[..cut as usize]), cut).unwrap();
        let expected = ends.iter().filter(|e| **e <= cut).count();
        assert_eq!(walk.index.fragments.len(), expected, "cut at {cut}");
        assert!(walk.valid_len <= cut);
    }
}

#[test]
fn a_zero_tail_after_a_power_cut_stops_the_walk_at_the_damage() {
    let f = fixture("gop-h264.mp4");
    let (mut bytes, _) = written(&f, 30);
    let good = bytes.len() as u64;
    bytes.extend(vec![0u8; 64 * 1024]);
    let walk = read::walk(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
    assert_eq!((walk.valid_len, walk.damaged_at), (good, Some(good)));
}

#[test]
fn the_soft_remux_gives_a_regular_mp4_that_ffmpeg_reads_frame_for_frame() {
    for name in ["gop-h264.mp4", "gop-hevc.mp4"] {
        let f = fixture(name);
        let (bytes, _) = written(&f, 60);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run-01.mp4");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let walk = read::walk(&mut &file, bytes.len() as u64).unwrap();
        let plan = finalize::plan(&walk).unwrap();
        finalize::write_tail(&OsIo, &file, &path, &plan).unwrap();
        // A crash here leaves a fragmented file with a trailing `free` box: still readable as it was.
        assert_eq!(decode_all(&path).0, 60, "{name} before the flips");
        finalize::flip(&OsIo, &file, &path, &plan).unwrap();
        let (frames, last) = decode_all(&path);
        assert_eq!((frames, last), (60, 59 * 1_000_000 / 30), "{name}");
        let after = std::fs::read(&path).unwrap();
        assert_eq!(&after[after.len() - plan.tail.len() + 4..][..4], b"moov");
        assert!(
            read::walk(&mut Cursor::new(&after), after.len() as u64)
                .unwrap()
                .remuxed
        );
    }
}

#[test]
fn an_hour_of_60_fps_keeps_a_small_index() {
    let f = fixture("gop-h264.mp4");
    let mut w = Fmp4Writer::new(&f.info);
    let p = &f.packets(1, 0, 60)[0];
    for i in 0..216_000i64 {
        w.push(i * 1_000_000 / 60, p);
    }
    let bytes: usize = w
        .index()
        .fragments
        .iter()
        .map(|g| size_of::<FragIndex>() + g.samples.len() * size_of::<SampleIndex>())
        .sum();
    assert!(bytes <= 3 * 1024 * 1024, "{bytes}");
}

/// Stop remuxes from the writer's own index, without a walk: a walk of the same bytes must plan the same tail.
#[test]
fn the_writers_own_remux_plan_is_the_walks() {
    for name in ["gop-h264.mp4", "gop-hevc.mp4"] {
        let f = fixture(name);
        let (bytes, w) = written(&f, 50);
        let walk = read::walk(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
        assert_eq!(&walk.index, w.index(), "{name}");
        assert_eq!(w.remux_plan(), finalize::plan(&walk), "{name}");
    }
}

/// A kill mid-fragment, then a remux cut short by a crash part-way through the flips: the torn bytes are never cut by
/// the remux, and a re-run of the same steps finishes a file that decodes every whole fragment.
#[test]
fn a_torn_take_keeps_its_whole_fragments_and_a_remux_cut_short_finishes_on_a_rerun() {
    let f = fixture("gop-h264.mp4");
    let (bytes, w) = written(&f, 60);
    let third = &w.index().fragments[2];
    let cut = third.data_at + 100;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run-01.mp4");
    std::fs::write(&path, &bytes[..cut as usize]).unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let walk = read::walk(&mut &file, cut).unwrap();
    assert_eq!(
        (walk.index.fragments.len(), walk.damaged_at),
        (2, Some(third.moof_at))
    );
    let plan = finalize::plan(&walk).unwrap();
    // Recovery saves the torn bytes before it truncates; the remux refuses to cut them itself.
    let refused = finalize::write_tail(&OsIo, &file, &path, &plan).unwrap_err();
    assert_eq!(
        (refused.kind(), file.metadata().unwrap().len()),
        (ErrorKind::InvalidData, cut)
    );
    file.set_len(third.moof_at).unwrap();
    finalize::write_tail(&OsIo, &file, &path, &plan).unwrap();
    // The crash: only the first `moof` became `free`.
    file.write_all_at(b"free", plan.flips[1]).unwrap();
    let len = || file.metadata().unwrap().len();
    let again = read::walk(&mut &file, len()).unwrap();
    assert!(!again.remuxed && again.damaged_at.is_none());
    assert_eq!(finalize::plan(&again).as_ref(), Some(&plan));
    finalize::write_tail(&OsIo, &file, &path, &plan).unwrap();
    finalize::flip(&OsIo, &file, &path, &plan).unwrap();
    finalize::flip(&OsIo, &file, &path, &plan).unwrap();
    assert_eq!(decode_all(&path), (30, 29 * 1_000_000 / 30));
    let done = read::walk(&mut &file, len()).unwrap();
    assert!(done.remuxed && done.damaged_at.is_none());
    assert_eq!(
        (done.index, len()),
        (walk.index, plan.tail_at + plan.tail.len() as u64)
    );
}

/// A remux tail cut short (a full disk, a kill or a power cut inside `write_tail`) is the remux's own: the walk ends
/// the media before it without calling it damage, and a re-run of the remux replaces it.
#[test]
fn a_remux_tail_cut_short_anywhere_finishes_on_a_rerun() {
    let f = fixture("gop-h264.mp4");
    let (bytes, _) = written(&f, 60);
    let walk = read::walk(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
    let plan = finalize::plan(&walk).unwrap();
    let mut typed_free = plan.tail.clone();
    typed_free[4..8].copy_from_slice(b"free");
    let dir = tempfile::tempdir().unwrap();
    for n in [8, 12, 16, 200, plan.tail.len() - 1] {
        let path = dir.path().join(format!("run-{n}.mp4"));
        let mut torn = bytes.clone();
        torn.extend_from_slice(&typed_free[..n]);
        std::fs::write(&path, &torn).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let again = read::walk(&mut &file, torn.len() as u64).unwrap();
        assert_eq!(
            (again.valid_len, again.damaged_at, again.remuxed),
            (bytes.len() as u64, None, false),
            "cut after {n} bytes of the tail"
        );
        assert_eq!(finalize::plan(&again).as_ref(), Some(&plan), "{n}");
        finalize::write_tail(&OsIo, &file, &path, &plan).unwrap();
        finalize::flip(&OsIo, &file, &path, &plan).unwrap();
        assert_eq!(decode_all(&path).0, 60, "{n}");
    }
}

/// A packet stamped ahead of real time can't hold a fragment open: it closes at 64 samples, and the fragments' times
/// stay contiguous and never run backwards.
#[test]
fn a_packet_stamped_ahead_cant_hold_a_fragment_open() {
    let f = fixture("gop-h264.mp4");
    let mut w = Fmp4Writer::new(&f.info);
    let mut fragments = vec![];
    for (i, p) in f.packets(300, 0, 60).iter().enumerate() {
        let pts = if i == 10 {
            3_000_000
        } else {
            i as i64 * 1_000_000 / 60
        };
        fragments.extend(w.push(pts, p));
    }
    fragments.extend(w.flush());
    let sizes: Vec<usize> = w
        .index()
        .fragments
        .iter()
        .map(|g| g.samples.len())
        .collect();
    assert!(sizes.iter().all(|&n| n <= 64), "{sizes:?}");
    assert_eq!(sizes.iter().sum::<usize>(), 300);
    assert!(fragments.iter().all(|g| g.end_us >= g.start_us));
    assert!(fragments.windows(2).all(|g| g[0].end_us == g[1].start_us));
}

/// The cap follows the run's frame rate (about 0.5 s of frames), so at 30 fps a packet stamped
/// ahead holds a fragment open for at most 16 frames, not 64 (2.1 s that a kill would lose). Each fragment's media time
/// is its own span, so the spans add up to the run's.
#[test]
fn the_fragment_cap_follows_the_frame_rate() {
    let f = fixture("gop-h264.mp4");
    let mut w = Fmp4Writer::at_rate(&f.info, 30);
    let mut fragments = vec![];
    for (i, p) in f.packets(90, 0, 30).iter().enumerate() {
        let pts = if i == 10 {
            3_000_000
        } else {
            i as i64 * 1_000_000 / 30
        };
        fragments.extend(w.push(pts, p));
    }
    fragments.extend(w.flush());
    let sizes: Vec<usize> = w
        .index()
        .fragments
        .iter()
        .map(|g| g.samples.len())
        .collect();
    assert!(sizes.iter().all(|&n| n <= 16), "{sizes:?}");
    assert_eq!(sizes.iter().sum::<usize>(), 90);
    let spans: i64 = fragments.iter().map(|g| g.media_us()).sum();
    assert_eq!(
        spans,
        fragments.last().unwrap().end_us - fragments[0].start_us
    );
}

/// A flipped `moof` whose `mfhd` is garbled reads as a plain `free`: the media ends at its `mdat`, never skipping it
/// and leaving a hole that later frames refer to.
#[test]
fn a_garbled_flipped_moof_ends_the_media_there() {
    let f = fixture("gop-h264.mp4");
    let (mut bytes, w) = written(&f, 60);
    let second = &w.index().fragments[1];
    let at = second.moof_at as usize;
    bytes[at + 4..at + 8].copy_from_slice(b"free");
    bytes[at + 12..at + 16].copy_from_slice(b"\0\0\0\0");
    let walk = read::walk(&mut Cursor::new(&bytes), bytes.len() as u64).unwrap();
    assert_eq!(
        (walk.index.fragments.len(), walk.damaged_at),
        (1, Some(second.data_at - 8))
    );
}
