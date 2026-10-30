mod support;
use std::sync::{Arc, Mutex};

use klipolo_engine::media::key::{CacheKind, stem};
use klipolo_engine::media::sprite::{PER_CHUNK, Sprite, TILE_H, chunks_for, tile_size};
use klipolo_engine::media::store::{Hold, ThumbStore, Want};
use klipolo_engine::media::thumbs::{Fill, chunk_path, strip};
use klipolo_platform::durable::Os;
use klipolo_platform::fake::{Fault, RecordingDurable, Step};
use support::*;

#[test]
fn tiles_follow_the_picture_shape() {
    assert_eq!(tile_size(1920, 1080), (128, TILE_H));
    assert_eq!(tile_size(1080, 1920), (40, TILE_H));
    assert_eq!(tile_size(1440, 1080), (96, TILE_H));
    assert_eq!(chunks_for(secs(130.0)), 3);
    assert_eq!(chunks_for(secs(64.0)), 1);
}

#[test]
fn holes_show_the_nearest_earlier_tile() {
    let mut s = Sprite::new(0, (40, 72));
    let y = vec![200u8; 40 * 72];
    let uv = vec![128u8; 20 * 36];
    s.put(3, [&y, &uv, &uv], [40, 20, 20]);
    s.fill_holes();
    // Slots 0–2 take slot 3's tile (nothing earlier), 4–63 hold it too.
    assert!((0..PER_CHUNK).all(|i| s.luma_at(i, 0, 0) == 200));
}

#[test]
fn stems_change_when_the_file_changes() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.mp4");
    std::fs::write(&f, b"one").unwrap();
    let path = f.to_string_lossy().to_string();
    let first = stem(&path, &CacheKind::Thumbs).unwrap();
    assert_ne!(
        Some(first.clone()),
        stem(&path, &CacheKind::Pcm),
        "kinds don't share names"
    );
    std::fs::write(&f, b"three").unwrap();
    assert_ne!(
        Some(first),
        stem(&path, &CacheKind::Thumbs),
        "an edited file gets a new name"
    );
}

fn wait_until(mut ok: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !ok() {
        assert!(std::time::Instant::now() < deadline, "timed out");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn a_scan_writes_one_chunk_per_64_seconds_and_redoes_only_a_missing_one() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("scan-130s-720p.mp4").to_string_lossy().to_string();
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    store.set_sink(Arc::new(move |e| seen.lock().unwrap().push(e)));
    let want = Want {
        path: video.clone(),
        duration_us: secs(130.0),
        at_us: 0,
    };
    store.want(vec![want.clone()]);
    wait_until(|| strip(root.path(), &video).is_some_and(|s| s.chunks.len() == 3));
    let s = strip(root.path(), &video).unwrap();
    assert_eq!((s.tile_w, s.tile_h, s.per_chunk), (128, 72, 64));
    assert!(!events.lock().unwrap().is_empty());
    let key = stem(&video, &CacheKind::Thumbs).unwrap();
    let first = chunk_path(root.path(), &key, 0, Fill::Keys);
    let kept = std::fs::metadata(&first).unwrap().modified().unwrap();
    std::fs::remove_file(chunk_path(root.path(), &key, 1, Fill::Keys)).unwrap();
    store.want(vec![want]);
    wait_until(|| strip(root.path(), &video).is_some_and(|s| s.chunks.len() == 3));
    assert_eq!(
        std::fs::metadata(&first).unwrap().modified().unwrap(),
        kept,
        "finished chunks are kept"
    );
}

#[test]
fn a_full_disk_stops_thumbnails_without_an_error() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("scan-130s-720p.mp4").to_string_lossy().to_string();
    let store = ThumbStore::new(
        root.path().to_path_buf(),
        Arc::new(RecordingDurable::failing(Step::Write, Fault::Full)),
    );
    store.want(vec![Want {
        path: video.clone(),
        duration_us: secs(130.0),
        at_us: 0,
    }]);
    wait_until(|| store.disk_full());
    assert!(strip(root.path(), &video).is_none_or(|s| s.chunks.is_empty()));
}

#[test]
fn a_held_scan_waits_then_finishes_where_it_was() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("scan-130s-720p.mp4").to_string_lossy().to_string();
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    // Transcription hears whether scans that need work wait or run, and goes on beside them at half its threads
    //.
    let waiting = Arc::new(std::sync::Mutex::new(Vec::new()));
    let heard = waiting.clone();
    store.on_waiting(move |on| heard.lock().unwrap().push(on));
    store.pause(Hold::Playback);
    store.pause(Hold::Copy);
    store.want(vec![Want {
        path: video.clone(),
        duration_us: secs(130.0),
        at_us: 0,
    }]);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!store.scanning(), "nothing scans while the video plays");
    assert!(strip(root.path(), &video).is_none_or(|s| s.chunks.is_empty()));
    store.resume(Hold::Playback);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!store.scanning(), "nor while an editing copy is made");
    assert_eq!(*waiting.lock().unwrap(), [false, true], "a scan waits");
    store.resume(Hold::Copy);
    wait_until(|| strip(root.path(), &video).is_some_and(|s| s.chunks.len() == 3));
    wait_until(|| !store.scanning());
    wait_until(|| *waiting.lock().unwrap() == [false, true, false]);
    // A file already done never says a scan waits, even while scans are held.
    store.pause(Hold::Copy);
    store.want(vec![Want {
        path: video.clone(),
        duration_us: secs(130.0),
        at_us: 0,
    }]);
    wait_until(|| !store.pending());
    assert_eq!(*waiting.lock().unwrap(), [false, true, false]);
    store.resume(Hold::Copy);
}

/// Sound first: the sound cache's pass reads sound only; the scan after it cuts the thumbnails.
#[test]
fn the_sound_build_reads_only_sound_and_the_scan_after_it_cuts_the_thumbnails() {
    let root = tempfile::tempdir().unwrap();
    let video = fixture("scan-130s-720p.mp4").to_string_lossy().to_string();
    let thumbs = ThumbStore::new(root.path().join("thumbs"), Arc::new(Os));
    let pcm = klipolo_engine::audio::pcm::PcmStore::new(root.path().join("pcm"));
    pcm.set_thumbs(thumbs.clone());
    pcm.build(&klipolo_engine::audio::pcm::PcmKey {
        path: video.clone(),
    })
    .unwrap();
    assert!(strip(&root.path().join("thumbs"), &video).is_none());
    let peaks = pcm.peaks_path(&video).unwrap();
    let info = klipolo_engine::audio::pcm::peaks::info(&peaks).unwrap();
    let bytes = klipolo_engine::audio::pcm::peaks::read(&peaks, 1, 0, 100).unwrap();
    assert_eq!(bytes.len(), 100 * usize::from(info.channels) * 4);
    thumbs.want(vec![Want {
        path: video.clone(),
        duration_us: secs(130.0),
        at_us: 0,
    }]);
    wait_until(|| strip(&root.path().join("thumbs"), &video).is_some_and(|s| s.chunks.len() == 3));
}

/// Chunks without a keyframe of their own (a GOP longer than a chunk, a file ending inside a GOP) show the
/// keyframe before them, are written, and the file is complete: a later opening scans nothing.
#[test]
fn a_long_gop_file_writes_every_chunk_and_is_not_scanned_again() {
    let root = tempfile::tempdir().unwrap();
    // Keyframes at 0 and 80 s over 140 s: chunk 1 starts inside a GOP, chunk 2 has no keyframe.
    let video = fixture("gop80-140s-180p.mp4").to_string_lossy().to_string();
    let want = Want {
        path: video.clone(),
        duration_us: secs(140.0),
        at_us: 0,
    };
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    store.want(vec![want.clone()]);
    wait_until(|| !store.pending());
    assert_eq!(strip(root.path(), &video).unwrap().chunks.len(), 3);
    let key = stem(&video, &CacheKind::Thumbs).unwrap();
    let last = chunk_path(root.path(), &key, 2, Fill::Keys);
    let written = std::fs::metadata(&last).unwrap().modified().unwrap();
    // Opened again (a new store, as after a relaunch): nothing to scan, nothing announced.
    let again = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    again.set_sink(Arc::new(move |e| seen.lock().unwrap().push(e)));
    again.want(vec![want]);
    wait_until(|| !again.pending());
    assert!(
        events.lock().unwrap().is_empty(),
        "{:?}",
        events.lock().unwrap()
    );
    assert_eq!(
        std::fs::metadata(&last).unwrap().modified().unwrap(),
        written
    );
}

/// An editing copy carries its keyframe index beside it, read from its sample table (no packet read),
/// the same keyframes a demux finds. It names the copy by its size and file number, so the least-recently-used touch
/// (a new modification time) keeps it, and a copy rewritten in place (another size) doesn't find it.
#[test]
fn a_copys_index_beside_it_is_its_keyframes_and_survives_a_touch() {
    use klipolo_engine::media::keyframes::{beside, from_sample_table, load, write_beside};
    let _ = ffmpeg::init();
    let dir = tempfile::tempdir().unwrap();
    // Frames in order, as an editing copy's are (no B-frames; this intra-only clip has none either).
    let copy = dir.path().join("0123456789abcdef.mov");
    std::fs::copy(fixture("intra-6s-mjpeg.mov"), &copy).unwrap();
    let keys = from_sample_table(&copy).expect("a MOV's sample table lists every frame");
    // The keyframes a demux of every packet finds.
    let mut input = ffmpeg::format::input(&copy).unwrap();
    let video = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .unwrap()
        .index();
    let demuxed: Vec<i64> = input
        .packets()
        .filter(|(s, p)| s.index() == video && p.is_key())
        .map(|(_, p)| p.pts().unwrap())
        .collect();
    assert_eq!(keys.pts, demuxed);
    assert!(keys.pts.len() > 1);
    write_beside(&copy, &keys).unwrap();
    assert_eq!(load(&copy), Some(keys.clone()));
    let file = std::fs::File::options().write(true).open(&copy).unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
        .unwrap();
    assert_eq!(load(&copy), Some(keys), "a touch keeps it");
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&copy)
        .unwrap()
        .write_all(b"more")
        .unwrap();
    assert_eq!(load(&copy), None, "another size: not this file's index");
    assert!(beside(&copy).exists());
}

#[test]
fn wants_start_under_the_playhead() {
    let p = project();
    let wants = klipolo_engine::media::wants(&p, secs(25.0));
    // The playhead is in "b" (broll): its file first, then the recording's screen and camera.
    let paths: Vec<_> = wants.iter().map(|w| w.path.as_str()).collect();
    assert_eq!(paths, ["/media/broll", "/media/screen", "/media/camera"]);
}

/// A keyframe index left by an older build (another format) is written again by the next scan, once: a file wanted
/// again afterwards is complete and nothing is rewritten. Before, it counted as stored forever and
/// every preview decoder read the whole file to build its own.
#[test]
fn an_index_in_an_older_format_is_written_again_once() {
    use klipolo_engine::media::keyframes::KEYS_FILE;
    let root = tempfile::tempdir().unwrap();
    let path = fixture("scan-130s-720p.mp4").to_string_lossy().to_string();
    let want = || {
        vec![Want {
            path: path.clone(),
            duration_us: secs(130.0),
            at_us: 0,
        }]
    };
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    store.want(want());
    let keys = root
        .path()
        .join(stem(&path, &CacheKind::Thumbs).unwrap())
        .join(KEYS_FILE);
    let settled = |store: &ThumbStore| !store.pending() && !store.scanning();
    wait_until(|| keys.exists() && settled(&store));
    let magic = |bytes: &[u8]| bytes[..8].to_vec();
    assert_eq!(magic(&std::fs::read(&keys).unwrap()), b"KLPOKEY3");
    // The same keyframes in the previous format.
    let mut old = std::fs::read(&keys).unwrap();
    old[..8].copy_from_slice(b"XXXXKEY3");
    std::fs::write(&keys, &old).unwrap();
    store.want(want());
    wait_until(|| std::fs::read(&keys).is_ok_and(|b| magic(&b) == b"KLPOKEY3") && settled(&store));
    let rewritten = std::fs::metadata(&keys).unwrap().modified().unwrap();
    // Wanted again: complete, so it isn't scanned or written a second time.
    store.want(want());
    wait_until(|| settled(&store));
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(
        std::fs::metadata(&keys).unwrap().modified().unwrap(),
        rewritten
    );
}

/// The pass that cuts thumbnails stores the keyframe index, so opening the file builds none.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn a_scanned_file_opens_with_its_stored_keyframe_index() {
    use klipolo_engine::media::keyframes::{KEYS_FILE, set_root};
    use klipolo_engine::vendor::cap::rendering::decoder::{DecoderTuning, spawn_decoder};
    use klipolo_engine::vendor::cap::video_decode::KeyframeIndex;
    let root = tempfile::tempdir().unwrap();
    let video = fixture("scan-130s-720p.mp4");
    let path = video.to_string_lossy().to_string();
    let store = ThumbStore::new(root.path().to_path_buf(), Arc::new(Os));
    store.want(vec![Want {
        path: path.clone(),
        duration_us: secs(130.0),
        at_us: 0,
    }]);
    let key = stem(&path, &CacheKind::Thumbs).unwrap();
    wait_until(|| root.path().join(&key).join(KEYS_FILE).exists());
    set_root(Some(root.path().to_path_buf()));
    let builds = KeyframeIndex::builds();
    // A preview decoder opens and seeks into the second chunk.
    let decoder = spawn_decoder(
        "klipolo-test",
        video.clone(),
        30.0,
        0.0,
        false,
        DecoderTuning::default(),
    )
    .await
    .unwrap();
    decoder.get_frame(65.5).await.expect("a frame at 65.5 s");
    assert_eq!(
        KeyframeIndex::builds(),
        builds,
        "the stored index is read, none is built"
    );
    // It is the index a demux would build: every seek lands on the same keyframe.
    let started = std::time::Instant::now();
    let stored = KeyframeIndex::load_or_build(&video).unwrap();
    let loaded = started.elapsed();
    let started = std::time::Instant::now();
    let built = KeyframeIndex::build(&video).unwrap();
    let demuxed = started.elapsed();
    assert_eq!(stored.keyframes(), built.keyframes());
    for frame in [0, 29, 30, 31, 1965, 1920, 3899] {
        assert_eq!(
            stored.nearest_keyframe_before(frame),
            built.nearest_keyframe_before(frame)
        );
    }
    assert_eq!(
        (stored.fps(), stored.duration_secs()),
        (built.fps(), built.duration_secs())
    );
    eprintln!("keyframe index: stored {loaded:?}, built by demuxing {demuxed:?}");

    // An index an older build stored (another format) is written again once by the next scan; after that, decoder
    // starts read it and build none.
    let keys = root.path().join(&key).join(KEYS_FILE);
    let mut old = std::fs::read(&keys).unwrap();
    old[..8].copy_from_slice(b"XXXXKEY3");
    std::fs::write(&keys, &old).unwrap();
    store.want(vec![Want {
        path: path.clone(),
        duration_us: secs(130.0),
        at_us: 0,
    }]);
    wait_until(|| std::fs::read(&keys).is_ok_and(|b| b.starts_with(b"KLPOKEY3")));
    wait_until(|| !store.pending() && !store.scanning());
    let builds = KeyframeIndex::builds();
    for _ in 0..2 {
        let decoder = spawn_decoder(
            "klipolo-test",
            video.clone(),
            30.0,
            0.0,
            false,
            DecoderTuning::default(),
        )
        .await
        .unwrap();
        decoder.get_frame(100.5).await.expect("a frame at 100.5 s");
    }
    assert_eq!(
        KeyframeIndex::builds(),
        builds,
        "no decoder start builds an index"
    );
}
