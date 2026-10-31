//! Recovery's cursor: the cursor file is trusted to its durable length, then to its last whole
//! line that reads (what is cut and isn't zeros kept beside it), and a shape's PNG is whole or absent: a missing one
//! isn't damage, its definition stays and the engine draws the Arrow look for it.
#![cfg(feature = "synthetic")]

mod common;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use klipolo_capture::import::{manifest_peaks, recording_from};
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::recover::repair;
use klipolo_capture::session::CURSOR;
use klipolo_cursor::{CursorTrack, Limits};
use klipolo_model::ids::new_id;
use klipolo_model::project::{Asset, AssetKind};

#[test]
fn recovery_trusts_the_cursor_file_to_its_durable_length_and_a_missing_png_is_kept_as_missing() {
    // A crashed take's media (the fixture's real H.264 and a mic, as the core writes them), and the cursor file and
    // shapes of a take with two shapes.
    let root = tempfile::tempdir().unwrap();
    let dir = common::take::write_take(root.path(), "take", common::take::Take::default()).dir;
    let shapes = common::harness::cursor::take_with_shapes(2);
    let mut cursor = shapes[CURSOR].clone();
    let durable = cursor.len() as u64;
    // The last tick vouched for the cursor file as it is; after it, one whole line, then a power cut's garbage and a
    // line that came after it.
    let through = BTreeMap::from([(CURSOR.to_string(), durable)]);
    let mut journal = journal::open_append(&dir.journal()).unwrap();
    journal
        .write_all(&journal::line(&Entry::Durable { through }))
        .unwrap();
    drop(journal);
    let whole = b"{\"k\":\"m\",\"t\":2900000,\"x\":0.6,\"y\":0.5}\n";
    cursor.extend_from_slice(whole);
    cursor.extend_from_slice(
        b"\0\0\0{\"k\":\"m\",\"t\":19\n{\"k\":\"m\",\"t\":2950000,\"x\":0.7,\"y\":0.5}\n",
    );
    std::fs::write(dir.cursor(), &cursor).unwrap();
    // `cursors/1.png` never landed (its rename didn't happen): only shape 0's file is there.
    std::fs::create_dir_all(dir.path("cursors")).unwrap();
    std::fs::write(dir.path("cursors/0.png"), &shapes["cursors/0.png"]).unwrap();
    assert!(shapes.contains_key("cursors/1.png"));

    let m = repair(&dir).unwrap().manifest;
    let kept = std::fs::read(dir.cursor()).unwrap();
    assert_eq!(
        kept.len() as u64,
        durable + whole.len() as u64,
        "cut after the last whole line"
    );
    assert!(
        dir.path("cursor.jsonl.damaged").is_file(),
        "the cut bytes kept beside it"
    );
    let track = CursorTrack::read(&kept[..], Limits::for_file_len(kept.len() as u64));
    assert_eq!(
        track.shapes().iter().map(|s| s.id).collect::<Vec<_>>(),
        [0, 1],
        "def 1 stays"
    );
    assert!(!dir.path("cursors/1.png").exists());
    assert_eq!(track.notes().headers, 1);
    assert!(!track.notes().lost_any(), "{:?}", track.notes());

    let mut probe = |path: &Path, name: &str| -> Result<Asset, String> {
        Ok(Asset {
            id: new_id(),
            kind: AssetKind::Video,
            path: path.to_string_lossy().into_owned(),
            name: name.into(),
            duration_us: 2_000_000,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: None,
            has_audio: false,
            audio_lead_us: None,
            made_with: None,
        })
    };
    let (imported, _) = recording_from(&m, &dir, &manifest_peaks, &mut probe).unwrap();
    let cursor = imported
        .recording
        .cursor
        .expect("the take imports with its cursor");
    assert_eq!(Path::new(&cursor.path), dir.cursor());
    // A second repair changes nothing (idempotent).
    repair(&dir).unwrap();
    assert_eq!(std::fs::read(dir.cursor()).unwrap(), kept);
}

#[test]
fn a_cursor_file_cut_mid_line_at_its_end_keeps_its_whole_lines() {
    let root = tempfile::tempdir().unwrap();
    let dir = common::take::write_take(root.path(), "take", common::take::Take::default()).dir;
    let mut cursor = b"{\"k\":\"h\",\"v\":2,\"capture\":\"burned\",\"clicks\":\"sampled\",\"w\":1440.0,\"h\":900.0}\n".to_vec();
    cursor.extend_from_slice(b"{\"k\":\"m\",\"t\":100000,\"x\":0.5,\"y\":0.5}\n");
    let whole = cursor.len() as u64;
    // A kill in the middle of an append: the last line has no newline (no Durable line names the file at all).
    cursor.extend_from_slice(b"{\"k\":\"m\",\"t\":2000");
    std::fs::write(dir.cursor(), &cursor).unwrap();
    repair(&dir).unwrap();
    assert_eq!(std::fs::metadata(dir.cursor()).unwrap().len(), whole);
    assert!(dir.path("cursor.jsonl.damaged").is_file());
}
