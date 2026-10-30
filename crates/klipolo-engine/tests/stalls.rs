//! Exports survive a slow or stalled decoder. An export failed twice with "This file couldn't be
//! read" when AVAssetReader stopped vending for over 3 s on a loaded Mac (at frames 210 and 881 of a 23.976 fps
//! file), and its software fallback, started cold, missed the same 3 s. Here the stall is injected
//! (`decode_wait::hooks`, trial builds) into `film-1080p-gop100-12s.mp4` (keyframes at frames 0, 100 and 200), never by
//! loading the computer.
//!
//! Here: the FFmpeg path, which every platform has (`a_slow_…`, in CI's slow-disk step). The AVAssetReader cases are in
//! `stalls_export.rs` and `stalls_preview.rs` (a Mac with VideoToolbox; `--features trial -- --include-ignored
//! --test-threads=2`).
#![cfg(feature = "trial")]

mod support;

use std::time::{Duration, Instant};

use klipolo_engine::decode_wait::DecodeWait;
use klipolo_engine::decode_wait::hooks::{self, SlowSoftware};
use klipolo_engine::render::EXPORT_TUNING;
use klipolo_engine::render::pool::{DecodedSet, DecoderPool};
use klipolo_engine::vendor::cap::rendering::decoder::DecoderTuning;
use support::plan_with;
use support::stalls::{
    Hooked, KEYFRAME, STALL_AT, TURN, film, film_luma, middle_luma, seconds, source, stops_at_210,
};

/// Every platform's FFmpeg path (Windows and Linux decode with it; a Mac's compatibility mode too): a decoder that
/// stalls under export's wait is opened again at the keyframe before the frame, never from frame 0, and the frame is
/// read; one that stalls again (there is no other decoder to try) is "couldn't be read", named, within two windows.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_slow_software_decoder_that_stalls_is_reopened_at_its_keyframe() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (file, stuck) = (
        film(dir.path(), "software.mp4"),
        film(dir.path(), "stuck.mp4"),
    );
    let _hooked = Hooked(vec![file.clone(), stuck.clone()]);
    let tuning = DecoderTuning {
        wait: DecodeWait::Progress {
            stall_after: Duration::from_millis(1_500),
            cap: Duration::from_secs(10),
        },
        ..EXPORT_TUNING
    };
    hooks::slow_software(file.clone(), [stops_at_210(1)]);
    let mut pool = DecoderPool::new(tuning, true).patient();
    let mut set = DecodedSet::default();
    pool.decode(&plan_with(&source(&file, 205)), &mut set).await;
    let frame = set.get(0).expect("frame 205").0.clone();
    assert!(middle_luma(&frame).abs_diff(film_luma(205)) <= 3);

    let started = Instant::now();
    pool.decode(&plan_with(&source(&file, STALL_AT)), &mut set)
        .await;
    let took = started.elapsed();
    let frame = set
        .get(0)
        .expect("frame 210 from the decoder opened again")
        .0
        .clone();
    assert!(
        middle_luma(&frame).abs_diff(film_luma(STALL_AT)) <= 3,
        "frame 210's luma: {}",
        middle_luma(&frame)
    );
    assert!(
        took >= Duration::from_millis(1_400) && took < Duration::from_secs(8),
        "one stall window, then the new decoder: {took:?}"
    );
    let decoded = hooks::software_decoded(&file);
    let reopened = decoded.iter().find(|(d, _)| *d == 1).map(|(_, s)| *s);
    assert!(
        reopened.is_some_and(|s| s >= seconds(KEYFRAME) - 0.02),
        "the new decoder's first frame is the keyframe before 210, not frame 0: {reopened:?}"
    );

    // The decoder opened again stalls too: nothing else can read it.
    hooks::slow_software(stuck.clone(), [stops_at_210(2)]);
    let mut pool = DecoderPool::new(tuning, true).patient();
    pool.decode(&plan_with(&source(&stuck, 205)), &mut set)
        .await;
    assert!(set.get(0).is_some(), "{:?}", set.warnings);
    let started = Instant::now();
    pool.decode(&plan_with(&source(&stuck, STALL_AT)), &mut set)
        .await;
    assert!(set.get(0).is_none());
    assert_eq!(set.warnings, ["This file couldn't be read: stuck.mp4"]);
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "{:?}",
        started.elapsed()
    );
}

/// A software decoder opened again after a stall, whose first frame is slow (a cold start
/// under load: the open ticks nothing until it is done), gets the 30 s start window rather than export's 9 s without
/// progress, and the worker's heartbeat hears it meanwhile, so a slow recovery never falls silent.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_slow_software_start_keeps_beating_and_gets_its_30_s_window() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let file = film(dir.path(), "cold start.mp4");
    let _hooked = Hooked(vec![file.clone()]);
    let tuning = DecoderTuning {
        wait: DecodeWait::Progress {
            stall_after: Duration::from_secs(2),
            cap: Duration::from_secs(20),
        },
        ..EXPORT_TUNING
    };
    hooks::slow_software(
        file.clone(),
        [
            stops_at_210(1),
            // The decoder opened again: its first frame takes 6 s, with no progress meanwhile.
            SlowSoftware {
                decoders: 1..2,
                from_s: 0.0,
                per_frame: Duration::from_secs(6),
                frames: 1,
            },
        ],
    );
    let mut pool = DecoderPool::new(tuning, true).patient();
    let beat = pool.decode_beat();
    let mut set = DecodedSet::default();
    pool.decode(&plan_with(&source(&file, 205)), &mut set).await;
    assert!(set.get(0).is_some(), "{:?}", set.warnings);
    beat.take();

    let started = Instant::now();
    pool.decode(&plan_with(&source(&file, STALL_AT)), &mut set)
        .await;
    let took = started.elapsed();
    let frame = set
        .get(0)
        .unwrap_or_else(|| panic!("frame 210 after a slow start: {:?}", set.warnings))
        .0
        .clone();
    assert!(middle_luma(&frame).abs_diff(film_luma(STALL_AT)) <= 3);
    assert!(
        took >= Duration::from_millis(7_500) && took < Duration::from_secs(15),
        "one 2 s stall, then the 6 s start: {took:?}"
    );
    assert!(beat.take(), "the heartbeat heard the recovery");
}

/// Export's first open of a source is a chain of bounded steps (the stats, the reader's start,
/// an FFmpeg start after it fails), and the worker's heartbeat hears it, so a slow one can't fall silent past the
/// parent's watchdog.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
async fn a_slow_first_open_keeps_the_heartbeat() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let file = film(dir.path(), "slow open.mp4");
    let _hooked = Hooked(vec![file.clone()]);
    let tuning = DecoderTuning {
        wait: DecodeWait::Progress {
            stall_after: Duration::from_secs(2),
            cap: Duration::from_secs(20),
        },
        ..EXPORT_TUNING
    };
    // The first decoder's first frame takes 3 s, with no progress meanwhile (inside its 30 s start window).
    hooks::slow_software(
        file.clone(),
        [SlowSoftware {
            decoders: 0..1,
            from_s: 0.0,
            per_frame: Duration::from_secs(3),
            frames: 1,
        }],
    );
    let mut pool = DecoderPool::new(tuning, true).patient();
    let beat = pool.decode_beat();
    let mut set = DecodedSet::default();
    pool.decode(&plan_with(&source(&file, 5)), &mut set).await;
    assert!(set.get(0).is_some(), "{:?}", set.warnings);
    assert!(beat.take(), "the heartbeat heard the first open");
}
