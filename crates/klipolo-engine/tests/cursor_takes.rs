//! What a cursor take holds; its budget at real scale is `cursor_budget`.

mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::Path;
use std::time::Duration;

use klipolo_cursor::activity::{
    Region, TICK_US, Tick, decode, encode_chunk, header as activity_header, typing,
};
use klipolo_cursor::facts::{Dwell, StillRuns};
use klipolo_cursor::format::{Capture, Line, TIME_LIMIT_US};
use klipolo_cursor::timeline::{TakeFacts, cursor_at};
use klipolo_engine::media::cursor::{CursorTracks, load_take, load_track, wanted};
use klipolo_model::cursor_style::{CursorCapture, Smoothing};
use klipolo_model::project::Project;
use klipolo_model::query::clip_starts;
use support::takes::{add_take, cursor_file, gliding, header, us, write};

/// Counts this thread's frees of 64 KB or more: a track's or a path's arrays, never the small things a lock guards.
struct Counting;

thread_local! {
    static BIG_FREES: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: forwards to the system allocator; the counter is a const-initialised thread-local (no allocation).
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if layout.size() >= 64 << 10 {
            let _ = BIG_FREES.try_with(|n| n.set(n.get() + 1));
        }
        unsafe { System.dealloc(p, layout) };
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

#[test]
fn a_take_keeps_one_set_of_facts_shares_its_track_and_counts_every_byte() {
    // Four seconds of gliding with a click, then a rest: a still run and a dwell.
    let dir = tempfile::tempdir().unwrap();
    let path = cursor_file(dir.path(), 1);
    let mut lines = gliding(Capture::Separate, 4.0);
    let Some(&Line::Move { x, y, .. }) = lines.last() else {
        unreachable!()
    };
    // The click far from where the cursor comes to rest, so the rest is a dwell.
    let (cx, cy) = (0.05, 0.05);
    lines.push(Line::Button {
        t: us(2.0),
        btn: 0,
        d: 1,
        x: cx,
        y: cy,
    });
    lines.push(Line::Button {
        t: us(2.1),
        btn: 0,
        d: 0,
        x: cx,
        y: cy,
    });
    lines.extend((9..=12).map(|k| Line::Move {
        t: k * 500_000,
        x,
        y,
    }));
    write(&path, &lines);
    let take = load_take(&path).unwrap();
    // `TakeFacts::of_cursor` is the one definition of what the cursor file gives a take.
    let track = load_track(&path).unwrap();
    assert_eq!(
        *take.facts(),
        TakeFacts::of_cursor(&track, &StillRuns::of(&track))
    );
    // The dwells are kept once, inside the facts, which clone for the smart camera to add the activity.
    assert_eq!(take.dwells().len(), 1);
    assert!(std::ptr::eq(take.dwells(), take.facts().dwells.as_slice()));
    let with_activity = TakeFacts {
        typing: Some(Vec::new()),
        ..(*take.facts()).clone()
    };
    assert_ne!(with_activity, *take.facts());
    // The track is shared, so the smart camera's glide reads it through `cursor_at` with no copy.
    let mut p = Project::new("One");
    add_take(&mut p, 1, &path, CursorCapture::Separate);
    let starts = clip_starts(&p.timeline.main);
    let shared = |_: &_| Some(take.track.clone());
    assert_eq!(
        cursor_at(&p, &starts, &shared, us(1.0)),
        take.track.position(us(1.0))
    );
    // `bytes` counts the track, the facts and the variants.
    let bare = take.bytes();
    assert!(bare >= take.track.bytes() + size_of::<Dwell>() + take.still.runs().len() * 16);
    take.prepare(&[Smoothing::Medium]);
    let medium = take.smoothed(Smoothing::Medium).unwrap();
    assert!(
        take.bytes() >= bare + medium.bytes(),
        "{} {}",
        take.bytes(),
        medium.bytes()
    );
}

/// `ticks` as an activity file read back over their duration.
fn activity(ticks: &[Tick]) -> klipolo_cursor::activity::ActivityTrack {
    let duration = ticks.len() as i64 * TICK_US;
    let mut bytes = Vec::new();
    activity_header(duration, &mut bytes);
    for (i, chunk) in ticks.chunks(600).enumerate() {
        encode_chunk(i as u32, chunk, &mut bytes);
    }
    decode(&bytes, duration)
}

#[test]
fn a_burned_in_take_rests_only_until_its_burn_in_so_typing_ends_there_too() {
    // The source went silent at 3 s and the recorder burned the cursor in.
    // The take's still runs end there, so a burned-in cursor moving slowly over a still screen (a small region
    // changing every tick) isn't read as typing after it. Six seconds of it: under the ten a playing video needs.
    let dir = tempfile::tempdir().unwrap();
    let path = cursor_file(dir.path(), 1);
    let mut lines = vec![header(Capture::Separate)];
    lines.extend((0..=5).map(|k| Line::Move {
        t: k * 500_000,
        x: 0.5,
        y: 0.5,
    }));
    lines.push(Line::Burned { t: us(3.0) });
    write(&path, &lines);
    let take = load_take(&path).unwrap();
    assert_eq!(take.still.runs(), [(0, us(3.0))]);
    assert_eq!(take.facts().still, [(0, us(3.0))]);
    let moving = Region {
        x0: 10,
        y0: 10,
        x1: 11,
        y1: 11,
        cells: 4,
    };
    let ticks = vec![
        Tick {
            changed: 4,
            top: [moving, Region::default()]
        };
        60
    ];
    let typed = typing(&activity(&ticks), &take.still, 0);
    assert!(!typed.is_empty(), "typing while it rests");
    assert!(typed.iter().all(|s| s.to <= us(3.0) + TICK_US), "{typed:?}");
}

#[test]
fn a_damaged_file_loads_what_it_can_and_the_loader_goes_on() {
    // Torn and foreign lines, zeros from a power cut, times at and past the reader's limit: the take loads with what it
    // can read, smooths without a panic on the loader thread, and the next take still loads after it.
    let dir = tempfile::tempdir().unwrap();
    let damaged = cursor_file(dir.path(), 1);
    let near = TIME_LIMIT_US - 2_000_000;
    let mut text = String::from(
        "{\"k\":\"h\",\"v\":2,\"capture\":\"separate\",\"clicks\":\"events\",\"w\":1440,\"h\":900}\n",
    );
    for k in 0..400 {
        text += &format!(
            "{{\"k\":\"m\",\"t\":{},\"x\":{},\"y\":0.5}}\n",
            near + k * 4_000,
            0.2 + 0.001 * k as f64
        );
    }
    text += "garbage\n{\"k\":\"m\",\"t\":1,\"x\":NaN,\"y\":0}\n";
    text += &"\0".repeat(4_096);
    text += &format!(
        "\n{{\"k\":\"m\",\"t\":{},\"x\":0.9,\"y\":0.5}}\n",
        TIME_LIMIT_US + 1
    );
    text += &format!(
        "{{\"k\":\"b\",\"t\":{},\"btn\":0,\"d\":1,\"x\":0.5,\"y\":0.5}}\n{{\"k\":\"m\",\"t\":12",
        near + 100_000
    );
    std::fs::create_dir_all(damaged.parent().unwrap()).unwrap();
    std::fs::write(&damaged, text).unwrap();
    let mut p = Project::new("Two");
    add_take(&mut p, 1, &damaged, CursorCapture::Separate);
    let good = cursor_file(dir.path(), 2);
    write(&good, &gliding(Capture::Separate, 2.0));
    add_take(&mut p, 2, &good, CursorCapture::Separate);
    p.cursor.smoothing = Smoothing::Strong;
    let wants = wanted(&p);
    let tracks = CursorTracks::default();
    tracks.want(&wants);
    assert!(tracks.wait_ready(&wants, Duration::from_secs(30)));
    let take = tracks.get(&wants[0].path).expect("loads what it can");
    assert_eq!(take.track.len(), 400);
    assert_eq!(take.track.presses().len(), 1);
    let path = take.smoothed(Smoothing::Strong).unwrap();
    for t in [near, near + 1_000_000, TIME_LIMIT_US, i64::MAX] {
        assert!(path.position(&take.track, t).is_some());
    }
    assert!(
        tracks
            .get(&wants[1].path)
            .is_some_and(|t| t.smoothed(Smoothing::Strong).is_some())
    );
}

#[test]
fn takes_pushed_past_the_budget_go_so_preview_holds_what_export_holds() {
    // A budget of one and a half takes. A and B load; then C is put in front of them. C and A are what export would
    // load (A is the one take past the budget), so B goes from the preview's cache: the budget is a hard cap, and
    // preview draws the cursors export draws.
    let dir = tempfile::tempdir().unwrap();
    let p = support::takes::cursor_takes(3, dir.path(), CursorCapture::Separate);
    let wants = wanted(&p);
    let one = load_take(Path::new(&wants[0].path)).unwrap();
    one.prepare(&wants[0].smoothings);
    let budget = one.bytes() * 3 / 2;
    let preview = CursorTracks::with_budget(budget);
    let (a, b, c) = (wants[0].clone(), wants[1].clone(), wants[2].clone());
    preview.want(&[a.clone(), b.clone()]);
    assert!(preview.wait_ready(&[a.clone(), b.clone()], Duration::from_secs(30)));
    assert!(preview.get(&b.path).is_some());
    let reordered = [c, a, b];
    preview.want(&reordered);
    assert!(preview.wait_ready(&reordered, Duration::from_secs(30)));
    let export = CursorTracks::with_budget(budget);
    export.load_all(&reordered);
    let held = |t: &CursorTracks| -> Vec<bool> {
        reordered.iter().map(|w| t.get(&w.path).is_some()).collect()
    };
    assert_eq!(held(&export), [true, true, false]);
    assert_eq!(held(&preview), held(&export));
}

#[test]
fn every_wanted_takes_variants_are_ready_before_export_plans() {
    // Export loads (and smooths) before its first frame, in the order and budget the preview uses.
    let dir = tempfile::tempdir().unwrap();
    let p = support::takes::cursor_takes(3, dir.path(), CursorCapture::Separate);
    let wants = wanted(&p);
    let export = CursorTracks::default();
    export.load_all(&wants);
    assert!(export.wait_ready(&wants, Duration::ZERO), "already ready");
    for w in &wants {
        let take = export.get(&w.path).unwrap();
        assert!(w.smoothings.iter().all(|&s| take.smoothed(s).is_some()));
    }
    assert!(Path::new(&wants[0].path).exists());
}

#[test]
fn a_project_switch_frees_its_takes_off_the_calling_thread() {
    // The actor clears the cache when the project switches. Ten minutes of motion is a track and a path of hundreds of
    // KB each; they are freed on another thread, never the actor.
    let dir = tempfile::tempdir().unwrap();
    let path = cursor_file(dir.path(), 1);
    write(&path, &gliding(Capture::Separate, 600.0));
    let mut p = Project::new("One");
    add_take(&mut p, 1, &path, CursorCapture::Separate);
    let tracks = CursorTracks::default();
    tracks.load_all(&wanted(&p));
    assert!(tracks.get(&wanted(&p)[0].path).is_some());
    BIG_FREES.with(|n| n.set(0));
    tracks.clear();
    assert_eq!(BIG_FREES.with(Cell::get), 0);
    assert!(tracks.get(&wanted(&p)[0].path).is_none());
}

#[test]
fn a_take_no_longer_wanted_is_freed_off_the_calling_thread() {
    // The project switch's rule on `want`'s path: two ten-minute takes in a budget of one and a half. When the project
    // wants only the second, the first goes (no longer wanted, over the budget), freed on another thread.
    let dir = tempfile::tempdir().unwrap();
    let mut p = Project::new("Two");
    for k in 1..=2 {
        let path = cursor_file(dir.path(), k);
        write(&path, &gliding(Capture::Separate, 600.0));
        add_take(&mut p, k, &path, CursorCapture::Separate);
    }
    let wants = wanted(&p);
    let one = load_take(Path::new(&wants[0].path)).unwrap();
    one.prepare(&wants[0].smoothings);
    let tracks = CursorTracks::with_budget(one.bytes() * 3 / 2);
    drop(one);
    tracks.load_all(&wants);
    assert!(wants.iter().all(|w| tracks.get(&w.path).is_some()));
    BIG_FREES.with(|n| n.set(0));
    tracks.want(&wants[1..]);
    assert_eq!(BIG_FREES.with(Cell::get), 0);
    assert!(tracks.get(&wants[0].path).is_none());
}
