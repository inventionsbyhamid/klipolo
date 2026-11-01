// No test in this file may spawn a process: `flock` is per open file description, and a forked child holds a copy of
// every descriptor until it execs, so the lock test could see its released lock still held (klipolo-platform's SPAWN).
mod common;
use common::config::config_with;
use klipolo_capture::drift::Anchor;
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::manifest;
use klipolo_capture::media::{Codec, StreamInfo};
use klipolo_capture::session::*;
use klipolo_model::project::{GameInfo, MarkerKind, RecordingKind, SourceRole};
use std::collections::BTreeMap;
use std::io::Write;

#[test]
fn entries_round_trip_and_a_torn_last_line_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(&journal::line(&Entry::Gate { host_ns: 5 }))
        .unwrap();
    f.write_all(&journal::line(&Entry::Marker {
        id: "m1".into(),
        at_us: 1_500_000,
        kind: MarkerKind::Hotkey,
    }))
    .unwrap();
    f.write_all(br#"{"t":"pause","atUs":20"#).unwrap(); // the power went here
    let read = journal::read(&path).unwrap();
    assert_eq!(read.entries.len(), 2);
    assert!(read.torn);
}

#[test]
fn a_held_lock_is_seen_by_a_second_opener() {
    let root = tempfile::tempdir().unwrap();
    let dir = SessionDir::create(root.path(), "take").unwrap();
    let held = SessionLock::try_acquire(&dir).unwrap();
    assert!(held.is_some());
    assert!(SessionLock::try_acquire(&dir).unwrap().is_none());
    drop(held);
    assert!(SessionLock::try_acquire(&dir).unwrap().is_some());
}

#[test]
fn a_session_folder_is_never_reused() {
    let root = tempfile::tempdir().unwrap();
    SessionDir::create(root.path(), "take").unwrap();
    assert_eq!(
        SessionDir::create(root.path(), "take").unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(SessionDir::run_key("mic", 2, false), "mic/run-02.caf");
    // A key read back from the journal never reaches outside the take.
    let dir = SessionDir::at(&root.path().join("take.klipolorec"));
    assert_eq!(
        dir.key_path("mic/run-02.caf").unwrap(),
        dir.root.join("mic").join("run-02.caf")
    );
    for bad in [
        "../x.mp4",
        "/etc/x",
        "mic//x.caf",
        "C:x.mp4",
        "mic/./x.caf",
        "",
    ] {
        assert_eq!(
            dir.key_path(bad).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput,
            "{bad}"
        );
    }
}

#[test]
fn the_manifest_comes_from_the_journal() {
    let cfg = config_with(&[
        ("screen", SourceRole::Screen),
        ("cam", SourceRole::Camera),
        ("mic", SourceRole::Mic),
    ]);
    let entries = vec![
        Entry::Gate { host_ns: 0 },
        Entry::RunStarted {
            source: "screen".into(),
            run: 1,
            file: "screen/run-01.mp4".into(),
            offset_us: 0,
            video: None,
            audio: None,
        },
        Entry::RunStarted {
            source: "cam".into(),
            run: 1,
            file: "cam/run-01.mp4".into(),
            offset_us: 33_000,
            video: None,
            audio: None,
        },
        Entry::RunStarted {
            source: "mic".into(),
            run: 1,
            file: "mic/run-01.caf".into(),
            offset_us: 0,
            video: None,
            audio: None,
        },
        Entry::RunEnded {
            source: "cam".into(),
            run: 1,
            end_us: 10_000_000,
        },
        Entry::RunStarted {
            source: "cam".into(),
            run: 2,
            file: "cam/run-02.mp4".into(),
            offset_us: 12_000_000,
            video: None,
            audio: None,
        },
        Entry::Marker {
            id: "m1".into(),
            at_us: 5_000_000,
            kind: MarkerKind::Hotkey,
        },
        Entry::Finalized {
            duration_us: 30_000_000,
        },
    ];
    let read = journal::JournalRead {
        entries,
        torn: false,
        skipped: 0,
    };
    let durations: BTreeMap<String, i64> = [
        ("screen/run-01.mp4", 30_000_000),
        ("cam/run-01.mp4", 9_967_000),
        ("cam/run-02.mp4", 18_000_000),
        ("mic/run-01.caf", 30_000_000),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let m = manifest::derive(&cfg, &read, &BTreeMap::new(), &durations);
    assert_eq!(m.duration_us, 30_000_000);
    let cam = m.sources.iter().find(|s| s.id == "cam").unwrap();
    assert_eq!(
        cam.runs
            .iter()
            .map(|r| (r.file.as_str(), r.offset_us))
            .collect::<Vec<_>>(),
        [("cam/run-01.mp4", 33_000), ("cam/run-02.mp4", 12_000_000)]
    );
    let mic = m.sources.iter().find(|s| s.id == "mic").unwrap();
    assert!(
        mic.runs[0].approximate,
        "no anchors: the first sample's time, no drift"
    );
    assert_eq!(m.markers.len(), 1);
    assert_eq!(m.kind, RecordingKind::Screen);
}

/// `Pause` and `Resume` keep their host times, so a resumed worker rebuilds the session clock with
/// the real pause length. And an entry appended after a torn line lands on its own line, never lost.
#[test]
fn pauses_rebuild_the_clock_and_an_append_after_a_torn_line_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let info = StreamInfo {
        codec: Codec::Hevc,
        config: vec![0x01, 0xab, 0x00],
        width: 1920,
        height: 1080,
    };
    let entries = [
        Entry::Gate {
            host_ns: 1_000_000_000,
        },
        Entry::RunStarted {
            source: "screen".into(),
            run: 1,
            file: "screen/run-01.mp4".into(),
            offset_us: 0,
            video: Some(info.clone()),
            audio: None,
        },
        Entry::Pause {
            at_us: 2_000_000,
            host_ns: 3_000_000_000,
        },
        Entry::Resume {
            at_us: 2_000_000,
            host_ns: 8_000_000_000,
        },
        Entry::Durable {
            through: BTreeMap::from([("screen/run-01.mp4".into(), 4_096)]),
        },
        Entry::Pause {
            at_us: 4_000_000,
            host_ns: 10_000_000_000,
        },
    ];
    let mut f = journal::open_append(&path).unwrap();
    for e in &entries {
        f.write_all(&journal::line(e)).unwrap();
    }
    assert!(
        String::from_utf8(journal::line(&entries[1]))
            .unwrap()
            .contains(r#""config":"01ab00""#)
    );
    f.write_all(br#"{"t":"durable","through":{"screen/run-"#)
        .unwrap(); // a kill here
    drop(f);

    let mut f = journal::open_append(&path).unwrap();
    let later = Entry::Durable {
        through: BTreeMap::from([("screen/run-01.mp4".into(), 8_192)]),
    };
    f.write_all(&journal::line(&later)).unwrap();
    let read = journal::read(&path).unwrap();
    assert_eq!(read.entries[..entries.len()], entries);
    assert_eq!(read.entries.last(), Some(&later));
    assert_eq!((read.torn, read.skipped), (false, 1));
    assert_eq!(read.durable_through()["screen/run-01.mp4"], 8_192);

    let clock = read.clock();
    assert!(clock.paused(), "a trailing pause stays paused");
    assert_eq!(
        clock.session_us(9_000_000_000),
        Some(3_000_000),
        "5 s paused left out"
    );
    assert_eq!(clock.session_us(5_000_000_000), None, "inside the pause");
}

#[test]
fn the_config_and_the_anchors_read_back() {
    let root = tempfile::tempdir().unwrap();
    let dir = SessionDir::create(root.path(), "take").unwrap();
    let mut cfg = config_with(&[
        ("screen", SourceRole::Game),
        ("gameSound", SourceRole::GameAudio),
        ("cam", SourceRole::Camera),
        ("mic", SourceRole::Mic),
    ]);
    cfg.sources[1].spec = SourceSpec::AppSound {
        of: "screen".into(),
    };
    cfg.kind = RecordingKind::Gameplay;
    cfg.auto_pause = Some("com.example.game".into());
    cfg.game = Some(GameInfo {
        app_id: "com.example.game".into(),
        name: "Example".into(),
        profile_id: None,
    });
    cfg.target = ImportTarget::Project { id: "p1".into() };
    write_config(&dir, &cfg).unwrap();
    assert_eq!(read_config(&dir).unwrap(), cfg);

    let a = |session_us, frame| Anchor { session_us, frame };
    let mut bytes = journal::anchor_line("mic", 1, &a(10, 0));
    bytes.extend(journal::anchor_line("gameSound", 1, &a(20, 48)));
    bytes.extend(journal::anchor_line("mic", 1, &a(1_000_010, 48_000)));
    bytes.extend(br#"{"source":"mic","run":1,"sess"#);
    std::fs::write(dir.timing(), bytes).unwrap();
    let anchors = journal::read_anchors(&dir.timing()).unwrap();
    assert_eq!(
        anchors[&("mic".to_string(), 1)],
        [a(10, 0), a(1_000_010, 48_000)]
    );
    assert_eq!(anchors[&("gameSound".to_string(), 1)], [a(20, 48)]);
}
