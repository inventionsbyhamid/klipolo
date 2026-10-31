#![cfg(feature = "synthetic")]
use klipolo_capture::io::{Fault, IoOp, LoggedIo};
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::session::SessionDir;
use klipolo_capture::writer::*;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn session() -> (tempfile::TempDir, SessionDir) {
    let root = tempfile::tempdir().unwrap();
    let dir = SessionDir::create(root.path(), "take").unwrap();
    (root, dir)
}

#[test]
fn a_tick_pushes_each_written_file_then_appends_durable_then_one_barrier() {
    let (_root, dir) = session();
    let io = Arc::new(LoggedIo::new());
    let w = DurableWriter::spawn(io.clone(), &dir).unwrap();
    w.open("screen/run-01.mp4", b"head".to_vec()).unwrap();
    w.open("mic/run-01.caf", b"caff".to_vec()).unwrap();
    w.append("screen/run-01.mp4", vec![1; 1_000], 500_000)
        .unwrap();
    w.append("mic/run-01.caf", vec![2; 300], 100_000).unwrap();
    w.tick().unwrap();
    w.append("mic/run-01.caf", vec![3; 300], 100_000).unwrap();
    w.tick().unwrap();
    let snap = w.finish().unwrap();
    let ops = io.ops();
    let pushes_then_barrier: Vec<String> = ops
        .iter()
        .filter_map(|op| match op {
            IoOp::Push(p) => Some(format!("push {}", p.file_name().unwrap().to_string_lossy())),
            IoOp::Barrier(p) => Some(format!(
                "barrier {}",
                p.file_name().unwrap().to_string_lossy()
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        pushes_then_barrier,
        [
            "push run-01.caf",
            "push run-01.mp4",
            "push journal.jsonl",
            "barrier journal.jsonl", // by key: mic/ before screen/
            "push run-01.caf",
            "push journal.jsonl",
            "barrier journal.jsonl", // the screen got nothing new
            "push journal.jsonl",
            "barrier journal.jsonl", // finish's last tick
        ]
    );
    assert_eq!(snap.durable["mic/run-01.caf"], 4 + 600);
    // The Durable line comes after a barrier that orders the media pushes ahead of it (macOS's F_BARRIERFSYNC), so
    // after a power cut it never vouches for media the drive hadn't stored.
    let name = |p: &std::path::Path| p.file_name().unwrap().to_string_lossy().into_owned();
    let first_tick: Vec<String> = ops
        .iter()
        .filter_map(|op| match op {
            IoOp::Push(p) => Some(format!("push {}", name(p))),
            IoOp::Order(p) => Some(format!("order {}", name(p))),
            IoOp::Append(p, _, b) if b.starts_with(br#"{"t":"durable""#) => {
                Some(format!("durable {}", name(p)))
            }
            IoOp::Barrier(p) => Some(format!("barrier {}", name(p))),
            _ => None,
        })
        .take(6)
        .collect();
    assert_eq!(
        first_tick,
        [
            "push run-01.caf",
            "push run-01.mp4",
            "order journal.jsonl",
            "durable journal.jsonl",
            "push journal.jsonl",
            "barrier journal.jsonl",
        ]
    );
}

#[test]
fn a_slow_drive_reports_its_queue_in_media_time_and_never_blocks_the_caller() {
    let (_root, dir) = session();
    let w = DurableWriter::spawn(
        Arc::new(LoggedIo::with_fault(Fault::Slow(Duration::from_millis(
            200,
        )))),
        &dir,
    )
    .unwrap();
    w.open("screen/run-01.mp4", vec![]).unwrap();
    let started = Instant::now();
    for _ in 0..30 {
        w.append("screen/run-01.mp4", vec![0; 10_000], 500_000)
            .unwrap();
    }
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "appends never wait for the disk"
    );
    assert!(
        w.snapshot().queued_ms >= WARN_QUEUED_MS,
        "{}",
        w.snapshot().queued_ms
    );
}

#[test]
fn enospc_mid_fragment_keeps_every_earlier_byte() {
    let (_root, dir) = session();
    let w = DurableWriter::spawn(
        Arc::new(LoggedIo::with_fault(Fault::Full {
            at_append: 6,
            partial: 100,
        })),
        &dir,
    )
    .unwrap();
    w.open("screen/run-01.mp4", b"head".to_vec()).unwrap();
    for i in 0..8u8 {
        let _ = w.append("screen/run-01.mp4", vec![i; 1_000], 500_000);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(matches!(w.snapshot().failed, Some(DiskFailure::Full)));
    assert_eq!(
        w.append("screen/run-01.mp4", vec![9; 10], 1),
        Err(WriterError::Gone)
    );
    let bytes = std::fs::read(dir.path("screen/run-01.mp4")).unwrap();
    assert_eq!(
        bytes.len(),
        4 + 4 * 1_000 + 100,
        "four whole appends and the part of the fifth the disk took"
    );
    assert!(bytes[4..1_004].iter().all(|b| *b == 0) && bytes[3_004..4_004].iter().all(|b| *b == 3));
    assert!(bytes[4_004..].iter().all(|b| *b == 4));
}

#[test]
fn an_io_error_is_retried_three_times_then_fails() {
    let (_root, dir) = session();
    let w = DurableWriter::spawn(
        Arc::new(LoggedIo::with_fault(Fault::Eio {
            at_append: 1,
            times: 2,
        })),
        &dir,
    )
    .unwrap();
    w.open("mic/run-01.caf", vec![]).unwrap();
    w.append("mic/run-01.caf", vec![1; 10], 10_000).unwrap();
    w.append("mic/run-01.caf", vec![2; 10], 10_000).unwrap();
    let snap = w.finish().unwrap();
    assert!(snap.failed.is_none(), "two failures heal on retry");
    // Each failed try left half its bytes behind; the retry cut them off first, so nothing is doubled.
    assert_eq!(
        std::fs::read(dir.path("mic/run-01.caf")).unwrap(),
        [[1; 10], [2; 10]].concat()
    );
    let (_root2, dir2) = session();
    let w = DurableWriter::spawn(
        Arc::new(LoggedIo::with_fault(Fault::Eio {
            at_append: 1,
            times: 5,
        })),
        &dir2,
    )
    .unwrap();
    w.open("mic/run-01.caf", vec![]).unwrap();
    w.append("mic/run-01.caf", vec![1; 10], 10_000).unwrap();
    assert!(matches!(w.finish().unwrap().failed, Some(DiskFailure::Io)));
}

/// An append the queue couldn't take loses that run's next frames' references, so the run takes
/// nothing more (the core ends it and starts a new one at a keyframe), and its file no longer matches the muxer's own
/// index: Stop walks it instead of using `remux_plan`.
#[test]
fn a_run_that_lost_an_append_takes_nothing_more_and_stop_knows_to_walk_it() {
    let (_root, dir) = session();
    let w = DurableWriter::spawn(
        Arc::new(LoggedIo::with_fault(Fault::Slow(Duration::from_millis(1)))),
        &dir,
    )
    .unwrap();
    w.open("screen/run-01.mp4", b"head".to_vec()).unwrap();
    w.open("mic/run-01.caf", b"caff".to_vec()).unwrap();
    w.append("mic/run-01.caf", vec![1; 6], 1_000).unwrap();
    let mut sent = 0_u64;
    let lost = loop {
        match w.append("screen/run-01.mp4", vec![7; 10], 1_000) {
            Ok(()) => sent += 1,
            Err(e) => break e,
        }
    };
    assert_eq!(lost, WriterError::Full);
    std::thread::sleep(Duration::from_millis(150)); // the queue has room again
    assert_eq!(
        w.append("screen/run-01.mp4", vec![8; 10], 1_000),
        Err(WriterError::Full),
        "never an append after a lost one"
    );
    let snap = w.finish().unwrap();
    assert!(snap.failed.is_none());
    assert!(snap.wrote_exactly("mic/run-01.caf", 10));
    assert!(snap.wrote_exactly("screen/run-01.mp4", 4 + 10 * sent));
    assert!(
        !snap.wrote_exactly("screen/run-01.mp4", 4 + 10 * (sent + 2)),
        "the muxer emitted two more fragments than the file holds"
    );
}

/// At Stop the drive may be far behind, the queue full: the Stop line still reaches the journal on the disk at once,
/// ahead of the backlog, and is never refused (a resume never records after the person's Stop).
#[test]
fn the_stop_line_jumps_a_full_queue_and_is_on_the_disk_at_once() {
    let (_root, dir) = session();
    let w = DurableWriter::spawn(
        Arc::new(LoggedIo::with_fault(Fault::Slow(Duration::from_millis(5)))),
        &dir,
    )
    .unwrap();
    w.open("screen/run-01.mp4", b"head".to_vec()).unwrap();
    while w.append("screen/run-01.mp4", vec![7; 10], 1_000).is_ok() {}
    let filler = Entry::Heartbeat {
        at_us: 0,
        queued_ms: 0,
    };
    while w.journal(&filler).is_ok() {}
    let stop = Entry::StopRequested {
        at_us: 2_000_000,
        reason: None,
    };
    assert_eq!(w.journal(&stop), Ok(()), "never refused");
    let started = std::time::Instant::now();
    let on_disk = || {
        klipolo_capture::journal::read(&dir.journal()).is_ok_and(|j| {
            j.entries
                .iter()
                .any(|e| matches!(e, Entry::StopRequested { .. }))
        })
    };
    while !on_disk() {
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the Stop line waited behind the backlog (about 5 s of it)"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(w); // the backlog drains on its own thread
}

/// Stop gave up on a writer stuck in a failing write and appended its own Stop line: the retry the writer wakes into
/// never cuts that line off.
#[test]
fn an_abandoned_writer_never_cuts_the_stop_line_with_a_retry() {
    let (_root, dir) = session();
    let w = DurableWriter::spawn(
        Arc::new(LoggedIo::with_fault(Fault::Eio {
            at_append: 1,
            times: 2,
        })),
        &dir,
    )
    .unwrap();
    let heartbeat = Entry::Heartbeat {
        at_us: 0,
        queued_ms: 0,
    };
    w.journal(&heartbeat).unwrap();
    // The first try failed half-way: the writer now waits 50 ms before it cuts the file back and tries again.
    let started = Instant::now();
    while std::fs::metadata(dir.journal()).map_or(0, |m| m.len()) == 0 {
        assert!(started.elapsed() < Duration::from_secs(2), "no first try");
        std::thread::sleep(Duration::from_millis(1));
    }
    w.progress().abandon();
    let stop = Entry::StopRequested {
        at_us: 1_000_000,
        reason: None,
    };
    journal::open_append(&dir.journal())
        .unwrap()
        .write_all(&journal::line(&stop))
        .unwrap();
    std::thread::sleep(Duration::from_millis(500)); // past both retries
    let entries = journal::read(&dir.journal()).unwrap().entries;
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::StopRequested { .. })),
        "the Stop line was cut: {entries:?}"
    );
}

/// A resumed writer appends to the crashed one's journal and line files through
/// `journal::open_append`, so nothing it writes is glued to a torn last line.
#[test]
fn a_resumed_writer_appends_after_a_torn_line_and_keeps_its_line_files() {
    let (_root, dir) = session();
    let mut torn = std::fs::File::create(dir.journal()).unwrap();
    torn.write_all(&journal::line(&Entry::Gate { host_ns: 1 }))
        .unwrap();
    torn.write_all(br#"{"t":"durable","through":{"mic/ru"#)
        .unwrap();
    std::fs::write(
        dir.timing(),
        b"{\"source\":\"mic\",\"run\":1,\"sessionUs\":0,\"frame\":0}\n{\"sou",
    )
    .unwrap();
    let w = DurableWriter::spawn(Arc::new(LoggedIo::new()), &dir).unwrap();
    w.journal(&Entry::Resume {
        at_us: 5,
        host_ns: 9,
    })
    .unwrap();
    w.open_lines("timing.jsonl").unwrap();
    w.append(
        "timing.jsonl",
        b"{\"source\":\"mic\",\"run\":2,\"sessionUs\":7,\"frame\":0}\n".to_vec(),
        0,
    )
    .unwrap();
    w.finish().unwrap();
    let read = journal::read(&dir.journal()).unwrap();
    assert_eq!((read.torn, read.skipped), (false, 1));
    assert_eq!(
        read.entries[1],
        Entry::Resume {
            at_us: 5,
            host_ns: 9
        }
    );
    assert!(
        matches!(read.entries.last(), Some(Entry::Durable { through }) if through.contains_key("timing.jsonl"))
    );
    let anchors = journal::read_anchors(&dir.timing()).unwrap();
    assert_eq!(anchors.len(), 2, "run 1's anchor kept, run 2's added");
}

/// Owner or controller, on the Mac and quietly: the real cost of the tick with three 30 Mb/s streams.
#[test]
#[ignore]
fn tick_cost_with_three_30_mbps_streams() {
    let root =
        tempfile::tempdir_in(std::env::var("KLIPOLO_BENCH_DIR").unwrap_or_else(|_| ".".into()))
            .unwrap();
    let dir = SessionDir::create(root.path(), "bench").unwrap();
    let w = DurableWriter::spawn(Arc::new(klipolo_capture::io::OsIo), &dir).unwrap();
    for s in ["a", "b", "c"] {
        w.open(&format!("{s}/run-01.mp4"), vec![]).unwrap();
    }
    for _ in 0..60 {
        for _ in 0..2 {
            for s in ["a", "b", "c"] {
                w.append(&format!("{s}/run-01.mp4"), vec![7; 1_875_000], 500_000)
                    .unwrap();
            }
        }
        std::thread::sleep(Duration::from_millis(1_000));
        w.tick().unwrap();
    }
    let snap = w.finish().unwrap();
    println!(
        "tick p99 {} ms, last {} ms",
        snap.tick_p99_ms, snap.last_tick_ms
    );
    assert!(snap.tick_p99_ms <= 200);
}
