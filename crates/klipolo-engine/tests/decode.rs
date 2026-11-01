mod support;

#[test]
fn ffmpeg_is_lgpl_9() {
    let info = klipolo_engine::ffmpeg_info();
    assert!(info.version.starts_with("9.0.2"), "{info:?}");
    assert!(info.license.contains("LGPL"), "{info:?}");
}

use klipolo_engine::vendor::cap::rendering::decoder::{DecoderTuning, DecoderType, spawn_decoder};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn decodes_a_frame_from_the_fixture() {
    let path = support::fixture("bars-30fps-0.5gop.mp4");
    // AVAssetReader (preview) and FFmpeg + VideoToolbox (`force_ffmpeg`: fallback and export retry).
    for force_ffmpeg in [false, true] {
        let handle = spawn_decoder(
            "klipolo-test",
            path.clone(),
            30.0,
            0.0,
            force_ffmpeg,
            DecoderTuning::default(),
        )
        .await
        .unwrap();
        let frame = handle.get_frame(1.0).await.expect("a frame at 1 s");
        assert_eq!(
            (frame.width(), frame.height()),
            (1920, 1080),
            "force_ffmpeg={force_ffmpeg}"
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            handle.decoder_type() == DecoderType::AVAssetReader,
            !force_ffmpeg,
            "{:?}",
            handle.decoder_type()
        );
        // Elsewhere FFmpeg decodes in software until W and L add D3D12VA and VA-API.
        #[cfg(not(target_os = "macos"))]
        assert_eq!(handle.decoder_type(), DecoderType::FFmpegSoftware);
    }
}

use std::time::{Duration, Instant};

use klipolo_engine::plan::{SourceRef, VideoRole};
use klipolo_engine::render::pool::{DecodedSet, DecoderPool, probe_fps};
use support::plan_with;

/// The tests that count software fallbacks (one count for the whole process) take turns.
#[cfg(target_os = "macos")]
static FALLBACKS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn source(path: &str, fps: f64) -> SourceRef {
    SourceRef {
        asset_id: "a".into(),
        path: path.into(),
        original: path.into(),
        time_us: 1_000_000,
        fps,
        rotation: None,
        width: 1920,
        height: 1080,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_media_is_reported_and_skipped() {
    let mut pool = DecoderPool::new(DecoderTuning::default(), false);
    let gone = source("/nope/intro.mp4", 30.0);
    let started = Instant::now();
    let mut set = DecodedSet::default();
    pool.decode(&plan_with(&gone), &mut set).await;
    assert!(set.get(0).is_none());
    // Said with the copy row's code (`api::MEDIA_MISSING`), never as a file that couldn't be read.
    assert_eq!(set.missing, [std::sync::Arc::from("/nope/intro.mp4")]);
    assert!(set.warnings.is_empty(), "{:?}", set.warnings);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
}

/// The preview reads `original` from its editing copy at `copy`.
fn from_copy(copy: &str, original: &str) -> SourceRef {
    SourceRef {
        path: copy.into(),
        original: original.into(),
        rotation: Some(0),
        ..source(copy, 30.0)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_copy_of_a_missing_source_names_the_source() {
    let mut pool = DecoderPool::new(DecoderTuning::default(), false);
    let mut set = DecodedSet::default();
    let src = from_copy("/nope/cache/1f2e.mp4", "/nope/intro.mp4");
    pool.decode(&plan_with(&src), &mut set).await;
    assert!(set.get(0).is_none());
    assert_eq!(set.missing, [std::sync::Arc::from("/nope/intro.mp4")]);
    assert!(set.warnings.is_empty(), "{:?}", set.warnings);
}

/// A file that is there but isn't video still "couldn't be read"; only an absent one is missing.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_is_there_but_unreadable_is_not_called_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("intro.mp4");
    std::fs::write(&path, b"not a video at all").unwrap();
    let mut pool = DecoderPool::new(DecoderTuning::default(), false);
    let mut set = DecodedSet::default();
    pool.decode(
        &plan_with(&source(&path.display().to_string(), 30.0)),
        &mut set,
    )
    .await;
    assert!(set.get(0).is_none());
    assert_eq!(set.warnings, ["This file couldn't be read: intro.mp4"]);
    assert!(set.missing.is_empty(), "{:?}", set.missing);
}

/// A source moved away while the preview reads it is said missing (the copy row's code), without the reader's
/// software fallback trying the file that isn't there; once it is back it is read again.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_source_moved_away_while_open_is_missing_with_no_fallback() {
    use klipolo_engine::vendor::cap::rendering::decoder::software_fallbacks;
    let _counting = FALLBACKS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("intro.mp4");
    let away = dir.path().join("intro moved.mp4");
    std::fs::hard_link(support::fixture("bars-30fps-0.5gop.mp4"), &path).unwrap();
    let mut src = source(&path.display().to_string(), 30.0);
    // Readers and cached frames go after 250 ms idle (`DecoderTuning::default`), so the next frame opens the file.
    let mut pool = DecoderPool::new(DecoderTuning::default(), false);
    let mut set = DecodedSet::default();
    pool.decode(&plan_with(&src), &mut set).await;
    assert!(set.get(0).is_some(), "{:?}", set.warnings);
    let fallbacks = software_fallbacks();

    std::fs::rename(&path, &away).unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    src.time_us = 3_000_000;
    pool.decode(&plan_with(&src), &mut set).await;
    assert!(set.get(0).is_none());
    assert_eq!(
        set.missing,
        [std::sync::Arc::from(path.display().to_string())]
    );
    assert!(set.warnings.is_empty(), "{:?}", set.warnings);
    // Asked again while it is gone: still missing, and the file isn't opened again yet.
    pool.decode(&plan_with(&src), &mut set).await;
    assert_eq!(set.missing.len(), 1);
    assert_eq!(
        software_fallbacks(),
        fallbacks,
        "no software fallback on a file that isn't there"
    );

    // Back where it was: read again once the pool tries it again (5 s after it failed).
    std::fs::rename(&away, &path).unwrap();
    tokio::time::sleep(Duration::from_millis(5_100)).await;
    pool.decode(&plan_with(&src), &mut set).await;
    assert!(set.get(0).is_some(), "{:?} {:?}", set.warnings, set.missing);
    assert!(set.missing.is_empty());
}

/// When an open source's reader stops returning frames, whether its file is still there is asked off the engine's task,
/// within the preview's limit, and only then is the software fallback started; the reader never asks itself (a stat
/// there ran on the engine's task and hung it when a volume stopped answering). While the disk is silent
/// (`slow_stats`), the frame is held no longer than that limit, a redraw and another source's frame aren't held at all,
/// and nothing falls back. Once the disk answers that the file is there, the redraw signal fires and the still is read
/// through the fallback.
#[cfg(all(target_os = "macos", feature = "trial"))]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_disk_that_stops_answering_under_an_open_source_never_holds_the_preview() {
    use klipolo_engine::render::pool::slow_stats;
    use klipolo_engine::vendor::cap::rendering::decoder::software_fallbacks;
    use std::sync::Arc;
    let _counting = FALLBACKS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (path, other) = (dir.path().join("intro.mp4"), dir.path().join("other.mp4"));
    std::fs::hard_link(support::fixture("bars-30fps-0.5gop.mp4"), &path).unwrap();
    std::fs::hard_link(support::fixture("bars-30fps-0.5gop.mp4"), &other).unwrap();
    let file: Arc<str> = path.display().to_string().into();
    let intro = source(&file, 30.0);
    let other = SourceRef {
        asset_id: "b".into(),
        ..source(&other.display().to_string(), 30.0)
    };
    let redraw = Arc::new(tokio::sync::Notify::new());
    let mut pool = DecoderPool::new(DecoderTuning::default(), false);
    pool.set_redraw(redraw.clone());
    let mut set = DecodedSet::default();
    pool.decode(&plan_with(&intro), &mut set).await;
    assert!(set.get(0).is_some(), "{:?}", set.warnings);

    // The reader lets its file go after 250 ms idle (`DecoderTuning::default`). Then the file becomes one the reader
    // can't open but software can (the same picture in Matroska), and its disk stops answering.
    tokio::time::sleep(Duration::from_millis(600)).await;
    std::fs::remove_file(&path).unwrap();
    std::fs::hard_link(support::fixture("bars-no-length.mkv"), &path).unwrap();
    slow_stats(file.clone(), Some(Duration::from_secs(3)));
    let fallbacks = software_fallbacks();

    let started = Instant::now();
    pool.decode(&plan_with(&intro), &mut set).await;
    assert!(
        started.elapsed() < Duration::from_millis(2_000),
        "held past the stat's limit: {:?}",
        started.elapsed()
    );
    assert!(set.get(0).is_none(), "nothing read before the disk answers");
    assert!(set.missing.is_empty() && set.warnings.is_empty());
    // Still silent: a redraw that also shows another source is answered at once, with that source's frame.
    let started = Instant::now();
    pool.decode(&support::plan_of(&[intro.clone(), other]), &mut set)
        .await;
    assert!(
        started.elapsed() < Duration::from_millis(1_500),
        "{:?}",
        started.elapsed()
    );
    assert!(set.get(0).is_none());
    assert!(set.get(1).is_some(), "{:?}", set.warnings);
    assert!(set.missing.is_empty() && set.warnings.is_empty());
    assert_eq!(
        software_fallbacks(),
        fallbacks,
        "no fallback before the disk says the file is there"
    );

    // The disk answers: the paused still is drawn again, read in software.
    tokio::time::timeout(Duration::from_secs(5), redraw.notified())
        .await
        .expect("the redraw signal fires once the disk answers");
    pool.decode(&plan_with(&intro), &mut set).await;
    assert!(set.get(0).is_some(), "{:?} {:?}", set.warnings, set.missing);
    assert!(set.missing.is_empty() && set.warnings.is_empty());
    assert_eq!(software_fallbacks(), fallbacks + 1);
    slow_stats(file, None);
}

/// A disk slow to answer (spinning up, a network volume) is never called missing. Export waits for it (up to a minute);
/// the preview says nothing while it waits and never holds a frame past the first second for it, and when the disk
/// answers its redraw signal fires: a paused still is drawn again, with the video, and nothing else has to happen.
#[cfg(feature = "trial")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_disk_slow_to_answer_is_never_called_missing() {
    use klipolo_engine::render::pool::slow_stats;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("intro.mp4");
    std::fs::copy(support::fixture("bars-30fps-0.5gop.mp4"), &path).unwrap();
    let file: std::sync::Arc<str> = path.display().to_string().into();
    let src = source(&file, 30.0);
    let slow = Duration::from_millis(1_500);
    slow_stats(file.clone(), Some(slow));

    // Export: waits past the preview's one-second limit, then decodes.
    let mut export = DecoderPool::new(DecoderTuning::default(), false).patient();
    let mut set = DecodedSet::default();
    let started = Instant::now();
    export.decode(&plan_with(&src), &mut set).await;
    assert!(started.elapsed() >= slow, "{:?}", started.elapsed());
    assert!(set.get(0).is_some(), "{:?} {:?}", set.warnings, set.missing);
    assert!(set.missing.is_empty() && set.warnings.is_empty());

    // Preview: shows nothing and says nothing.
    let redraw = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut preview = DecoderPool::new(DecoderTuning::default(), false);
    preview.set_redraw(redraw.clone());
    let mut set = DecodedSet::default();
    preview.decode(&plan_with(&src), &mut set).await;
    assert!(set.get(0).is_none());
    assert!(set.missing.is_empty(), "{:?}", set.missing);
    assert!(set.warnings.is_empty(), "{:?}", set.warnings);
    // Asked again while that stat is still out (playback's next frame): answers at once (one stat per file), still
    // saying nothing.
    let started = Instant::now();
    preview.decode(&plan_with(&src), &mut set).await;
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert!(set.get(0).is_none());
    assert!(set.missing.is_empty() && set.warnings.is_empty());

    // Paused: nothing asks for another frame. The redraw signal fires once the disk answers, and the still it draws
    // shows the video (stats are still slow here: it takes the answer that woke it rather than asking again).
    tokio::time::timeout(Duration::from_secs(5), redraw.notified())
        .await
        .expect("the redraw signal fires once the disk answers");
    preview.decode(&plan_with(&src), &mut set).await;
    assert!(set.get(0).is_some(), "{:?} {:?}", set.warnings, set.missing);
    assert!(set.missing.is_empty() && set.warnings.is_empty());
    slow_stats(file, None);
}

/// A file that isn't there, on a share slow to say so, is said missing once the answer comes: its
/// redraw signal fires and the still it draws has the notice, with nothing else happening.
#[cfg(feature = "trial")]
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_not_found_is_said_on_a_paused_still_once_it_answers() {
    use klipolo_engine::render::pool::slow_stats;
    let dir = tempfile::tempdir().unwrap();
    let file: std::sync::Arc<str> = dir.path().join("intro.mp4").display().to_string().into();
    let src = source(&file, 30.0);
    slow_stats(file.clone(), Some(Duration::from_millis(1_500)));
    let redraw = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut preview = DecoderPool::new(DecoderTuning::default(), false);
    preview.set_redraw(redraw.clone());
    let mut set = DecodedSet::default();
    preview.decode(&plan_with(&src), &mut set).await;
    assert!(set.missing.is_empty() && set.warnings.is_empty());
    tokio::time::timeout(Duration::from_secs(5), redraw.notified())
        .await
        .expect("the redraw signal fires once the share answers");
    preview.decode(&plan_with(&src), &mut set).await;
    assert_eq!(set.missing, std::slice::from_ref(&file));
    assert!(set.warnings.is_empty(), "{:?}", set.warnings);
    slow_stats(file, None);
}

/// Whether an editing copy is still its original's is checked off the engine's task. A current copy is
/// read as before. A copy whose original's disk doesn't answer (`slow_stats`) is drawn without waiting past the
/// preview's limit, and nothing is said; once the original answers, the redraw signal fires and the copy is checked: one
/// that isn't the original's any more gives way to the original, and the app hears it once.
#[cfg(feature = "trial")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_slow_original_never_holds_its_editing_copy() {
    use klipolo_engine::export::shared::FrameRate;
    use klipolo_engine::media::key::SourceId;
    use klipolo_engine::media::proxy::job::CopyRecord;
    use klipolo_engine::media::{MediaEvent, MediaKind};
    use klipolo_engine::render::pool::slow_stats;
    use std::sync::{Arc, Mutex};
    let dir = tempfile::tempdir().unwrap();
    let (original, copy) = (dir.path().join("intro.mp4"), dir.path().join("1f2e.mp4"));
    for file in [&original, &copy] {
        std::fs::copy(support::fixture("bars-30fps-0.5gop.mp4"), file).unwrap();
    }
    let id = SourceId::of(&std::fs::metadata(&original).unwrap());
    let record = |source_id| CopyRecord {
        version: 1,
        source: original.display().to_string(),
        stem: "1f2e".into(),
        source_id,
        source_width: 1920,
        source_height: 1080,
        duration_us: 2_000_000,
        rate: FrameRate::whole(30),
        width: 1920,
        height: 1080,
        frames: 60,
        from_stream_start: true,
    };
    let (original, copy): (Arc<str>, Arc<str>) = (
        original.display().to_string().into(),
        copy.display().to_string().into(),
    );
    let mut src = from_copy(&copy, &original);
    let heard = Arc::new(Mutex::new(Vec::new()));
    let redraw = Arc::new(tokio::sync::Notify::new());
    let pool = || {
        let mut pool = DecoderPool::new(DecoderTuning::default(), false);
        let sink = heard.clone();
        pool.set_media_sink(Arc::new(move |e| sink.lock().unwrap().push(e)));
        pool.set_redraw(redraw.clone());
        pool
    };
    let mut set = DecodedSet::default();

    // The copy of the original as it is: read, nothing said.
    record(id).write(std::path::Path::new(&*copy)).unwrap();
    pool().decode(&plan_with(&src), &mut set).await;
    assert!(set.get(0).is_some(), "{:?}", set.warnings);
    assert!(heard.lock().unwrap().is_empty());

    // Made from the original as it was before (its record no longer matches), and the original's disk stops answering:
    // the copy is drawn meanwhile, without waiting for the original, and nothing is said.
    let then = SourceId {
        len: id.len + 1,
        ..id
    };
    record(then).write(std::path::Path::new(&*copy)).unwrap();
    slow_stats(original.clone(), Some(Duration::from_secs(4)));
    let mut preview = pool();
    let started = Instant::now();
    preview.decode(&plan_with(&src), &mut set).await;
    assert!(
        started.elapsed() < Duration::from_millis(3_000),
        "held for the original: {:?}",
        started.elapsed()
    );
    assert!(set.get(0).is_some(), "{:?}", set.warnings);
    assert!(set.missing.is_empty() && set.warnings.is_empty());
    // The next frame, while the original is still silent: drawn from the copy at once.
    src.time_us = 1_500_000;
    let started = Instant::now();
    preview.decode(&plan_with(&src), &mut set).await;
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert!(set.get(0).is_some());
    assert!(heard.lock().unwrap().is_empty(), "nothing said meanwhile");

    // The original answers: the still is drawn again, from the original, and the app hears the copy is unusable.
    tokio::time::timeout(Duration::from_secs(6), redraw.notified())
        .await
        .expect("the redraw signal fires once the original answers");
    preview.decode(&plan_with(&src), &mut set).await;
    assert!(set.get(0).is_some(), "{:?} {:?}", set.warnings, set.missing);
    assert!(set.missing.is_empty() && set.warnings.is_empty());
    assert_eq!(
        *heard.lock().unwrap(),
        [MediaEvent::Changed {
            path: original.to_string(),
            kind: MediaKind::Proxy
        }]
    );
    slow_stats(original, None);
}

/// A purged editing copy falls back to the original in place (same times), and the app hears it once, naming
/// the original, so it drops the mapping.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_copy_that_cant_be_opened_falls_back_to_the_original_once() {
    use klipolo_engine::media::{MediaEvent, MediaKind};
    use std::sync::{Arc, Mutex};
    let original = support::fixture("bars-30fps-0.5gop.mp4")
        .display()
        .to_string();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let mut pool = DecoderPool::new(DecoderTuning::default(), false);
    let sink = heard.clone();
    pool.set_media_sink(Arc::new(move |e| sink.lock().unwrap().push(e)));
    let src = from_copy("/nope/cache/purged.mp4", &original);
    let mut set = DecodedSet::default();
    for at in [1_000_000, 1_500_000] {
        let src = SourceRef {
            time_us: at,
            ..src.clone()
        };
        pool.decode(&plan_with(&src), &mut set).await;
        assert!(set.get(0).is_some(), "{:?}", set.warnings);
        assert!(set.warnings.is_empty(), "{:?}", set.warnings);
    }
    assert_eq!(
        *heard.lock().unwrap(),
        [MediaEvent::Changed {
            path: original.clone(),
            kind: MediaKind::Proxy
        }]
    );
    // The app dropped the mapping: the plan names the original, and the fallen-back decoder serves it as it is.
    let plain = source(&original, 30.0);
    pool.decode(&plan_with(&plain), &mut set).await;
    assert!(set.get(0).is_some());
    assert_eq!(heard.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn unknown_fps_is_probed() {
    let path = support::fixture("bars-30fps-0.5gop.mp4");
    assert_eq!(probe_fps(&path), 30.0);
    let mut pool = DecoderPool::new(DecoderTuning::default(), false);
    let src = source(&path.display().to_string(), 0.0);
    let mut set = DecodedSet::default();
    pool.decode(&plan_with(&src), &mut set).await;
    assert!(set.get(0).is_some());
}

#[test]
fn export_decodes_at_full_size_and_the_preview_at_preview_size() {
    use klipolo_engine::render::{EXPORT_TUNING, PREVIEW_TUNING};
    let export = DecoderPool::new(EXPORT_TUNING, false);
    for (role, lane) in [
        (VideoRole::Main, 0),
        (VideoRole::Camera, 0),
        (VideoRole::Overlay, 1),
    ] {
        assert_eq!(export.tuning_for(role, lane).max_output, None, "{role:?}");
    }
    let preview = DecoderPool::new(PREVIEW_TUNING, false);
    assert_eq!(
        preview.tuning_for(VideoRole::Main, 0).max_output,
        Some((1920, 1080))
    );
    assert_eq!(
        preview.tuning_for(VideoRole::Overlay, 0).max_output,
        Some((1280, 720))
    );
}

// A seek pattern scaled into the 4 s fixture (5.0, 1.2, 1.3, 9.9, 0.0, 7.5, 2.0, 8.8 on 10 s): far, back, a small step
// on, to the end, to the start.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn seeks_land_on_the_frame_asked_for() {
    let path = support::fixture("bars-30fps-0.5gop.mp4");
    let decoder = spawn_decoder("seeks", path, 30.0, 0.0, false, DecoderTuning::default())
        .await
        .unwrap();
    for t in [2.0f32, 0.5, 0.6, 3.9, 0.0, 3.0, 0.8, 3.5] {
        let frame = decoder.get_frame(t).await.expect("a frame");
        let frame_s = frame.time_s();
        assert!(
            (frame_s - (t * 30.0).floor() / 30.0).abs() < 0.5 / 30.0,
            "asked {t}, got {frame_s}"
        );
    }
}

/// A seek to a keyframe off the 1/100 s grid (every NTSC one) shows that keyframe, not the frame before it: the
/// reader's range starts just after the keyframe. Rounded to 1/100 s it started just before, and AVFoundation showed
/// the previous frame re-timed to the start (labelled as the keyframe). Each frame of the fixture is a flat grey whose
/// luma names it.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn an_ntsc_seek_to_a_keyframe_shows_that_keyframe() {
    use klipolo_engine::vendor::cap::video_decode::KeyframeIndex;

    let path = support::fixture("ntsc-luma-10s.mp4");
    let index = KeyframeIndex::build(&path).unwrap();
    let &(frame, time) = index
        .keyframes()
        .iter()
        .find(|&&(_, t)| (1.0..4.0).contains(&t) && (t * 100.0).round() / 100.0 < t - 1e-4)
        .expect("a keyframe that 1/100 s rounding puts just before");
    // As the pool asks: on the source's own grid, in the middle of the frame.
    let fps = 30_000.0 / 1001.0;
    let decoder = spawn_decoder("ntsc", path, fps, 0.0, false, DecoderTuning::default())
        .await
        .unwrap();
    decoder.get_frame(9.0).await.expect("a frame near the end");
    let got = decoder
        .get_frame((time + 0.5 / fps) as f32)
        .await
        .expect("the keyframe");
    let y = got.y_plane().expect("NV12");
    let luma = i32::from(y[90 * got.y_stride() as usize + 160]);
    let want = 16 + 8 * (frame % 25) as i32;
    assert!(
        (luma - want).abs() <= 3,
        "keyframe {frame} at {time:.4} s: luma {luma}, want {want} (the frame before is {})",
        want - 8
    );
}

/// 10-bit HEVC (a phone's Main 10) is read by AVAssetReader through VideoToolbox as 8-bit NV12: each
/// seek lands on its frame, whose luma names it, with neutral chroma, within a few code values of the 8-bit source.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn ten_bit_hevc_is_read_by_videotoolbox_on_its_frame() {
    let path = support::fixture("hevc10-luma-2s.mov");
    let decoder = spawn_decoder("hevc10", path, 30.0, 0.0, false, DecoderTuning::default())
        .await
        .unwrap();
    assert_eq!(decoder.decoder_type(), DecoderType::AVAssetReader);
    for n in [45u64, 7, 52, 30] {
        let frame = decoder
            .get_frame(((n as f64 + 0.5) / 30.0) as f32)
            .await
            .expect("a frame");
        assert_eq!((frame.width(), frame.height()), (640, 360));
        let y = frame.y_plane().expect("NV12");
        let luma = i32::from(y[180 * frame.y_stride() as usize + 320]);
        let want = 16 + 8 * (n % 25) as i32;
        assert!(
            (luma - want).abs() <= 3,
            "frame {n}: luma {luma}, want {want}"
        );
        let uv = frame.uv_plane().expect("NV12");
        let at = 90 * frame.uv_stride() as usize + 320;
        let (cb, cr) = (i32::from(uv[at]), i32::from(uv[at + 1]));
        assert!(
            (cb - 128).abs() <= 3 && (cr - 128).abs() <= 3,
            "frame {n}: chroma {cb} {cr}"
        );
    }
}

/// The thumbnail scan's stored index records the codec, so the reader's format is decided from it once, without
/// opening the file again: 10-bit HEVC is read as 8-bit; H.264 High 10 (no VideoToolbox decoder) falls back.
#[cfg(target_os = "macos")]
#[test]
fn a_stored_index_decides_the_readers_format_from_its_codec() {
    use klipolo_engine::media::keyframes::StoredKeys;
    use klipolo_engine::vendor::cap::video_decode::KeyframeIndex;
    let stored = |codec: &str, pixel: &str| StoredKeys {
        pts: vec![0, 15],
        time_base: (1, 30),
        start: 0,
        fps: 30.0,
        duration_secs: 2.0,
        pixel_format: pixel.into(),
        codec: codec.into(),
        width: 640,
        height: 360,
    };
    let info = |codec: &str, pixel: &str| {
        KeyframeIndex::from_stored(stored(codec, pixel)).cached_video_info()
    };
    assert!(info("hevc", "yuv420p10le").is_some());
    assert!(info("h264", "yuv420p").is_some());
    assert!(info("h264", "yuv420p10le").is_none());
}

/// The luma at the centre of the main layer's frame when the pool shows `src` (the luma fixtures name each frame:
/// 16 + 8 × (frame mod 25)).
async fn shown_luma(pool: &mut DecoderPool, src: &SourceRef) -> i32 {
    let mut set = DecodedSet::default();
    pool.decode(&plan_with(src), &mut set).await;
    let (frame, _) = set.get(0).expect("a frame");
    let y = frame.y_plane().expect("NV12");
    i32::from(y[90 * frame.y_stride() as usize + 160])
}

/// The pool numbers a 29.97 fps source's frames on its own grid, not a whole-number one that drifts a little more each
/// frame: late in the clip, the frame shown at a time is still the one whose span holds it (frame N from N × 1001/30000
/// s), at its start as the timeline asks (whole µs) and three quarters of the way through it. On a 30 fps grid frame
/// 290's last quarter showed frame 291. Each frame of the fixture is a flat grey whose luma names it; both decoders.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn ntsc_frames_late_in_a_clip_are_the_ones_shown_at_their_time() {
    let path = support::fixture("ntsc-luma-10s.mp4");
    for force_ffmpeg in [false, true] {
        let mut pool = DecoderPool::new(DecoderTuning::default(), force_ffmpeg);
        let mut src = source(&path.display().to_string(), 30_000.0 / 1001.0);
        for n in [240u64, 265, 290] {
            for quarter in [0, 3] {
                // (n + quarter / 4) frames of 1001/30000 s, in whole µs (never before the time).
                src.time_us = ((4 * n + quarter) * 100_100).div_ceil(12) as i64;
                let luma = shown_luma(&mut pool, &src).await;
                let want = 16 + 8 * (n % 25) as i32;
                assert!(
                    (luma - want).abs() <= 3,
                    "force_ffmpeg={force_ffmpeg}: frame {n} + {quarter}/4 at {} µs shows luma {luma}, want {want}",
                    src.time_us
                );
            }
        }
    }
}

/// A timeline frame's start rounded down to whole µs (frame 1 at 30 fps starts at 33 333 µs, a third of a µs early)
/// shows that frame, not the one before: the pool's 1 µs of slack. Before, a 30 fps source on a 30 fps timeline showed
/// the previous frame again at about one frame start in four. At 29.97 fps too (frame 290 at 9 676 333 µs).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn frame_starts_rounded_down_to_whole_us_show_their_own_frame() {
    // Frames whose start in µs has a fraction under a half: n × 10^6 / 30 for n = 1 mod 3, n × 1001 × 10^6 / 30000 for
    // n = 2 mod 3.
    let cases: [(&str, (u64, u64), [u64; 4]); 2] = [
        ("luma-30fps-2s.mp4", (30, 1), [1, 4, 31, 46]),
        ("ntsc-luma-10s.mp4", (30_000, 1001), [2, 242, 266, 290]),
    ];
    for (name, (num, den), frames) in cases {
        let path = support::fixture(name);
        for force_ffmpeg in [false, true] {
            let mut pool = DecoderPool::new(DecoderTuning::default(), force_ffmpeg);
            let mut src = source(&path.display().to_string(), num as f64 / den as f64);
            for n in frames {
                // Rounded to the nearest µs, as the timeline's frame starts are: here, down.
                src.time_us = ((n * 1_000_000 * den + num / 2) / num) as i64;
                assert!(
                    (src.time_us as u64) * num < n * 1_000_000 * den,
                    "{name}: frame {n} rounds down"
                );
                let luma = shown_luma(&mut pool, &src).await;
                let want = 16 + 8 * (n % 25) as i32;
                assert!(
                    (luma - want).abs() <= 3,
                    "{name}, force_ffmpeg={force_ffmpeg}: frame {n} at {} µs shows luma {luma}, want {want}",
                    src.time_us
                );
            }
        }
    }
}

/// On a short GOP (an editing copy's: a keyframe every 0.5 s), a still a newer seek replaced stops its read at the
/// nearest frame of its GOP read so far (a scrub moving on doesn't wait for the rest of the GOP); one nobody replaced
/// is read to its own frame.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_superseded_seek_stops_at_the_nearest_frame_read() {
    use klipolo_engine::vendor::cap::rendering::decoder::Superseded;
    let path = support::fixture("bars-30fps-0.5gop.mp4");
    let mut src = source(&path.display().to_string(), 30.0);
    let mut pool = DecoderPool::new(DecoderTuning::default(), false);
    let mut set = DecodedSet::default();
    src.time_us = 0;
    pool.decode(&plan_with(&src), &mut set).await;
    // Frame 44: its GOP starts at frame 30 (a keyframe every 15 frames), far past what the first read cached.
    src.time_us = 1_480_000;
    let replaced: Superseded = std::sync::Arc::new(|| true);
    pool.decode_unless(&plan_with(&src), &mut set, Some(&replaced))
        .await;
    let (frame, _) = set.get(0).expect("the nearest frame read");
    let shown = (f64::from(frame.time_s()) * 30.0).round() as i64;
    assert!(
        (30..44).contains(&shown),
        "a replaced seek shows a frame of its GOP read before it, not {shown}"
    );
    // Nobody replaced it: its own frame.
    src.time_us = 3_480_000;
    let kept: Superseded = std::sync::Arc::new(|| false);
    pool.decode_unless(&plan_with(&src), &mut set, Some(&kept))
        .await;
    let (frame, _) = set.get(0).expect("its frame");
    assert_eq!((f64::from(frame.time_s()) * 30.0).round() as i64, 104);
}

/// On a long-GOP original (a keyframe every 10 s) a replaced seek is still read to its own frame: the nearest frame
/// read could be seconds away, so only GOPs of at most a second stop early.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_superseded_seek_on_a_long_gop_keeps_its_exact_frame() {
    use klipolo_engine::vendor::cap::rendering::decoder::Superseded;
    let path = support::fixture("longgop-20s-1080p.mp4");
    let mut src = source(&path.display().to_string(), 30.0);
    let mut pool = DecoderPool::new(DecoderTuning::default(), false);
    let mut set = DecodedSet::default();
    src.time_us = 0;
    pool.decode(&plan_with(&src), &mut set).await;
    let replaced: Superseded = std::sync::Arc::new(|| true);
    // Frames 200 (GOP 0–299) and 450 (GOP 300–599), each far past what the reader had read.
    for (at_us, want) in [(6_680_000, 200), (15_010_000, 450)] {
        src.time_us = at_us;
        pool.decode_unless(&plan_with(&src), &mut set, Some(&replaced))
            .await;
        let (frame, _) = set.get(0).expect("its frame");
        assert_eq!((f64::from(frame.time_s()) * 30.0).round() as i64, want);
    }
}

/// Seek latency: seeded random seeks on two fixtures in blocks of 50, each on a fresh decoder after an
/// untimed first frame. Per block: get_frame p50 / p95 ms, readers made, frames off target, timeouts and `uptime`'s
/// load (a busy machine moves the numbers, so compare runs interleaved). Needs `scripts/gen-engine-fixtures.sh --bench`.
/// Each seek asks for the middle of its frame, as the preview does (`render::pool::request_s`): the decoder numbers
/// frames `floor(t × fps)`, so an f32 frame start can round below its frame and count as off target.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement: run by hand"]
async fn scrub_p95() {
    use klipolo_engine::vendor::cap::video_decode::avassetreader::readers_created;

    fn next(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }
    fn pct(values: &[f64], p: f64) -> f64 {
        if values.is_empty() {
            return f64::NAN;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[((sorted.len() - 1) as f64 * p).round() as usize]
    }
    fn load() -> String {
        let out = std::process::Command::new("uptime").output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        text.unwrap_or_default()
            .rsplit("load averages:")
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    const BLOCKS: usize = 4;
    const SEEKS: usize = 50;
    for (name, frames) in [
        ("bars-30fps-0.5gop.mp4", 120u64),
        ("screen-60s-1080p30.mp4", 1800),
    ] {
        let path = support::fixture(name);
        let (mut all_ms, mut made, mut off_target, mut timeouts) = (Vec::new(), 0, 0, 0);
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for block in 0..BLOCKS {
            let decoder = spawn_decoder(
                "scrub",
                path.clone(),
                30.0,
                0.0,
                false,
                DecoderTuning::default(),
            )
            .await
            .unwrap();
            // Opening (the keyframe index, the first reader) isn't a seek; a loaded machine can take seconds.
            decoder.get_frame_initial(0.0).await;
            let before = readers_created();
            let mut block_ms = Vec::with_capacity(SEEKS);
            for _ in 0..SEEKS {
                let n = next(&mut seed) % frames;
                let t = ((n as f64 + 0.5) / 30.0) as f32;
                let started = Instant::now();
                // A request that times out switches the handle to FFmpeg: the rest of the block wouldn't count.
                let Some(frame) = decoder.get_frame(t).await else {
                    timeouts += 1;
                    break;
                };
                block_ms.push(started.elapsed().as_secs_f64() * 1e3);
                if (f64::from(frame.time_s()) - n as f64 / 30.0).abs() >= 0.5 / 30.0 {
                    off_target += 1;
                }
            }
            let block_made = readers_created() - before;
            made += block_made;
            println!(
                "{name} block {block}: p50 {:.1} p95 {:.1} ms over {} seeks, readers made {block_made} | load {}",
                pct(&block_ms, 0.5),
                pct(&block_ms, 0.95),
                block_ms.len(),
                load()
            );
            all_ms.extend(block_ms);
        }
        println!(
            "SUMMARY {name}: p50 {:.1} p95 {:.1} ms over {} seeks, readers made {made}, off target {off_target}, timeouts {timeouts}",
            pct(&all_ms, 0.5),
            pct(&all_ms, 0.95),
            all_ms.len()
        );
    }
}

/// Ruler drags as the editor makes them, on one file through the preview's decoder pool (an editing copy:
/// `KLIPOLO_SCRUB_FILE`, `KLIPOLO_SCRUB_SECONDS` long; the 0.5 s GOP fixture by default). A pointer moves every 16.7 ms
/// for 10 s along a sweep: back and forth over `range` seconds of the timeline every 4 s. The decoding side takes only
/// the newest move each time it is free, as the engine's actor does (its seek is a `watch`). Per speed: each move → the
/// first decode of it or a newer one, p50 / p95 ms (input → shown, less drawing and presenting), the decodes, and the
/// readers made. Each decode stops its read once a newer move arrived, as the engine's stills do
/// (`DecoderPool::decode_unless`); `KLIPOLO_SCRUB_NO_CUT=1` reads every frame to the end instead.
/// `KLIPOLO_SCRUB_FFMPEG=1` reads with FFmpeg instead of AVAssetReader; `--features trial` adds what readers and reads
/// cost.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement: run by hand"]
async fn scrub_drag() {
    use klipolo_engine::vendor::cap::video_decode::avassetreader::readers_created;

    fn pct(values: &[f64], p: f64) -> f64 {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted
            .get(((sorted.len().max(1) - 1) as f64 * p).round() as usize)
            .copied()
            .unwrap_or(f64::NAN)
    }

    let path = std::env::var("KLIPOLO_SCRUB_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| support::fixture("bars-30fps-0.5gop.mp4"));
    let seconds: f64 = std::env::var("KLIPOLO_SCRUB_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4.0);
    let ffmpeg = std::env::var("KLIPOLO_SCRUB_FFMPEG").is_ok();
    let cut = std::env::var("KLIPOLO_SCRUB_NO_CUT").is_err();
    let fps = probe_fps(&path);
    let mut src = source(&path.display().to_string(), fps);
    let mut pool = DecoderPool::new(klipolo_engine::render::PREVIEW_TUNING, ffmpeg);
    let mut set = DecodedSet::default();
    // Opening the decoder isn't a seek.
    src.time_us = 0;
    pool.decode(&plan_with(&src), &mut set).await;
    for (speed, range) in [
        ("fine (2 s/s)", 4.0),
        ("medium (10 s/s)", 20.0),
        ("whole ruler", f64::INFINITY),
    ] {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let range: f64 = f64::min(range, seconds - 0.2);
        let from = (seconds - range) / 2.0;
        let (tx, mut rx) = tokio::sync::watch::channel((0u32, 0i64));
        // The move being decoded: a newer one supersedes it, as a newer seek does a still.
        let decoding = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let superseded: klipolo_engine::vendor::cap::rendering::decoder::Superseded = {
            let (latest, decoding) = (rx.clone(), decoding.clone());
            std::sync::Arc::new(move || {
                latest.borrow().0 != decoding.load(std::sync::atomic::Ordering::Relaxed)
            })
        };
        let started = Instant::now();
        let mover = tokio::spawn(async move {
            let mut moves = Vec::new();
            let mut tick = tokio::time::interval(Duration::from_micros(16_667));
            for seq in 1u32.. {
                tick.tick().await;
                let t = started.elapsed().as_secs_f64();
                if t >= 10.0 {
                    break;
                }
                let at = from + range * (1.0 - (t / 4.0 * std::f64::consts::TAU).cos()) / 2.0;
                moves.push((seq, Instant::now()));
                tx.send_replace((seq, (at * 1e6) as i64));
            }
            moves
        });
        let (mut done, mut decode_ms, mut empty) = (Vec::new(), Vec::new(), 0);
        let made = readers_created();
        let costs = ReadCosts::now();
        while rx.changed().await.is_ok() {
            let (seq, us) = *rx.borrow_and_update();
            src.time_us = us;
            decoding.store(seq, std::sync::atomic::Ordering::Relaxed);
            let started = Instant::now();
            pool.decode_unless(&plan_with(&src), &mut set, cut.then_some(&superseded))
                .await;
            decode_ms.push(started.elapsed().as_secs_f64() * 1e3);
            empty += usize::from(set.get(0).is_none());
            done.push((seq, Instant::now()));
        }
        let moves = mover.await.unwrap();
        let made = readers_created() - made;
        let latency: Vec<f64> = moves
            .iter()
            .filter_map(|(seq, at)| {
                done.iter()
                    .find(|(s, d)| s >= seq && d >= at)
                    .map(|(_, d)| d.duration_since(*at).as_secs_f64() * 1e3)
            })
            .collect();
        println!(
            "{speed} over {range:.1} s: move → decoded p50 {:.1} p95 {:.1} ms ({} moves); {} decodes p50 {:.1} p95 \
             {:.1} max {:.1} ms; readers made {made}{}; {empty} without a frame",
            pct(&latency, 0.5),
            pct(&latency, 0.95),
            latency.len(),
            decode_ms.len(),
            pct(&decode_ms, 0.5),
            pct(&decode_ms, 0.95),
            pct(&decode_ms, 1.0),
            costs.since(made, decode_ms.len()),
        );
    }
}

/// What making readers and reading frames cost (`--features trial`: the decoders' measurement counters).
#[cfg(target_os = "macos")]
struct ReadCosts {
    #[cfg(feature = "trial")]
    at: ((u64, u64, u64), (u64, u64, u64)),
}

#[cfg(target_os = "macos")]
impl ReadCosts {
    fn now() -> Self {
        Self {
            #[cfg(feature = "trial")]
            at: (
                klipolo_engine::vendor::cap::video_decode::avassetreader::reader_ns(),
                klipolo_engine::vendor::cap::rendering::decoder::frames_read(),
            ),
        }
    }

    /// Per reader made and per decode since `now`; nothing without the counters.
    #[allow(unused_variables)]
    fn since(&self, readers: u64, decodes: usize) -> String {
        #[cfg(feature = "trial")]
        {
            let ((asset, start, cancel), (frames, read, first)) = self.at;
            let (a, s, c) = klipolo_engine::vendor::cap::video_decode::avassetreader::reader_ns();
            let (f, r, fi) = klipolo_engine::vendor::cap::rendering::decoder::frames_read();
            let per_reader = |ns: u64| ns as f64 / 1e6 / readers.max(1) as f64;
            let decodes = decodes.max(1) as f64;
            format!(
                " (cancel {:.1} + asset {:.1} + start {:.1} ms each); per decode {:.1} frames read in {:.1} ms \
                 (first {:.1} ms)",
                per_reader(c - cancel),
                per_reader(a - asset),
                per_reader(s - start),
                (f - frames) as f64 / decodes,
                (r - read) as f64 / 1e6 / decodes,
                (fi - first) as f64 / 1e6 / decodes,
            )
        }
        #[cfg(not(feature = "trial"))]
        String::new()
    }
}
