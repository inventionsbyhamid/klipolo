//! The power cut that `replay` recreates from a take's write log, for the power-loss replay test, and the
//! one `power_cut` recreates in the middle of a repair.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use klipolo_capture::caf;
use klipolo_capture::fmp4::read;
use klipolo_capture::io::IoOp;
use klipolo_capture::journal;
use klipolo_capture::session::{self, EXTENSION, SessionDir};
use klipolo_platform::file::PositionedIo;

/// Recreates the takes under `from` in `to` as a power cut would have left them: `ops[..=upto]` exactly, then each
/// later append dropped, cut short or written as zeros of its length, by `seed` (later creates are made; pushes,
/// barriers, truncations and positioned writes after `upto` never landed). Returns each source's durable media in µs: every file's
/// durable length from all the `Durable` lines at or before `upto` (a run closed earlier isn't named in later lines),
/// measured the way recovery measures (whole fragments, whole frames).
pub fn replay(
    ops: &[IoOp],
    upto: usize,
    seed: u64,
    from: &Path,
    to: &Path,
) -> BTreeMap<String, i64> {
    let map = |p: &Path| to.join(p.strip_prefix(from).unwrap());
    let mut takes = Vec::new();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == EXTENSION) {
            // session.json is written before capture, atomically, not through `Io`.
            std::fs::create_dir_all(map(&path)).unwrap();
            std::fs::copy(path.join("session.json"), map(&path).join("session.json")).unwrap();
            takes.push(SessionDir::at(&map(&path)));
        }
    }
    for op in &ops[..=upto] {
        apply(op, &map, None);
    }
    let mut durable = BTreeMap::new();
    for dir in &takes {
        durable_media(dir, &mut durable);
    }
    let mut rng = Rng(seed);
    for op in &ops[upto + 1..] {
        apply(op, &map, Some(&mut rng));
    }
    durable
}

/// Recreates in `to` a power cut after the last of `segments`' steps: each segment is a log of steps (the recorder's,
/// then a repair's) with the folder its paths are under. Per file (the conservative model, Linux's): a step landed
/// exactly when a later push or barrier of the same file comes before the cut; `order` alone makes nothing durable. A
/// later step, by `seed`: an append is dropped, cut short or written as zeros; a write over old bytes is dropped, lands,
/// or lands only up to the end of its first 512-byte sector (a disk writes whole sectors, so a remux flip straddling
/// one can tear; never zeros: its old bytes stay); a truncation never lands.
pub fn power_cut(segments: &[(&[IoOp], &Path)], seed: u64, to: &Path) {
    let (_, first) = segments[0];
    for entry in std::fs::read_dir(first).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == EXTENSION) {
            let into = to.join(path.strip_prefix(first).unwrap());
            std::fs::create_dir_all(&into).unwrap();
            std::fs::copy(path.join("session.json"), into.join("session.json")).unwrap();
        }
    }
    let steps: Vec<(&IoOp, &Path)> = segments
        .iter()
        .flat_map(|(ops, root)| ops.iter().map(move |op| (op, *root)))
        .collect();
    let target = |op: &IoOp, root: &Path| {
        let p = match op {
            IoOp::Create(p)
            | IoOp::OpenAppend(p)
            | IoOp::Append(p, ..)
            | IoOp::Write(p, ..)
            | IoOp::Truncate(p, _)
            | IoOp::Push(p)
            | IoOp::Order(p)
            | IoOp::Barrier(p) => p,
        };
        to.join(p.strip_prefix(root).unwrap())
    };
    // Walking back: a step is durable when its file is pushed or barriered after it.
    let mut synced = BTreeSet::new();
    let mut landed = vec![false; steps.len()];
    for (i, &(op, root)) in steps.iter().enumerate().rev() {
        let path = target(op, root);
        if matches!(op, IoOp::Push(_) | IoOp::Barrier(_)) {
            synced.insert(path.clone());
        }
        landed[i] = synced.contains(&path);
    }
    let mut rng = Rng(seed);
    for (i, &(op, root)) in steps.iter().enumerate() {
        let to_root = |p: &Path| to.join(p.strip_prefix(root).unwrap());
        match (op, landed[i]) {
            (IoOp::Write(p, at, bytes), landed) => {
                let n = match (landed, rng.next() % 3) {
                    (true, _) | (false, 1) => bytes.len(),
                    (false, 0) => 0,
                    _ => ((at + 1).next_multiple_of(512) - at).min(bytes.len() as u64) as usize,
                };
                let file = OpenOptions::new().write(true).open(to_root(p)).unwrap();
                file.write_all_at(&bytes[..n], *at).unwrap();
            }
            (op, true) => apply(op, &to_root, None),
            (op, false) => apply(op, &to_root, Some(&mut rng)),
        }
    }
}

fn durable_media(dir: &SessionDir, out: &mut BTreeMap<String, i64>) {
    for source in session::read_config(dir).unwrap().sources {
        out.entry(source.id).or_insert(0);
    }
    let through = journal::read(&dir.journal()).unwrap().durable_through();
    for (key, len) in through {
        let Some((source, _)) = key.split_once('/') else {
            continue; // timing.jsonl
        };
        let file = File::open(dir.path(&key)).unwrap();
        let len = len.min(file.metadata().unwrap().len());
        let us = if key.ends_with(".mp4") {
            let walk = read::walk(&mut &file, len).unwrap();
            walk.head.map_or(0, |h| walk.index.duration_us(h.timescale))
        } else {
            caf::scan(&mut &file, len).map_or(0, |s| {
                (s.frames * 1_000_000 / u64::from(s.format.rate)) as i64
            })
        };
        *out.entry(source.to_owned()).or_insert(0) += us;
    }
}

fn apply(op: &IoOp, map: &impl Fn(&Path) -> PathBuf, damage: Option<&mut Rng>) {
    let open = |p: &Path| {
        let path = map(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .unwrap()
    };
    match (op, damage) {
        (IoOp::Create(p) | IoOp::OpenAppend(p), _) => {
            open(p);
        }
        (IoOp::Append(p, at, bytes), None) => open(p).write_all_at(bytes, *at).unwrap(),
        (IoOp::Append(p, at, bytes), Some(rng)) => match rng.next() % 3 {
            0 => {}
            1 if bytes.len() > 1 => {
                let n = 1 + (rng.next() % (bytes.len() as u64 - 1)) as usize;
                open(p).write_all_at(&bytes[..n], *at).unwrap();
            }
            1 => {}
            _ => open(p).write_all_at(&vec![0; bytes.len()], *at).unwrap(),
        },
        (IoOp::Truncate(p, len), None) => open(p).set_len(*len).unwrap(),
        // A positioned write that landed (the remux's tail and flips, a CAF's size: Stop's steps).
        (IoOp::Write(p, at, bytes), None) => open(p).write_all_at(bytes, *at).unwrap(),
        _ => {}
    }
}

/// splitmix64: the replay's seeded choices.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
