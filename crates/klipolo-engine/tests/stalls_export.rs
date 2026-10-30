//! On a Mac: AVAssetReader stalls during an export (in process, as the worker runs it). The reader is
//! stalled by `decode_wait::hooks` in `film-1080p-gop100-12s.mp4`: waited out, opened again at the keyframe before,
//! decoded in software from there, or failing named when nothing progresses; and the worker's heartbeat. Run on a Mac:
//! `cargo test -p klipolo-engine --features trial --test stalls_export -- --include-ignored --test-threads=2`.
#![cfg(all(feature = "trial", target_os = "macos"))]

mod support;

use std::time::{Duration, Instant};

use klipolo_engine::decode_wait::DecodeWait;
use klipolo_engine::decode_wait::hooks::{self, SlowSoftware, Stall};
use klipolo_engine::export::protocol::{ErrorCode, WorkerLine};
use klipolo_engine::export::worker::Ended;
use klipolo_engine::vendor::cap::rendering::decoder::software_fallbacks;
use support::export_film::{AROUND_THE_STALL, export_film, film_job, shows};
use support::stalls::{
    FILM_FPS, Hooked, KEYFRAME, STALL_AT, TURN, at_210, film, seconds, stops_at_210,
};

/// A reader that stops for 6 s (a Mac under load) is waited out: export's wait follows the reader's
/// progress, not the preview's 3 s, so nothing falls back and no reader is made again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh, a GPU and VideoToolbox"]
async fn export_waits_out_a_slow_reader() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let file = film(dir.path(), "slow reader.mp4");
    let _hooked = Hooked(vec![file.clone()]);
    hooks::stall_reads(
        file.clone(),
        [at_210(0..1, Stall::For(Duration::from_secs(6)))],
    );
    let fallbacks = software_fallbacks();
    let done = export_film(&file, dir.path(), 11.0, 30).await;
    assert!(matches!(done.ended, Ok(Ended::Done)), "{:?}", done.ended);
    assert_eq!(software_fallbacks(), fallbacks, "no software fallback");
    assert_eq!(hooks::readers_made(&file).len(), 1, "no reader made again");
    let output = done.output.expect("the joined file");
    let (frames, _) = klipolo_engine::export::segment::probe(&output).unwrap();
    assert_eq!(frames, 330);
    shows(&output, &AROUND_THE_STALL);
}

/// A reader that never returns is let go (never cancelled from another thread): a new one is made at the keyframe
/// before the frame, the export finishes in hardware, and that is the one reader made again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh, a GPU and VideoToolbox"]
async fn export_reopens_a_stalled_reader_at_the_keyframe_before() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let file = film(dir.path(), "stalled reader.mp4");
    let _hooked = Hooked(vec![file.clone()]);
    hooks::stall_reads(file.clone(), [at_210(0..1, Stall::Forever)]);
    let fallbacks = software_fallbacks();
    let done = export_film(&file, dir.path(), 11.0, 30).await;
    assert!(matches!(done.ended, Ok(Ended::Done)), "{:?}", done.ended);
    assert_eq!(software_fallbacks(), fallbacks, "finished in hardware");
    let made = hooks::readers_made(&file);
    assert_eq!(made.len(), 2, "one reader made again: {made:?}");
    assert!(
        made[1].is_some_and(|first| (KEYFRAME..=STALL_AT).contains(&first)),
        "the new reader starts at the keyframe before frame 210: {made:?}"
    );
    shows(&done.output.expect("the joined file"), &AROUND_THE_STALL);
}

/// When the reader opened again stalls too, software decodes the frame, starting at the keyframe before it: not
/// frame 0 first, and not every frame from 0.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh, a GPU and VideoToolbox"]
async fn software_fallback_seeks_to_the_keyframe_not_from_zero() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let file = film(dir.path(), "every reader stalls.mp4");
    let _hooked = Hooked(vec![file.clone()]);
    hooks::stall_reads(file.clone(), [at_210(0..usize::MAX, Stall::Forever)]);
    // Recorded, never slowed: no frame of the file is at 600 s.
    hooks::slow_software(file.clone(), []);
    let fallbacks = software_fallbacks();
    let done = export_film(&file, dir.path(), 11.0, 30).await;
    assert!(matches!(done.ended, Ok(Ended::Done)), "{:?}", done.ended);
    assert_eq!(software_fallbacks(), fallbacks + 1);
    let decoded: Vec<f64> = hooks::software_decoded(&file)
        .into_iter()
        .filter(|(decoder, _)| *decoder == 0)
        .map(|(_, s)| s)
        .collect();
    let half = 0.5 / FILM_FPS;
    assert!(
        decoded
            .first()
            .is_some_and(|s| *s >= seconds(KEYFRAME) - half),
        "the software decoder's first frame is the keyframe before 210: {:?}",
        &decoded[..decoded.len().min(4)]
    );
    let before = decoded
        .iter()
        .filter(|s| **s < seconds(STALL_AT) - half)
        .count();
    assert!(
        before <= (STALL_AT - KEYFRAME) as usize + 2,
        "{before} frames decoded before frame 210"
    );
    shows(&done.output.expect("the joined file"), &AROUND_THE_STALL);
}

/// Nothing progresses (every reader and every software decoder stops): the frame fails, naming the file, within
/// its recovery steps' windows (shortened here), as a retryable decode error, with the finished segments kept.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh, a GPU and VideoToolbox"]
async fn export_fails_named_when_nothing_progresses() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let file = film(dir.path(), "nothing moves.mp4");
    let _hooked = Hooked(vec![file.clone()]);
    hooks::set_export_wait(Some(DecodeWait::Progress {
        stall_after: Duration::from_millis(1_500),
        cap: Duration::from_secs(8),
    }));
    hooks::stall_reads(file.clone(), [at_210(0..usize::MAX, Stall::Forever)]);
    hooks::slow_software(
        file.clone(),
        [SlowSoftware {
            from_s: 0.0,
            ..stops_at_210(usize::MAX)
        }],
    );
    let done = export_film(&file, dir.path(), 11.0, 2).await;
    let error = done.ended.expect_err("nothing can read frame 210");
    assert_eq!(error.code, ErrorCode::Decode);
    assert!(error.retryable);
    assert_eq!(
        error.message,
        "This file couldn't be read: nothing moves.mp4"
    );
    let last_frame = done
        .lines
        .iter()
        .rev()
        .find(|(_, line)| matches!(line, WorkerLine::Progress { .. }))
        .map(|(at, _)| *at)
        .expect("frames before the stall");
    let gave_up = done.ended_at - last_frame;
    // The software start keeps its 30 s window and beats meanwhile.
    assert!(
        gave_up < Duration::from_secs(45),
        "windows of 1.5 s, the 30 s start window and the opens: {gave_up:?}"
    );
    // 2 s segments: frames 0–239 were finished before frame 263 stalled, and stay.
    assert_eq!(done.job.segments_done(), 4);
}

/// While a frame waits on a decoder that is still making progress (software decoding slowly), the worker says so
/// every few seconds, so the parent's watchdog (120 s of silence) never stops a worker that is getting there.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs scripts/gen-engine-fixtures.sh, a GPU and VideoToolbox"]
async fn the_heartbeat_keeps_the_parent_hearing_while_the_decoder_progresses() {
    let _turn = TURN.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let file = film(dir.path(), "slow software.mp4");
    let _hooked = Hooked(vec![file.clone()]);
    hooks::set_export_wait(Some(DecodeWait::Progress {
        stall_after: Duration::from_secs(3),
        cap: Duration::from_secs(45),
    }));
    hooks::stall_reads(file.clone(), [at_210(0..usize::MAX, Stall::Forever)]);
    // The fallback decodes its first 24 frames (200–223) at 0.9 s each: about 10 s for frame 210, then 0.9 s for
    // each frame after it.
    hooks::slow_software(
        file.clone(),
        [SlowSoftware {
            decoders: 0..1,
            from_s: 0.0,
            per_frame: Duration::from_millis(900),
            frames: 24,
        }],
    );
    let done = export_film(&file, dir.path(), 11.0, 30).await;
    assert!(matches!(done.ended, Ok(Ended::Done)), "{:?}", done.ended);
    let beats: Vec<Instant> = done
        .lines
        .iter()
        .filter(|(_, line)| matches!(line, WorkerLine::WaitingForDecoder))
        .map(|(at, _)| *at)
        .collect();
    assert!(beats.len() >= 2, "{} heartbeats", beats.len());
    let next_frame = done
        .lines
        .iter()
        .find(|(at, line)| {
            *at > beats[beats.len() - 1] && matches!(line, WorkerLine::Progress { .. })
        })
        .map(|(at, _)| *at)
        .expect("frames after the wait");
    let gaps = beats
        .windows(2)
        .map(|w| w[1] - w[0])
        .chain([next_frame - beats[beats.len() - 1]]);
    for gap in gaps {
        assert!(gap < Duration::from_millis(6_500), "quiet for {gap:?}");
    }
}

/// The trial smoke (run by hand): writes an export job of the film's first 11 s in `KLIPOLO_SMOKE_DIR`
/// and prints its manifest, for the trial app's worker (`klipolo --export-worker <job.json>`, with
/// `KLIPOLO_TRIAL_STALL_READS=<the film's path>|210|forever|1`).
#[test]
#[ignore = "smoke: run by hand"]
fn smoke_job() {
    let Some(dir) = std::env::var_os("KLIPOLO_SMOKE_DIR").map(std::path::PathBuf::from) else {
        eprintln!("skipped: set KLIPOLO_SMOKE_DIR");
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let file = film(&dir, "smoke film.mp4");
    let job = film_job(&file, &dir, 11.0, 30);
    println!("SMOKE film={file} job={}", job.manifest_path().display());
}
