#![cfg(feature = "synthetic")]
// No test in this file may spawn a process: `flock` is per open file description, and a forked child holds a copy of
// every descriptor until it execs, so a released lock could still look held (klipolo-platform's SPAWN).
mod common;
use common::take::{Take, write_take};
use klipolo_capture::io::IoOp;
use klipolo_capture::recover::{FoundState, repair, scan};
use klipolo_capture::session::{SessionDir, SessionLock};

#[test]
fn scan_lists_live_interrupted_and_ready_takes() {
    let root = tempfile::tempdir().unwrap();
    let live = write_take(
        root.path(),
        "a",
        Take {
            seconds: 2.0,
            finalize: false,
            ..Take::default()
        },
    );
    let _held = SessionLock::try_acquire(&live.dir).unwrap().unwrap();
    write_take(
        root.path(),
        "b",
        Take {
            seconds: 2.0,
            finalize: false,
            ..Take::default()
        },
    );
    write_take(
        root.path(),
        "c",
        Take {
            seconds: 2.0,
            finalize: true,
            ..Take::default()
        },
    );
    let found = scan(root.path(), 500);
    let states: Vec<_> = found.iter().map(|f| (f.id.as_str(), f.state)).collect();
    assert!(states.contains(&("a", FoundState::Live)));
    assert!(states.contains(&("b", FoundState::Interrupted)));
    assert!(states.contains(&("c", FoundState::Ready)));
}

#[test]
fn a_killed_take_imports_every_complete_fragment() {
    let root = tempfile::tempdir().unwrap();
    let t = write_take(
        root.path(),
        "k",
        Take {
            seconds: 5.0,
            finalize: false,
            cut_last_fragment: true,
            ..Take::default()
        },
    );
    let r = repair(&t.dir).unwrap();
    let screen = &r
        .manifest
        .sources
        .iter()
        .find(|s| s.id == "screen")
        .unwrap()
        .runs[0];
    assert!(
        screen.duration_us >= 5_000_000 - 700_000,
        "{}",
        screen.duration_us
    );
    let mic = &r
        .manifest
        .sources
        .iter()
        .find(|s| s.id == "mic")
        .unwrap()
        .runs[0];
    assert!(mic.duration_us >= 5_000_000 - 200_000);
    assert_eq!(
        common::decode::decode_all(&t.dir.path("screen/run-01.mp4")).0 as i64,
        screen.duration_us * 30 / 1_000_000
    );
    assert!(r.manifest.recovered);
}

#[test]
fn a_damaged_head_is_rebuilt_from_the_journal() {
    let root = tempfile::tempdir().unwrap();
    let t = write_take(
        root.path(),
        "h",
        Take {
            seconds: 3.0,
            finalize: false,
            ..Take::default()
        },
    );
    let path = t.dir.path("screen/run-01.mp4");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[..200].fill(0);
    std::fs::write(&path, &bytes).unwrap();
    let r = repair(&t.dir).unwrap();
    assert!(r.manifest.sources[0].runs[0].duration_us >= 2_500_000);
    assert_eq!(common::decode::decode_all(&path).0, 90);
}

#[test]
fn no_journal_still_recovers_with_approximate_timing() {
    let root = tempfile::tempdir().unwrap();
    let t = write_take(
        root.path(),
        "j",
        Take {
            seconds: 3.0,
            finalize: false,
            ..Take::default()
        },
    );
    std::fs::remove_file(t.dir.journal()).unwrap();
    let r = repair(&t.dir).unwrap();
    assert!(
        r.manifest
            .sources
            .iter()
            .all(|s| s.runs.iter().all(|run| run.approximate))
    );
    assert!(
        r.manifest
            .notes
            .iter()
            .any(|n| n.code == "capture.recovered.approximate")
    );
}

#[test]
fn repair_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let t = write_take(
        root.path(),
        "i",
        Take {
            seconds: 2.0,
            finalize: false,
            ..Take::default()
        },
    );
    let first = repair(&t.dir).unwrap().manifest;
    let bytes = std::fs::read(t.dir.path("screen/run-01.mp4")).unwrap();
    assert_eq!(repair(&t.dir).unwrap().manifest, first);
    assert_eq!(
        std::fs::read(t.dir.path("screen/run-01.mp4")).unwrap(),
        bytes
    );
}

/// Power loss: replay the write log up to each full flush, then drop, cut short or zero the later,
/// unflushed writes (seeded). Recovery never panics, keeps everything that was durable, and never imports damage.
/// Extended: a take that Stop finished is cut inside Stop's own steps too (the
/// remux's tail and flips, the CAF's size, the journal's last lines), the cuts start before the first tick, and every
/// source the journal made durable is checked, so one missing from the manifest altogether fails.
#[test]
fn power_loss_replay_never_imports_damaged_bytes() {
    for finalize in [false, true] {
        let source = tempfile::tempdir().unwrap();
        let take = Take {
            seconds: 6.0,
            finalize,
            ..Take::default()
        };
        let t = write_take(source.path(), "p", take);
        let ops = t.io.ops();
        let first_media = ops
            .iter()
            .position(|o| matches!(o, IoOp::Append(p, ..) if p.extension().is_some_and(|e| e == "mp4" || e == "caf")))
            .unwrap();
        let mut cuts = vec![first_media];
        cuts.extend(
            ops.iter()
                .enumerate()
                .filter(|(_, o)| matches!(o, IoOp::Barrier(_)))
                .map(|(i, _)| i),
        );
        assert!(
            !finalize || cuts.len() > 8,
            "Stop's own barriers are cut too: {}",
            cuts.len()
        );
        for (round, &b) in cuts.iter().enumerate() {
            for variant in 0..3u64 {
                let dir = tempfile::tempdir().unwrap();
                let durable = common::take::replay(
                    &ops,
                    b,
                    variant * 1_000 + round as u64,
                    source.path(),
                    dir.path(),
                );
                let session = SessionDir::at(&dir.path().join("p.klipolorec"));
                let r = repair(&session).unwrap_or_else(|e| {
                    panic!("finalize {finalize} round {round} variant {variant}: {e:?}")
                });
                for (id, &want) in &durable {
                    let got: i64 = r
                        .manifest
                        .sources
                        .iter()
                        .filter(|s| &s.id == id)
                        .flat_map(|s| &s.runs)
                        .map(|run| run.duration_us)
                        .sum();
                    assert!(
                        got >= want,
                        "{id} lost durable media (finalize {finalize}, round {round}): {got} < {want}"
                    );
                }
                let screen = session.path("screen/run-01.mp4");
                let Some(run) = r
                    .manifest
                    .sources
                    .iter()
                    .find(|s| s.id == "screen")
                    .map(|s| &s.runs[0])
                else {
                    continue;
                };
                let frames = common::decode::decode_all(&screen).0 as i64;
                assert!(
                    (frames - run.duration_us * 30 / 1_000_000).abs() <= 1,
                    "every imported frame decodes"
                );
            }
        }
    }
}

#[test]
#[ignore]
fn repairing_an_hour_takes_under_ten_seconds() {
    let root = tempfile::tempdir().unwrap();
    let t = write_take(
        root.path(),
        "long",
        Take {
            seconds: 3_600.0,
            finalize: false,
            fast: true,
            ..Take::default()
        },
    );
    let r = repair(&t.dir).unwrap();
    println!("repaired an hour in {} ms", r.took_ms);
    assert!(r.took_ms < 10_000, "{} ms", r.took_ms);
}
