//! Real-scale `apply_ops` benchmarks on a 2-hour project:
//! 1,500 clips cut alternately from two 1-hour 4K recordings (screen, mic and system sound each), a 0.5 s dissolve
//! at every other cut (750), 300 attached layers (200 titles, 50 images, 50 zooms) and 57 media files. Ids are
//! 36-character UUID-shaped strings, as in real projects.
//!
//! Not part of the default suite. Run it in release on a quiet machine:
//! `cargo test -p klipolo-model --release --test perf_batches -- --ignored --nocapture`
//!
//! `load_project_at_two_hours`: opening parses the file once, straight into the project, against
//! a JSON tree first (the code it replaced, copied here). Budget: ≤ 0.5× the tree's time, and a peak heap ≤ 1.5× the
//! project's own (the tree alone peaked at several times the file's size).
//!
//! Budgets: the fastest of 20 runs on the target machine (2020 13" i5, 16 GB), with time and heap side by side. The
//! times leave about 2× headroom over a quiet run, for a machine that is also building; the allocation counts don't
//! depend on load, so they are the exact guards.
//! - `validate`: ≤ 1 ms, and at most 4 allocations (its id set; no messages are built for a valid project);
//! - `apply_ops` with no ops (the clone, the transition refit and `validate`): ≤ 2.5 ms;
//! - with 1 op (a commit, or one dry-run frame of a drag): ≤ 2.5 ms;
//! - with 10 ops: ≤ 3 ms; with 100 ops: ≤ 5 ms; 50 added layers (a paste or a chat batch): ≤ 4 ms;
//! - one clip trim (a ripple over everything after it): ≤ 4 ms;
//! - heap: every `apply_ops` clones the project once (its result); the peak above the caller's stays under 1.5× the
//!   project's own heap, and each layer op beyond the first adds at most 10 allocations (a move) or 20 (a new layer),
//!   none of them growing with the project.
//!
//! `speed_runs_that_bump`: "speed every clip up 4×" on a crowded project, where each 4 s clip
//! holds two 1 s titles 0.1 s apart, so every op bumps a title and the run can't be laid out once. Before batching it
//! took 787 ms at 1,500 clips and 2,085 ms at 2,400 on a 2013 Xeon, a whole pass per op; after the first version,
//! which planned the run again after every op, 1,994 ms and 5,228 ms. Played on lanes (`apply::speed_lanes`) it is
//! linear: budget ≤ 40 ms at 1,500 clips and ≤ 64 ms at 2,400 (10.9 ms and 18.4 ms there), with the result equal
//! to applying the ops one batch each. With free labels on the titles' track as well (one every 16 clips), the lanes
//! refused the track and the run went op by op: 554 ms at 1,500 clips. On lanes each op also checks the
//! free labels a moved layer can reach (past the first layer anchored in its clip or later): 14.6 ms at 1,500 clips
//! and 25.8 ms at 2,400 (94 and 150 labels), and with 300 labels at 1,500 clips 15.7 ms in clip order and 19.8 ms in
//! reverse (the Xeon, load 11), within the same budgets. Checking every label past the op's clip's start instead took
//! 45.8 ms for the 300 labels in clip order.

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use klipolo_model::contrib::builtin;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::motion::{Motion, MotionUse};
use klipolo_model::ops::{Edge, Op, TrackTarget};
use klipolo_model::project::*;
use klipolo_model::query::anchor_at;
use klipolo_model::time::Us;
use klipolo_model::timeline::*;
use klipolo_model::transitions::FEATURE;
use klipolo_model::{apply_ops, load_project, validate, Project};

/// Counts heap use, so each benchmark reports memory beside time.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let live = LIVE.fetch_add(by, Relaxed) + by;
    PEAK.fetch_max(live, Relaxed);
}

// SAFETY: every call forwards to `System` with the caller's arguments; the counters don't touch the memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size(), Relaxed);
            grew(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: AllocLayout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: AllocLayout, new_size: usize) -> *mut u8 {
        let out = unsafe { System.realloc(ptr, layout, new_size) };
        if !out.is_null() {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(new_size, Relaxed);
            LIVE.fetch_sub(layout.size(), Relaxed);
            grew(new_size);
        }
        out
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The counters are the whole process's: benchmarks run one at a time, or each would count the other's heap.
fn alone() -> MutexGuard<'static, ()> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Heap use of one call: allocations, bytes allocated, and the peak above what was live before it.
struct Heap {
    allocs: usize,
    bytes: usize,
    peak: usize,
}

fn heap_of<T>(f: impl FnOnce() -> T) -> Heap {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    let (allocs, bytes) = (ALLOCS.load(Relaxed), BYTES.load(Relaxed));
    let out = f();
    let heap = Heap {
        allocs: ALLOCS.load(Relaxed) - allocs,
        bytes: BYTES.load(Relaxed) - bytes,
        peak: PEAK.load(Relaxed).saturating_sub(live),
    };
    drop(out);
    heap
}

const S: Us = 1_000_000;
const CLIPS: usize = 1_500;
/// Each clip plays 4.8 s from its recording's slot, 0.25 s in, so every cut has footage on both sides.
const CLIP_US: Us = 4_800_000;
const LEAD_US: Us = 250_000;
const HOUR_US: Us = 3_601 * S;

/// A UUID-shaped id (36 characters), the same each run.
fn uuid(kind: u32, n: usize) -> String {
    format!("{kind:08x}-0000-4000-8000-{n:012x}")
}

fn media(id: String, kind: AssetKind, duration_us: Us, has_audio: bool) -> Asset {
    let (width, height) = match kind {
        AssetKind::Audio => (0, 0),
        AssetKind::Image => (1024, 1024),
        _ => (3840, 2160),
    };
    Asset {
        name: format!("{id}.mov"),
        path: format!("/Users/creator/Movies/Klipolo/{id}.mov"),
        id,
        kind,
        duration_us,
        width,
        height,
        fps: if kind == AssetKind::Video { 60.0 } else { 0.0 },
        has_audio,
        audio_lead_us: None,
        rotation: None,
    }
}

/// A layer attached to the clip under it; a title fades in and out over 0.3 s (motion presets).
fn layer(main: &[Clip], id: String, start_us: Us, duration_us: Us, content: ItemContent) -> Item {
    let mut layout = Layout::new(
        Anchor::TopLeft,
        BoxN {
            x: 0.05,
            y: 0.05,
            w: 0.4,
            h: 0.1,
        },
    );
    if matches!(content, ItemContent::Text { .. }) {
        let fade = || MotionUse {
            preset: builtin::motion("fade"),
            params: Default::default(),
            duration_us: 300_000,
            ease: None,
        };
        layout.motion = Some(Motion {
            enter: Some(fade()),
            exit: Some(fade()),
            ..Motion::default()
        });
    }
    Item {
        id,
        start_us,
        duration_us,
        attach: anchor_at(main, start_us),
        layout,
        content,
        effects: Vec::new(),
    }
}

fn title(text: &str) -> ItemContent {
    ItemContent::Text {
        text: text.into(),
        style: TextStyle::default(),
    }
}

/// A 2-hour project, and the ids of its 200 titles.
fn two_hour_project() -> (Project, Vec<String>) {
    let mut p = Project::new("Two hours");
    p.id = uuid(0, 0);
    let mut recordings = Vec::new();
    for r in 1..=2 {
        let file = |part: u32| uuid(r, part as usize);
        p.assets
            .push(media(file(1), AssetKind::Video, HOUR_US, false));
        p.assets
            .push(media(file(2), AssetKind::Audio, HOUR_US, true));
        p.assets
            .push(media(file(3), AssetKind::Audio, HOUR_US, true));
        let source = |part: u32, role| RecordingSource {
            id: format!("{part}"),
            role,
            label: String::new(),
            runs: vec![SourceRun {
                asset_id: file(part),
                offset_us: 0,
                duration_us: HOUR_US,
                drift_ppm: 0.0,
                file_in_us: 0,
            }],
            nudge_us: 0,
            mix: SourceMix::default(),
            synced: None,
        };
        recordings.push(Recording {
            id: uuid(r, 0),
            kind: RecordingKind::Screen,
            sources: vec![
                source(1, SourceRole::Screen),
                source(2, SourceRole::Mic),
                source(3, SourceRole::SystemAudio),
            ],
            primary: "1".into(),
            cursor: None,
            markers: Vec::new(),
            game: None,
        });
    }
    for b in 0..50 {
        p.assets
            .push(media(uuid(3, b), AssetKind::Video, 30 * S, true));
    }
    let logo = uuid(4, 0);
    p.assets
        .push(media(logo.clone(), AssetKind::Image, 0, false));
    p.timeline.main = (0..CLIPS)
        .map(|k| {
            let slot = (k / 2) as Us;
            let source_in = LEAD_US + slot * CLIP_US;
            let mut c = Clip::new(
                ClipSource::Recording {
                    id: recordings[k % 2].id.clone(),
                },
                source_in,
                source_in + CLIP_US,
            );
            c.id = uuid(5, k);
            if k % 2 == 0 {
                c.transition_out = Some(Transition {
                    effect: builtin::TRANSITION_DISSOLVE.into(),
                    duration_us: S / 2,
                    params: Default::default(),
                });
            }
            c
        })
        .collect();
    p.recordings = recordings;
    p.requires.insert(FEATURE.to_string());
    let main = &p.timeline.main;
    let mut tracks = vec![
        Track::new(TrackKind::Overlay, "Overlay 1"),
        Track::new(TrackKind::Overlay, "Overlay 2"),
        Track::new(TrackKind::Zoom, "Zoom"),
    ];
    for (t, track) in tracks.iter_mut().enumerate() {
        track.id = uuid(6, t);
    }
    let titles: Vec<String> = (0..200).map(|k| uuid(7, k)).collect();
    for (k, id) in titles.iter().enumerate() {
        let at = (36 * k as Us + 1) * S;
        tracks[0]
            .items
            .push(layer(main, id.clone(), at, 3 * S, title("Chapter")));
    }
    for m in 0..50 {
        let image = ItemContent::Image {
            asset_id: logo.clone(),
        };
        let at = (144 * m as Us + 15) * S;
        tracks[1]
            .items
            .push(layer(main, uuid(8, m), at, 5 * S, image));
        let zoom = ItemContent::Zoom {
            target: ZoomTarget::Auto { using: vec![] },
            scale: 1.5,
        };
        let at = (144 * m as Us + 60) * S;
        tracks[2]
            .items
            .push(layer(main, uuid(9, m), at, 4 * S, zoom));
    }
    p.timeline.tracks = tracks;
    validate(&p).expect("the benchmark project is valid");
    // A project as the app holds it: the result of an edit, so already normalised.
    let p = apply_ops(&p, &[]).unwrap();
    (p, titles)
}

/// Move each of the first `n` titles 1 s later (they stay attached).
fn moves(p: &Project, titles: &[String], n: usize) -> Vec<Op> {
    let start = |id: &str| {
        p.timeline.tracks[0]
            .items
            .iter()
            .find(|item| item.id == id)
            .unwrap()
            .start_us
    };
    titles[..n]
        .iter()
        .map(|id| Op::MoveItem {
            id: id.clone(),
            track_id: None,
            start_us: start(id) + S,
        })
        .collect()
}

/// 50 new attached titles between the existing ones (a paste or a chat batch).
fn adds(p: &Project) -> Vec<Op> {
    (0..50)
        .map(|k| Op::AddItem {
            track: TrackTarget::Auto,
            item: layer(
                &p.timeline.main,
                uuid(10, k),
                (36 * k as Us + 20) * S,
                3 * S,
                title("Pasted"),
            ),
            attach: true,
        })
        .collect()
}

struct Timing {
    fastest: Duration,
    median: Duration,
}

fn time(mut f: impl FnMut()) -> Timing {
    for _ in 0..3 {
        f();
    }
    let mut runs: Vec<Duration> = (0..20)
        .map(|_| {
            let started = Instant::now();
            f();
            started.elapsed()
        })
        .collect();
    runs.sort();
    Timing {
        fastest: runs[0],
        median: runs[runs.len() / 2],
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

#[test]
#[ignore = "benchmark; run in release with --ignored --nocapture"]
fn apply_ops_at_two_hours() {
    let _alone = alone();
    let (p, titles) = two_hour_project();
    let json = serde_json::to_vec(&p).unwrap().len();
    let own = heap_of(|| p.clone()).bytes;
    println!(
        "2-hour project: {} clips, {} transitions, {} layers, {} media files; {} KB of JSON, {} KB of heap",
        p.timeline.main.len(),
        p.timeline
            .main
            .iter()
            .filter(|c| c.transition_out.is_some())
            .count(),
        p.timeline
            .tracks
            .iter()
            .map(|t| t.items.len())
            .sum::<usize>(),
        p.assets.len(),
        json / 1024,
        own / 1024
    );
    let trim = {
        let clip = &p.timeline.main[CLIPS / 2];
        vec![Op::TrimClip {
            id: clip.id.clone(),
            edge: Edge::End,
            source_us: clip.source_out_us - S / 2,
        }]
    };
    let batches: [(&str, Vec<Op>, f64); 6] = [
        ("apply_ops, no ops", Vec::new(), 2.5),
        ("apply_ops, 1 move", moves(&p, &titles, 1), 2.5),
        ("apply_ops, 10 moves", moves(&p, &titles, 10), 3.0),
        ("apply_ops, 100 moves", moves(&p, &titles, 100), 5.0),
        ("apply_ops, 50 added layers", adds(&p), 4.0),
        ("apply_ops, 1 clip trim", trim, 4.0),
    ];
    let mut over = Vec::new();
    let mut report = |name: &str, t: Timing, heap: &Heap, budget_ms: f64| {
        println!(
            "{name}: fastest {:.2} ms, median {:.2} ms (budget {budget_ms} ms); {} allocations, {} KB allocated, \
             peak {} KB above the caller",
            ms(t.fastest),
            ms(t.median),
            heap.allocs,
            heap.bytes / 1024,
            heap.peak / 1024
        );
        if ms(t.fastest) > budget_ms {
            over.push(format!("{name}: {:.2} ms > {budget_ms} ms", ms(t.fastest)));
        }
        if heap.peak * 2 > own * 3 {
            over.push(format!(
                "{name}: peak {} KB > 1.5 × {} KB",
                heap.peak / 1024,
                own / 1024
            ));
        }
    };
    let t = time(|| validate(&p).unwrap());
    let checked = heap_of(|| validate(&p).unwrap());
    report("validate", t, &checked, 1.0);
    let mut allocs = Vec::new();
    for (name, ops, budget) in &batches {
        let t = time(|| {
            apply_ops(&p, ops).unwrap();
        });
        let heap = heap_of(|| apply_ops(&p, ops).unwrap());
        report(name, t, &heap, *budget);
        allocs.push(heap.allocs);
    }
    if checked.allocs > 4 {
        over.push(format!("validate: {} allocations > 4", checked.allocs));
    }
    // Beyond the no-op batch: per op of the 100 moves and of the 50 added layers.
    let per_move = (allocs[3] - allocs[0]) / 100;
    let per_add = (allocs[4] - allocs[0]) / 50;
    println!("allocations per op: {per_move} per move, {per_add} per added layer");
    if per_move > 10 || per_add > 20 {
        over.push(format!(
            "allocations per op: {per_move} per move (≤ 10), {per_add} per added layer (≤ 20)"
        ));
    }
    assert!(over.is_empty(), "over budget: {over:#?}");
}

/// `load_project` before it parsed once: a JSON tree, then the project from the tree, then `validate`.
fn load_by_tree(json: &str) -> Project {
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let project: Project = serde_json::from_value(value).unwrap();
    validate(&project).unwrap();
    project
}

#[test]
#[ignore = "benchmark; run in release with --ignored --nocapture"]
fn load_project_at_two_hours() {
    let _alone = alone();
    let (p, _) = two_hour_project();
    let json = serde_json::to_string(&p).unwrap();
    let own = heap_of(|| p.clone()).bytes;
    assert_eq!(load_project(&json).unwrap(), load_by_tree(&json));
    // Interleaved: the tree, then once, five times over.
    let (mut tree, mut once) = (Vec::new(), Vec::new());
    for _ in 0..5 {
        tree.push(time(|| drop(load_by_tree(&json))));
        once.push(time(|| drop(load_project(&json).unwrap())));
    }
    let best = |runs: &[Timing]| runs.iter().map(|t| t.fastest).min().unwrap();
    let median = |runs: &[Timing]| {
        let mut m: Vec<Duration> = runs.iter().map(|t| t.median).collect();
        m.sort();
        m[m.len() / 2]
    };
    let tree_heap = heap_of(|| load_by_tree(&json));
    let once_heap = heap_of(|| load_project(&json).unwrap());
    for (name, runs, heap) in [
        ("load_project, before (tree)", &tree, &tree_heap),
        ("load_project, after (once)", &once, &once_heap),
    ] {
        println!(
            "{name}: fastest {:.2} ms, median {:.2} ms; {} allocations, {} KB allocated, peak {} KB above the caller \
             ({} KB of JSON, {} KB of project heap)",
            ms(best(runs)),
            ms(median(runs)),
            heap.allocs,
            heap.bytes / 1024,
            heap.peak / 1024,
            json.len() / 1024,
            own / 1024
        );
    }
    assert!(
        best(&once) * 2 <= best(&tree),
        "parsing once isn't 2× faster"
    );
    assert!(
        once_heap.peak * 2 <= own * 3,
        "peak {} KB > 1.5 × {} KB",
        once_heap.peak / 1024,
        own / 1024
    );
}

/// `clips` clips of 4 s from one file, each with two attached 1 s titles 0.1 s apart (0.5–1.5 s and 1.6–2.6 s in), on
/// one track: speeding a clip up 4× makes its two titles overlap. With `free_every`, every so many clips also have a
/// free (unattached) 0.6 s label 3 s in, on the same track (a track that mixes free and attached layers).
fn crowded(clips: usize, free_every: Option<usize>) -> Project {
    const CLIP: Us = 4 * S;
    let mut p = Project::new("Crowded");
    p.id = uuid(0, 0);
    let file = uuid(1, 0);
    p.assets.push(media(
        file.clone(),
        AssetKind::Video,
        clips as Us * CLIP + S,
        true,
    ));
    p.timeline.main = (0..clips)
        .map(|k| {
            let source = ClipSource::Asset { id: file.clone() };
            let mut c = Clip::new(source, k as Us * CLIP, (k as Us + 1) * CLIP);
            c.id = uuid(2, k);
            c
        })
        .collect();
    let main = &p.timeline.main;
    let mut track = Track::new(TrackKind::Overlay, "Callouts");
    track.id = uuid(3, 0);
    for k in 0..clips {
        let at = k as Us * CLIP;
        for (n, offset) in [(0, S / 2), (1, 16 * S / 10)] {
            track.items.push(layer(
                main,
                uuid(4, 2 * k + n),
                at + offset,
                S,
                title("Callout"),
            ));
        }
        if free_every.is_some_and(|n| k % n == 0) {
            let mut free = layer(main, uuid(5, k), at + 3 * S, 6 * S / 10, title("Label"));
            free.attach = None;
            track.items.push(free);
        }
    }
    p.timeline.tracks = vec![track];
    validate(&p).expect("the crowded project is valid");
    apply_ops(&p, &[]).unwrap()
}

#[test]
#[ignore = "benchmark; run in release with --ignored --nocapture"]
fn speed_runs_that_bump() {
    let _alone = alone();
    let mut over = Vec::new();
    let rows = [
        (1_500, None, false, 40.0),
        (2_400, None, false, 64.0),
        // A free label every 16 clips on the titles' track: 94 at 1,500, 150 at 2,400 (4,950 layers, near
        // the model's 5,000).
        (1_500, Some(16), false, 40.0),
        (2_400, Some(16), false, 64.0),
        // 300 free labels (one every 5 clips), the batch in clip order and in reverse.
        (1_500, Some(5), false, 40.0),
        (1_500, Some(5), true, 40.0),
    ];
    for (clips, free_every, reverse, budget_ms) in rows {
        let p = crowded(clips, free_every);
        let mixed = match (free_every, reverse) {
            (Some(n), false) => format!(", a free label every {n} clips"),
            (Some(n), true) => format!(", a free label every {n} clips, last clip first"),
            (None, _) => String::new(),
        };
        let mut clips_in_order: Vec<&Clip> = p.timeline.main.iter().collect();
        if reverse {
            clips_in_order.reverse();
        }
        let ops: Vec<Op> = clips_in_order
            .into_iter()
            .map(|c| Op::UpdateClip {
                id: c.id.clone(),
                patch: klipolo_model::ops::ClipPatch {
                    speed: Some(4.0),
                    ..Default::default()
                },
            })
            .collect();
        let batch = apply_ops(&p, &ops).unwrap();
        let one_by_one = ops.iter().fold(p.clone(), |q, op| {
            apply_ops(&q, std::slice::from_ref(op)).unwrap()
        });
        assert!(
            batch == one_by_one,
            "{clips} clips: the batch differs from one op at a time"
        );
        assert!(batch.timeline.tracks.len() > 1, "titles were bumped");
        let t = time(|| {
            apply_ops(&p, &ops).unwrap();
        });
        let heap = heap_of(|| apply_ops(&p, &ops).unwrap());
        println!(
            "speed every clip up 4×, every op bumping{mixed}, {clips} clips: fastest {:.2} ms, median {:.2} ms \
             (budget {budget_ms} ms); {} allocations, peak {} KB above the caller",
            ms(t.fastest),
            ms(t.median),
            heap.allocs,
            heap.peak / 1024
        );
        if ms(t.fastest) > budget_ms {
            over.push(format!(
                "{clips} clips{mixed}: {:.2} ms > {budget_ms} ms",
                ms(t.fastest)
            ));
        }
    }
    assert!(over.is_empty(), "over budget: {over:#?}");
}
