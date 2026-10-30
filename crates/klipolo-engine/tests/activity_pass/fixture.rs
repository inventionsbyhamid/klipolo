//! The FFmpeg decoder on the committed fixture (`scripts/gen-capture-fixtures.sh`): 12 s of a 640 × 360 screen, still
//! for 4 s, then a line typed a glyph every 0.2 s, then a square sliding along a row.

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use klipolo_cursor::activity::QUIET_CELLS;
use klipolo_engine::platform::activity::{ActivityError, LUMA_BYTES, Opener, open_frames};
use klipolo_model::time::Us;

use super::{S, never, stores, wait_until};

pub fn fixture() -> String {
    clip("activity-12s-360p.mp4")
}

fn clip(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    assert!(path.exists(), "run scripts/gen-capture-fixtures.sh");
    path.display().to_string()
}

/// Tick 0 is the video stream's first frame, the clock every decoder numbers frames on (engine fix
/// 7): an audio lead inside a file is the recording's `audioLeadUs`, never a shift of the picture's clock. This clip's
/// sound starts at 0 and its video 0.5 s in, for 2.5 s, and a box appears 1 s into the video: so at 1.0 s on the
/// video's clock, the change of tick 9 (the container's clock would put it at tick 14), and the track lasts the
/// video's 2.5 s.
#[test]
fn ticks_count_from_the_video_streams_start() {
    let dir = tempfile::tempdir().unwrap();
    let (_thumbs, store) = stores(dir.path(), None);
    let video = clip("activity-offset-3s.mkv");
    let track = store.ensure(&video, &never(), &|_| {}).unwrap();
    // The video's own length (Matroska states none for it: the container's end less the video's start; the sound's
    // encoder padding adds a few ms).
    assert!(
        (2_500_000..2_600_000).contains(&track.duration_us()),
        "{}",
        track.duration_us()
    );
    let changed = |i: i64| track.tick_at(i * 100_000).is_some_and(|t| t.changed > 0);
    assert_eq!((0..30).find(|&i| changed(i)), Some(9));
    // A seek is on the same clock: from 0.9 s, the box appears between the first picture and the second.
    let cancel = Arc::new(AtomicBool::new(false));
    let mut from = open_frames(Path::new(&video), 900_000, &cancel).unwrap();
    let (mut before, mut after) = ([0u8; LUMA_BYTES], [0u8; LUMA_BYTES]);
    assert_eq!(from.next(&mut before).unwrap(), Some(900_000));
    assert_eq!(from.next(&mut after).unwrap(), Some(1_000_000));
    assert!(before != after, "the box appears at 1.0 s");
    assert_eq!(from.duration_us(), track.duration_us());
}

#[test]
fn the_pass_finds_the_typing_box_and_quiet_background() {
    let dir = tempfile::tempdir().unwrap();
    let (_thumbs, store) = stores(dir.path(), None);
    let video = fixture();
    let track = store.ensure(&video, &never(), &|_| {}).unwrap();
    assert_eq!(track.duration_us(), 12 * S);
    assert!(track.is_complete());
    let tick = |i: i64| track.tick_at(i * 100_000);
    // 0–3.9 s: nothing moves.
    assert!((0..39).all(|i| tick(i).is_some_and(|t| t.changed <= QUIET_CELLS)));
    let quiet = track.quiet_spans(2 * S);
    assert_eq!(quiet.first().map(|q| q.0), Some(0));
    assert!(quiet[0].1 >= 3_900_000, "{quiet:?}");
    // 4–8 s: a glyph every 0.2 s, and nothing changes but the line being typed (cells 10–25 across, row 10).
    let typed: Vec<_> = (39..79)
        .filter_map(tick)
        .filter(|t| t.changed > 0)
        .collect();
    assert!((18..=22).contains(&typed.len()), "{} glyphs", typed.len());
    for t in &typed {
        let r = t.top[0];
        assert!(
            t.changed <= 4 && r.x0 >= 9 && r.x1 <= 26 && r.y0 >= 9 && r.y1 <= 11,
            "{t:?}"
        );
    }
    // 8–12 s: the square slides along row 25–28 every tick.
    for i in 79..119 {
        let t = tick(i).unwrap();
        assert!(
            t.changed >= 8 && t.top[0].y0 >= 24 && t.top[0].y1 <= 29,
            "tick {i}: {t:?}"
        );
    }
    // The last tick needs a picture at 12 s, after the last frame: never made.
    assert_eq!(tick(119), None);
    assert!(store.get(&video).is_some_and(|t| t.is_complete()));
}

/// A resumed pass opens the decoder at a chunk's start: from a seek, its pictures are the ones a pass from the start
/// sees at those ticks (the fixture's keyframes are a second apart, so 5.5 s decodes from 5 s).
#[test]
fn a_decoder_opened_mid_file_gives_the_pictures_a_pass_from_the_start_does() {
    let video = fixture();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut whole = open_frames(Path::new(&video), 0, &cancel).unwrap();
    let mut from = open_frames(Path::new(&video), 5_500_000, &cancel).unwrap();
    assert_eq!(whole.duration_us(), 12 * S);
    let (mut a, mut b) = ([0u8; LUMA_BYTES], [0u8; LUMA_BYTES]);
    for _ in 0..55 {
        whole.next(&mut a).unwrap().unwrap();
    }
    for i in 55..75 {
        assert_eq!(whole.next(&mut a).unwrap(), Some(i * 100_000));
        assert_eq!(from.next(&mut b).unwrap(), Some(i * 100_000));
        assert!(a == b, "tick {i}");
    }
}

/// For the trend only (software decode on Linux): × real time over `KLIPOLO_ACTIVITY_CLIP` (the fixture by default), and
/// the peak memory the pass added (Linux).
#[test]
#[ignore]
fn activity_pass_speed() {
    let video = std::env::var("KLIPOLO_ACTIVITY_CLIP").unwrap_or_else(|_| fixture());
    let dir = tempfile::tempdir().unwrap();
    let (_t, store) = stores(dir.path(), None);
    let rss = |field: &str| {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let line = status.lines().find(|l| l.starts_with(field)).unwrap_or("");
        line.split_whitespace()
            .nth(1)
            .and_then(|kb| kb.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let before = rss("VmRSS:");
    let started = Instant::now();
    let track = store.ensure(&video, &never(), &|_| {}).unwrap();
    let took = started.elapsed().as_secs_f64();
    let seconds = track.duration_us() as f64 / 1e6;
    println!(
        "activity pass: {seconds:.1} s of video in {took:.2} s: {:.1}× real time, {:.0} s per hour of this video; \
         peak RSS {} MB over the {} MB before",
        seconds / took,
        3_600.0 * took / seconds,
        rss("VmHWM:").saturating_sub(before) / 1024,
        before / 1024
    );
    assert!(track.is_complete());
}

/// What a file is (a container FFmpeg rejects) can't be analysed for good, while a file it may
/// read later (no access now) can't be read now; only the first is ever marked.
#[test]
fn open_errors_say_whether_they_last() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let garbage = dir.path().join("garbage.mp4");
    std::fs::write(&garbage, b"not a video at all").unwrap();
    let opened = open_frames(&garbage, 0, &cancel).map(|_| ());
    assert!(
        matches!(opened, Err(ActivityError::Unsupported(_))),
        "{opened:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.path().join("locked.mp4");
        std::fs::copy(fixture(), &locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads it anyway: nothing to check then.
        if std::fs::File::open(&locked).is_err() {
            let opened = open_frames(&locked, 0, &cancel).map(|_| ());
            assert!(
                matches!(opened, Err(ActivityError::Unavailable(_))),
                "{opened:?}"
            );
        }
    }
}

/// An `ffconcat` list named like a recording: a container outside the media guard's demuxers, as a file imported before
/// the guard can be.
fn ffconcat_list(dir: &Path) -> std::path::PathBuf {
    let list = dir.join("list.mov");
    std::fs::write(&list, "ffconcat version 1.0\nfile screen.mov\n").unwrap();
    list
}

/// Guard 1: a file the media guard refuses never opens, however often it is tried, so it can't be analysed; a file that
/// isn't there may be later, so it can't be read now.
#[test]
fn a_container_the_media_guard_refuses_cant_be_analysed() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = never();
    let opened = open_frames(&ffconcat_list(dir.path()), 0, &cancel).map(|_| ());
    assert!(
        matches!(opened, Err(ActivityError::Unsupported(_))),
        "{opened:?}"
    );
    let opened = open_frames(&dir.path().join("gone.mov"), 0, &cancel).map(|_| ());
    assert!(
        matches!(opened, Err(ActivityError::Unavailable(_))),
        "{opened:?}"
    );
}

/// Guard 1: the editor's pass marks a file the guard refuses after one open, and nothing opens it again (an export
/// plans without it at once): the list, and a QuickTime reference movie, whose picture is in another file the guard
/// never opens (`enable_drefs=0`).
#[test]
fn a_file_the_media_guard_refuses_is_marked_after_one_open() {
    let dir = tempfile::tempdir().unwrap();
    let reference = dir.path().join("reference.mov");
    let movie = reference_movie(&std::fs::read(fixture()).unwrap(), "/elsewhere/screen.mov");
    std::fs::write(&reference, movie).unwrap();
    // It opens (FFmpeg skips the external track's data: "Skipped opening external track"), and gives no picture.
    let mut frames = open_frames(&reference, 0, &never()).unwrap();
    assert_eq!(frames.next(&mut [0u8; LUMA_BYTES]).unwrap(), None);
    drop(frames);
    for file in [ffconcat_list(dir.path()), reference] {
        let video = file.display().to_string();
        let opens = Arc::new(AtomicUsize::new(0));
        let counted: Opener = {
            let opens = opens.clone();
            Arc::new(move |path: &Path, from_us: Us, cancel: &Arc<AtomicBool>| {
                opens.fetch_add(1, Ordering::SeqCst);
                open_frames(path, from_us, cancel)
            })
        };
        let (_t, store) = stores(dir.path(), Some(counted));
        store.want(std::slice::from_ref(&video));
        // Marked on disk (after the worker lets the file go), so the export's look below opens nothing.
        wait_until("the editor's pass marks it", || {
            opens.load(Ordering::SeqCst) > 0 && !store.pending() && store.failed_for_good(&video)
        });
        let again = store.ensure(&video, &never(), &|_| {}).map(|_| ());
        assert!(
            matches!(again, Err(ActivityError::Unsupported(_))),
            "{video}: {again:?}"
        );
        assert_eq!(opens.load(Ordering::SeqCst), 1, "{video}: opened once");
    }
}

/// `mp4` (one track, its index before its data) as a QuickTime reference movie: the track's data reference is a Mac
/// alias to `target` (the absolute path FFmpeg's `mov_read_dref` reads) in place of "in this file". The boxes around it
/// grow by what it adds; the sample offsets, into a file never read, stay as they are.
fn reference_movie(mp4: &[u8], target: &str) -> Vec<u8> {
    fn size(mp4: &[u8], at: usize) -> usize {
        u32::from_be_bytes(mp4[at..at + 4].try_into().unwrap()) as usize
    }
    /// The box named `kind` among those in `within`.
    fn find(mp4: &[u8], within: Range<usize>, kind: &[u8; 4]) -> Range<usize> {
        let mut at = within.start;
        while at + 8 <= within.end {
            if mp4[at + 4..at + 8] == kind[..] {
                return at..at + size(mp4, at);
            }
            at += size(mp4, at).max(8);
        }
        panic!("no {} box", String::from_utf8_lossy(kind));
    }
    let mut boxes = vec![find(mp4, 0..mp4.len(), b"moov")];
    for kind in [b"trak", b"mdia", b"minf", b"dinf", b"dref"] {
        let parent = boxes.last().unwrap().clone();
        boxes.push(find(mp4, parent.start + 8..parent.end, kind));
    }
    let dref = boxes.pop().unwrap();
    // The alias: its fixed fields (150 bytes; no volume name), then its records: the absolute path (type 2, padded to
    // an even length) and the end (type -1).
    let mut alias = vec![0u8; 150];
    alias.extend_from_slice(&2u16.to_be_bytes());
    alias.extend_from_slice(&(target.len() as u16).to_be_bytes());
    alias.extend_from_slice(target.as_bytes());
    alias.resize(alias.len() + target.len() % 2, 0);
    alias.extend_from_slice(&[0xff, 0xff, 0, 0]);
    let entry = 12 + alias.len();
    let mut new = Vec::with_capacity(16 + entry);
    new.extend_from_slice(&(16 + entry as u32).to_be_bytes());
    new.extend_from_slice(b"dref");
    new.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]); // version and flags; one entry
    new.extend_from_slice(&(entry as u32).to_be_bytes());
    new.extend_from_slice(b"alis");
    new.extend_from_slice(&[0; 4]); // flags 0: the data isn't in this file
    new.extend_from_slice(&alias);
    let grown = new.len() - dref.len();
    let mut out = [&mp4[..dref.start], &new[..], &mp4[dref.end..]].concat();
    for container in boxes {
        let grew = (size(mp4, container.start) + grown) as u32;
        out[container.start..container.start + 4].copy_from_slice(&grew.to_be_bytes());
    }
    out
}
