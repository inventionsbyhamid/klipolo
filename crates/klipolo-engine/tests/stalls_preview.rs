//! On a Mac: the preview's AVAssetReader stalls. It keeps Cap's fixed deadline and opens a stalled reader
//! again before decoding in software. Run on a Mac: `cargo test -p klipolo-engine --features trial --test stalls_preview
//! -- --include-ignored --test-threads=2`.
#![cfg(all(feature = "trial", target_os = "macos"))]

mod support;

use std::time::{Duration, Instant};

use klipolo_engine::decode_wait::hooks::{self, ReadStall, Stall};
use klipolo_engine::render::PREVIEW_TUNING;
use klipolo_engine::render::pool::{DecodedSet, DecoderPool};
use klipolo_engine::vendor::cap::rendering::decoder::software_fallbacks;
use support::plan_with;
use support::stalls::{Hooked, STALL_AT, TURN, at_210, film, film_luma, middle_luma, source};

/// The preview keeps Cap's fixed deadline (3 s at 1080p): a reader stopped for 6 s is given up on then, reopened
/// once at the keyframe before, and the new reader answers; no software decoding for the rest of the session. A
/// reader that stalls again after its reopen goes to software.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh and VideoToolbox"]
async fn preview_keeps_its_fixed_deadline_and_reopens_once() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (once, twice) = (film(dir.path(), "once.mp4"), film(dir.path(), "twice.mp4"));
    let _hooked = Hooked(vec![once.clone(), twice.clone()]);
    let mut set = DecodedSet::default();

    hooks::stall_reads(
        once.clone(),
        [at_210(0..1, Stall::For(Duration::from_secs(6)))],
    );
    let fallbacks = software_fallbacks();
    let mut pool = DecoderPool::new(PREVIEW_TUNING, false);
    let started = Instant::now();
    pool.decode(&plan_with(&source(&once, STALL_AT)), &mut set)
        .await;
    let took = started.elapsed();
    let frame = set
        .get(0)
        .expect("frame 210 from the reopened reader")
        .0
        .clone();
    assert!(middle_luma(&frame).abs_diff(film_luma(STALL_AT)) <= 3);
    assert!(
        took >= Duration::from_millis(2_900) && took < Duration::from_millis(4_800),
        "the 3 s deadline, then the reopened reader: {took:?}"
    );
    assert_eq!(software_fallbacks(), fallbacks, "still hardware");
    assert_eq!(hooks::readers_made(&once).len(), 2);

    hooks::stall_reads(
        twice.clone(),
        [at_210(0..2, Stall::For(Duration::from_secs(6)))],
    );
    let mut pool = DecoderPool::new(PREVIEW_TUNING, false);
    let started = Instant::now();
    pool.decode(&plan_with(&source(&twice, STALL_AT)), &mut set)
        .await;
    let took = started.elapsed();
    let frame = set.get(0).expect("frame 210 in software").0.clone();
    assert!(middle_luma(&frame).abs_diff(film_luma(STALL_AT)) <= 3);
    assert!(
        took >= Duration::from_millis(5_900) && took < Duration::from_secs(10),
        "two deadlines, then software: {took:?}"
    );
    assert_eq!(software_fallbacks(), fallbacks + 1);
}

/// One passing stall per session doesn't send a file to software. Once the reopened reader has
/// gone a quiet spell without a stall (2 minutes; 2 s here), the next stall opens it again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh and VideoToolbox"]
async fn the_preview_may_reopen_again_after_a_quiet_spell() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let file = film(dir.path(), "quiet.mp4");
    let _hooked = Hooked(vec![file.clone()]);
    hooks::set_reopen_quiet(Some(Duration::from_secs(2)));
    let six = Stall::For(Duration::from_secs(6));
    hooks::stall_reads(
        file.clone(),
        [
            at_210(0..1, six),
            // The reopened reader stalls too, a little later (frame 215: the same lane reads on to it).
            ReadStall {
                readers: 1..2,
                at_frame: 215,
                stall: six,
            },
        ],
    );
    let fallbacks = software_fallbacks();
    let mut pool = DecoderPool::new(PREVIEW_TUNING, false);
    let mut set = DecodedSet::default();
    pool.decode(&plan_with(&source(&file, STALL_AT)), &mut set)
        .await;
    assert!(set.get(0).is_some(), "{:?}", set.warnings);
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    let started = Instant::now();
    pool.decode(&plan_with(&source(&file, 215)), &mut set).await;
    let took = started.elapsed();
    let frame = set
        .get(0)
        .expect("frame 215 from a reader opened again")
        .0
        .clone();
    assert!(middle_luma(&frame).abs_diff(film_luma(215)) <= 3);
    assert!(
        took >= Duration::from_millis(2_900) && took < Duration::from_millis(4_800),
        "{took:?}"
    );
    assert_eq!(software_fallbacks(), fallbacks, "still hardware");
    assert_eq!(hooks::readers_made(&file).len(), 3);
}

/// A still that a newer seek replaced while its reader stalled isn't waited for again. The reader
/// is opened again for the frames to come, but the stale frame is skipped, not read again or decoded in software.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh and VideoToolbox"]
async fn a_stale_still_never_waits_on_the_reader_opened_again() {
    use klipolo_engine::vendor::cap::rendering::decoder::Superseded;
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let file = film(dir.path(), "stale.mp4");
    let _hooked = Hooked(vec![file.clone()]);
    hooks::stall_reads(
        file.clone(),
        [at_210(0..2, Stall::For(Duration::from_secs(6)))],
    );
    let fallbacks = software_fallbacks();
    let mut pool = DecoderPool::new(PREVIEW_TUNING, false);
    let mut set = DecodedSet::default();
    let replaced: Superseded = std::sync::Arc::new(|| true);
    let started = Instant::now();
    pool.decode_unless(
        &plan_with(&source(&file, STALL_AT)),
        &mut set,
        Some(&replaced),
    )
    .await;
    let took = started.elapsed();
    assert!(set.get(0).is_none(), "the stale frame is skipped");
    assert!(
        set.warnings.is_empty() && set.missing.is_empty(),
        "{:?}",
        set.warnings
    );
    assert!(
        took < Duration::from_millis(4_500),
        "one deadline, no second wait: {took:?}"
    );
    assert_eq!(
        software_fallbacks(),
        fallbacks,
        "no software for a stale frame"
    );
}
