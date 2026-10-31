#![cfg(feature = "synthetic")]
//! Recovery's rules beyond `tests/recover.rs`: one test per rule or choice.
// No test in this file may spawn a process: `flock` is per open file description, and a forked child holds a copy of
// every descriptor until it execs, so a released lock could still look held (klipolo-platform's SPAWN).
mod common;
use common::take::{Take, write_take};
use klipolo_capture::recover::{RecoverError, repair};
use klipolo_capture::session::{SessionDir, SessionLock};

/// A kill loses at most what never reached `write()` (≤ 0.7 s of video): whole fragments past the last `Durable` mark
/// are kept. But a power cut can land a fragment's box headers without its media, which the structural walk accepts
/// past the mark, a fragment counts only if its samples are whole NAL units. The cut bytes that aren't
/// zeros are kept beside the file, once, however often repair runs.
#[test]
fn past_the_durable_mark_whole_fragments_count_and_zeroed_media_does_not() {
    use klipolo_capture::fmp4::read;
    use klipolo_platform::file::PositionedIo;
    let root = tempfile::tempdir().unwrap();
    let t = write_take(
        root.path(),
        "d",
        Take {
            seconds: 4.0,
            ..Take::default()
        },
    );
    // The journal as a kill after the first tick leaves it: one Durable line (it vouches for the first fragment).
    let text = std::fs::read_to_string(t.dir.journal()).unwrap();
    let mut seen = false;
    let kept: String = text
        .split_inclusive('\n')
        .filter(|l| {
            let durable = l.starts_with(r#"{"t":"durable""#);
            let keep = !durable || !seen;
            seen |= durable;
            keep
        })
        .collect();
    std::fs::write(t.dir.journal(), kept).unwrap();
    // The last fragment's media never landed; its moof and mdat headers did.
    let path = t.dir.path("screen/run-01.mp4");
    let original = std::fs::read(&path).unwrap();
    let file = common::take::rw(&path);
    let walk = read::walk(&mut &file, original.len() as u64).unwrap();
    let last = walk.index.fragments.last().unwrap().clone();
    let zeros = vec![0; (last.data_end() - last.data_at) as usize];
    file.write_all_at(&zeros, last.data_at).unwrap();

    let r = repair(&t.dir).unwrap();
    let run =
        |id: &str| r.manifest.sources.iter().find(|s| s.id == id).unwrap().runs[0].duration_us;
    assert_eq!(
        run("screen"),
        3_500_000,
        "7 whole fragments, 6 past the mark"
    );
    assert_eq!(common::decode::decode_all(&path).0, 105);
    assert_eq!(run("mic"), 4_000_000, "sound past the mark is kept too");
    let damaged = t.dir.path("screen/run-01.mp4.damaged");
    let saved = (last.moof_at as usize, last.data_at as usize);
    assert_eq!(std::fs::read(&damaged).unwrap(), original[saved.0..saved.1]);
    assert_eq!(r.damaged_bytes, (saved.1 - saved.0) as u64);

    let again = repair(&t.dir).unwrap();
    assert_eq!((again.manifest, again.damaged_bytes), (r.manifest, 0));
    let names: Vec<_> = std::fs::read_dir(t.dir.path("screen"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 2, "the run and one .damaged: {names:?}");
}

/// An all-zero CAF header (the power went before its first flush landed) is rebuilt from the
/// journal's audio format, and an all-zero tail after the media (a remux tail that never landed) is cut without being
/// kept as damage.
#[test]
fn a_zero_sound_header_is_rebuilt_and_a_zero_tail_is_not_damage() {
    use klipolo_platform::file::PositionedIo;
    let root = tempfile::tempdir().unwrap();
    let t = write_take(root.path(), "z", Take::default());
    let mic = t.dir.path("mic/run-01.caf");
    common::take::rw(&mic).write_all_at(&[0; 68], 0).unwrap();
    let screen = t.dir.path("screen/run-01.mp4");
    let len = std::fs::metadata(&screen).unwrap().len();
    common::take::rw(&screen)
        .write_all_at(&[0; 4096], len)
        .unwrap();
    let r = repair(&t.dir).unwrap();
    assert_eq!(r.damaged_bytes, 0);
    let run =
        |id: &str| r.manifest.sources.iter().find(|s| s.id == id).unwrap().runs[0].duration_us;
    assert_eq!(run("mic"), 2_000_000);
    assert_eq!(common::decode::decode_audio_frames(&mic), 96_000);
    assert_eq!(run("screen"), 2_000_000);
    assert_eq!(common::decode::decode_all(&screen).0, 60);
    for folder in ["screen", "mic"] {
        assert_eq!(
            std::fs::read_dir(t.dir.path(folder)).unwrap().count(),
            1,
            "no .damaged"
        );
    }
}

/// A run closed in an earlier tick isn't named by later `Durable` lines, so its durable length
/// comes from every line (`durable_through`). Here the mic's first run ends with half a second of exact zeros (a
/// muted moment) inside its durable length: real silence, which recovery keeps.
#[test]
fn a_run_closed_in_an_earlier_tick_keeps_its_whole_durable_length() {
    use klipolo_capture::journal::{self, Entry};
    use klipolo_capture::writer::DurableWriter;
    use klipolo_capture::{caf, io::OsIo, session};
    use klipolo_model::project::SourceRole;
    let root = tempfile::tempdir().unwrap();
    let dir = SessionDir::create(root.path(), "s").unwrap();
    let mut cfg = common::config::config_with(&[("mic", SourceRole::Mic)]);
    cfg.id = "s".into();
    session::write_config(&dir, &cfg).unwrap();
    let format = common::take::mic_format();
    let tone: Vec<f32> = (0..48_000)
        .map(|i| 0.25 * (i as f32 * 0.0576).sin())
        .collect();
    let started = |run: u32, offset_us: i64| Entry::RunStarted {
        source: "mic".into(),
        run,
        file: SessionDir::run_key("mic", run, false),
        offset_us,
        video: None,
        audio: Some(format),
    };
    let w = DurableWriter::spawn(std::sync::Arc::new(OsIo), &dir).unwrap();
    let (one, two) = (
        SessionDir::run_key("mic", 1, false),
        SessionDir::run_key("mic", 2, false),
    );
    w.open(&one, caf::header(format)).unwrap();
    w.journal(&started(1, 0)).unwrap();
    let mut pcm = Vec::new();
    caf::encode_into(&tone, &mut pcm);
    caf::encode_into(&[0.0; 24_000], &mut pcm);
    w.append(&one, pcm, 1_500_000).unwrap();
    w.journal(&Entry::RunEnded {
        source: "mic".into(),
        run: 1,
        end_us: 1_500_000,
    })
    .unwrap();
    w.close(&one).unwrap();
    w.tick().unwrap();
    w.open(&two, caf::header(format)).unwrap();
    w.journal(&started(2, 1_600_000)).unwrap();
    for _ in 0..3 {
        w.append(
            &two,
            caf::encode(&klipolo_capture::PcmBlock {
                host_ns: 0,
                samples: tone.clone(),
            }),
            1_000_000,
        )
        .unwrap();
        w.tick().unwrap();
    }
    w.finish().unwrap();
    let read = journal::read(&dir.journal()).unwrap();
    let last = read.entries.iter().rev().find_map(|e| match e {
        Entry::Durable { through } => Some(through),
        _ => None,
    });
    assert!(
        !last.unwrap().contains_key(&one),
        "later lines don't name the closed run"
    );

    let r = repair(&dir).unwrap();
    let runs = &r.manifest.sources[0].runs;
    assert_eq!(
        runs[0].duration_us, 1_500_000,
        "its silence is inside its durable length"
    );
    assert_eq!(runs[1].duration_us, 3_000_000);
}

/// Repair waits for a held lock: a take whose lock is held may be one whose recorder is just starting or resuming, so
/// repair retries for about half a second before it says the take is live.
#[test]
fn repair_waits_half_a_second_for_the_lock_then_says_live() {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    let t = write_take(root.path(), "l", Take::default());
    let held = SessionLock::try_acquire(&t.dir).unwrap().unwrap();
    let started = Instant::now();
    assert_eq!(repair(&t.dir).unwrap_err(), RecoverError::Live);
    assert!(started.elapsed() >= Duration::from_millis(450));
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(held);
    });
    assert!(repair(&t.dir).is_ok(), "a lock released while it waits");
    release.join().unwrap();
}

/// Recovery never destroys more than it keeps: a non-zero damaged tail over 64 MB (durable footage after a corrupt
/// fragment, not a power cut's last second) is never cut. The run stays as it is, out of the manifest, and named.
#[test]
fn a_damaged_tail_too_big_to_keep_is_never_cut() {
    use klipolo_platform::file::PositionedIo;
    let root = tempfile::tempdir().unwrap();
    let t = write_take(root.path(), "g", Take::default());
    let screen = t.dir.path("screen/run-01.mp4");
    let len = std::fs::metadata(&screen).unwrap().len();
    common::take::rw(&screen)
        .write_all_at(&vec![0xA5; 65 << 20], len)
        .unwrap();
    let r = repair(&t.dir).unwrap();
    assert!(r.manifest.sources.iter().all(|s| s.id != "screen"));
    assert!(r.manifest.sources.iter().any(|s| s.id == "mic"));
    assert_eq!(std::fs::metadata(&screen).unwrap().len(), len + (65 << 20));
    assert!(!t.dir.path("screen/run-01.mp4.damaged").exists());
    assert_eq!(
        kept(&r.manifest),
        ["Screen couldn't be repaired automatically. Its files are kept in the take's folder."]
    );
}

/// The person never loses part of a take silently. Without a journal, a screen run whose head is zeros
/// can't be rebuilt: it is left out, named in the notes (the whole source: its other run recorded nothing), and its
/// file is kept byte for byte. A run that holds nothing but its head recorded nothing, so it isn't named. A second
/// repair says the same.
#[test]
fn a_run_left_out_is_named_and_kept_and_an_empty_one_is_not() {
    use klipolo_capture::fmp4::Fmp4Writer;
    let root = tempfile::tempdir().unwrap();
    let t = write_take(root.path(), "n", Take::default());
    std::fs::remove_file(t.dir.journal()).unwrap();
    let screen = t.dir.path("screen/run-01.mp4");
    let mut bytes = std::fs::read(&screen).unwrap();
    bytes[..200].fill(0);
    std::fs::write(&screen, &bytes).unwrap();
    let head = Fmp4Writer::new(&common::take::fixture().info).head();
    std::fs::write(t.dir.path("screen/run-02.mp4"), &head).unwrap();

    let r = repair(&t.dir).unwrap();
    assert!(r.manifest.sources.iter().all(|s| s.id != "screen"));
    // Every screen run left out: the whole source is named, not "Part of" it.
    assert_eq!(
        kept(&r.manifest),
        ["Screen couldn't be repaired automatically. Its files are kept in the take's folder."]
    );
    assert!(r.manifest.recovered);
    assert_eq!(std::fs::read(&screen).unwrap(), bytes, "kept as it was");
    assert_eq!(
        std::fs::read(t.dir.path("screen/run-02.mp4")).unwrap(),
        head
    );
    assert_eq!(repair(&t.dir).unwrap().manifest, r.manifest);
}

/// The notes naming what was left out (`capture.recovered.partKept` and `sourceKept`), in English.
fn kept(m: &klipolo_capture::manifest::Manifest) -> Vec<String> {
    m.notes
        .iter()
        .filter(|n| n.code.ends_with("Kept"))
        .map(klipolo_capture::english::english)
        .collect()
}
