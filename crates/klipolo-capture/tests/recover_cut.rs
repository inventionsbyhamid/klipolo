#![cfg(feature = "synthetic")]
//! A power cut in the middle of repair itself.
// No test in this file may spawn a process: `flock` is per open file description, and a forked child holds a copy of
// every descriptor until it execs, so a released lock could still look held (klipolo-platform's SPAWN).
mod common;
use std::path::Path;

use common::take::{Take, write_take};
use klipolo_capture::fmp4::read;
use klipolo_capture::io::{IoOp, LoggedIo};
use klipolo_capture::recover::{repair, repair_with};
use klipolo_capture::session::SessionDir;

/// A take killed between two ticks (its last fragments and sound reached the page cache, not the disk) is repaired
/// through the logged store. Then the power is cut after each of repair's steps, with every write not yet pushed or
/// barriered (the recorder's and repair's) dropped, cut short or zeroed, and the take is repaired again. Every frame
/// it imports is the recorder's own bytes (never a zeroed or torn one), and every source keeps what was durable
/// before the first repair.
#[test]
fn a_power_cut_during_repair_never_imports_media_that_didnt_land() {
    let source = tempfile::tempdir().unwrap();
    let take = write_take(
        source.path(),
        "p",
        Take {
            seconds: 4.0,
            ..Take::default()
        },
    );
    let (root, ops) = (source.path(), take.io.ops());
    let kills: Vec<usize> = (1..ops.len())
        .filter(|&i| matches!(ops[i], IoOp::Push(_)) && !matches!(ops[i - 1], IoOp::Push(_)))
        .map(|i| i - 1)
        .collect();
    assert!(kills.len() >= 4, "a kill before each tick: {kills:?}");
    for &kill in &kills {
        let killed = tempfile::tempdir().unwrap();
        let recorded = &ops[..=kill];
        let durable = common::take::replay(recorded, kill, 0, root, killed.path());
        let dir = SessionDir::at(&killed.path().join("p.klipolorec"));
        let original = std::fs::read(dir.path("screen/run-01.mp4")).unwrap();
        let logged = LoggedIo::new();
        repair_with(&dir, &logged).unwrap();
        let steps = logged.ops();
        for cut in 0..steps.len() {
            for seed in 0..4 {
                let to = tempfile::tempdir().unwrap();
                let segments = [(recorded, root), (&steps[..=cut], killed.path())];
                common::replay::power_cut(&segments, seed, to.path());
                let again = SessionDir::at(&to.path().join("p.klipolorec"));
                let at = format!("kill {kill}, cut after {:?}, seed {seed}", steps[cut]);
                let r = repair(&again).unwrap_or_else(|e| panic!("{at}: {e:?}"));
                for (id, &before) in &durable {
                    let got: i64 = r
                        .manifest
                        .sources
                        .iter()
                        .filter(|s| &s.id == id)
                        .flat_map(|s| &s.runs)
                        .map(|run| run.duration_us)
                        .sum();
                    assert!(
                        got >= before,
                        "{at}: {id} lost durable media: {got} < {before}"
                    );
                }
                let screen = r.manifest.sources.iter().any(|s| s.id == "screen");
                if screen {
                    own_frames(&again.path("screen/run-01.mp4"), &original, &at);
                }
            }
        }
    }
}

/// Every sample the file's walk finds is byte for byte the recorder's, where the recorder wrote it.
fn own_frames(path: &Path, original: &[u8], at: &str) {
    let bytes = std::fs::read(path).unwrap();
    let walk = read::walk(&mut std::io::Cursor::new(&bytes), bytes.len() as u64).unwrap();
    assert!(walk.remuxed, "{at}: imported unremuxed");
    for f in &walk.index.fragments {
        let mut from = f.data_at as usize;
        for s in &f.samples {
            let to = from + s.size as usize;
            assert!(
                original.get(from..to) == Some(&bytes[from..to]),
                "{at}: a sample at {from} isn't the recorder's"
            );
            from = to;
        }
    }
}
