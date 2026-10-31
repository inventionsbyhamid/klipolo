//! Real-scale benchmarks of the engine's audio work, before against after, in one
//! process and interleaved. The project is a long one: 2 hours of 1,500 clips alternating between two recordings
//! (mic and system sound each: 3,000 sources), a dissolve at every other cut, 300 titles. Each recording's files are
//! a minute long and each clip plays a 4.8 s stretch of them (so the mix reads real samples from mapped caches;
//! hour-long files would add their page-ins to "before" only).
//!
//! - Preview loudness after a sound-changing edit. Before: the whole mix rendered and metered (`measure_until`, with
//!   the mixer walking every source per block as it did). After: the estimate from the files' energies.
//! - A full pass of the mix (export renders one per format and measures with one): the walk against the sweep.
//! - The decode pass's loudness work per hour of stereo: the `ebur128` meter it ran against the energy writer.
//! - A title drag's dry run while playing: planning the sound again against the op-kind check that skips it.
//!
//! Not part of the default suite. Run it in release on a quiet machine (a release test binary has no rpath to the LGPL
//! FFmpeg build, which the app bundles, so it is named for the loader):
//! `DYLD_FALLBACK_LIBRARY_PATH=$PWD/.build/ffmpeg-9.0/prefix/lib cargo test -p klipolo-engine --release --lib audio::perf -- --ignored --nocapture --test-threads=1`
//!
//! Budgets (on the target 2020 i5, about 2× headroom over a quiet run): the estimate takes ≤ 30 ms with a peak heap of
//! ≤ 2 MB at this scale; the sweep's full pass is ≤ 0.6× the walk's; the energy writer costs ≤ the meter it replaced;
//! a sound-neutral dry run plans nothing (≤ 0.05 ms, no allocation).

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use klipolo_model::Op;
use klipolo_model::layout::{Anchor, BoxN, Layout as BoxLayout};
use klipolo_model::project::{
    Asset, AssetKind, Loudness, Project, Recording, RecordingKind, RecordingSource, SourceMix,
    SourceRole, SourceRun,
};
use klipolo_model::time::secs_to_us;
use klipolo_model::timeline::{
    Clip, ClipSource, Item, ItemContent, TextStyle, Track, TrackKind, Transition,
};

use super::loudness::{Estimate, estimate_until, measure_until};
use super::mix::{Mixdown, RATE, to_frame};
use super::pcm::energy::EnergyWriter;
use super::pcm::{Energies, Pcm, PcmKey, PcmStore};
use super::plan::{AudioPlan, AudioSource, audio_plan, may_change_sound};
use crate::registry::Registry;

/// Counts heap use while a benchmark case runs (`run`), so each case reports memory beside time. Rust allows one
/// global allocator per binary, so it is installed for the whole lib test binary, but it counts only while `COUNTING`
/// is on: every other unit test's allocations pass straight to `System` after one relaxed load, uncounted.
struct Counting;

static COUNTING: AtomicBool = AtomicBool::new(false);
/// Bytes allocated minus freed while counting (frees of earlier allocations can take it below where it started).
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

fn grew(by: isize) {
    let live = LIVE.fetch_add(by, Relaxed) + by;
    PEAK.fetch_max(live, Relaxed);
}

// SAFETY: every call forwards to `System` with the caller's arguments; the counters don't touch the memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && COUNTING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            grew(layout.size() as isize);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        if COUNTING.load(Relaxed) {
            grew(-(layout.size() as isize));
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let out = unsafe { System.realloc(ptr, layout, size) };
        if !out.is_null() && COUNTING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            grew(size as isize - layout.size() as isize);
        }
        out
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// One run of `f`: milliseconds, allocations and the peak heap above the caller (counted only meanwhile; the
/// benchmark runs with `--test-threads=1`, so no other test is counted).
fn run<T>(f: &mut impl FnMut() -> T) -> (f64, usize, usize, T) {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    let allocs = ALLOCS.load(Relaxed);
    COUNTING.store(true, Relaxed);
    let started = Instant::now();
    let out = f();
    let ms = started.elapsed().as_secs_f64() * 1e3;
    COUNTING.store(false, Relaxed);
    (
        ms,
        ALLOCS.load(Relaxed) - allocs,
        usize::try_from(PEAK.load(Relaxed) - live).unwrap_or(0),
        out,
    )
}

/// Before and after, interleaved `runs` times: prints the fastest and median of each with the heap of one run, and
/// returns the allocations of one run of each.
fn compare<A, B>(
    what: &str,
    runs: usize,
    mut before: impl FnMut() -> A,
    mut after: impl FnMut() -> B,
) -> (usize, usize) {
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let (mut heap_a, mut heap_b) = ((0, 0), (0, 0));
    for _ in 0..runs {
        let (ms, allocs, peak, _) = run(&mut before);
        a.push(ms);
        heap_a = (allocs, peak);
        let (ms, allocs, peak, _) = run(&mut after);
        b.push(ms);
        heap_b = (allocs, peak);
    }
    let stats = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        (v[0], v[v.len() / 2])
    };
    let ((fa, ma), (fb, mb)) = (stats(&mut a), stats(&mut b));
    println!(
        "{what}\n  before: fastest {fa:.2} ms, median {ma:.2} ms; {} allocations, peak heap {:.1} KB\n  after:  fastest {fb:.3} ms, median {mb:.3} ms; {} allocations, peak heap {:.1} KB",
        heap_a.0,
        heap_a.1 as f64 / 1024.0,
        heap_b.0,
        heap_b.1 as f64 / 1024.0,
    );
    (heap_a.0, heap_b.0)
}

fn load() -> String {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    format!("load {out}")
}

/// A minute of 48 kHz 16-bit WAV; sample `n` of channel `c` from `f`.
fn wav(path: &Path, channels: u16, f: impl Fn(usize, usize) -> f32) {
    use std::io::Write;
    let frames = 60 * RATE as usize;
    let data = (frames * 2 * usize::from(channels)) as u32;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for part in [
        &b"RIFF"[..],
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &channels.to_le_bytes(),
        &RATE.to_le_bytes(),
        &(RATE * 2 * u32::from(channels)).to_le_bytes(),
        &(2 * channels).to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ] {
        out.write_all(part).unwrap();
    }
    for n in 0..frames {
        for c in 0..usize::from(channels) {
            let x = (f(n, c).clamp(-1.0, 1.0) * 32767.0) as i16;
            out.write_all(&x.to_le_bytes()).unwrap();
        }
    }
}

fn noise(seed: u64, n: usize) -> f32 {
    let mut x = (n as u64 ^ seed.rotate_left(17)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// A 2-hour project over `mic` and `system` (a minute each, 4.8 s stretches of them per clip).
fn two_hours(mic: &str, system: &str) -> Project {
    let mut p = Project::new("audio bench");
    let asset = |id: String, kind, path: &str, has_audio| Asset {
        id: id.clone(),
        kind,
        path: path.into(),
        name: id,
        duration_us: secs_to_us(3_600.0),
        width: if kind == AssetKind::Video { 3840 } else { 0 },
        height: if kind == AssetKind::Video { 2160 } else { 0 },
        fps: if kind == AssetKind::Video { 30.0 } else { 0.0 },
        has_audio,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    };
    for r in ["a", "b"] {
        p.assets.push(asset(
            format!("screen-{r}"),
            AssetKind::Video,
            "/screen.mov",
            false,
        ));
        p.assets
            .push(asset(format!("mic-{r}"), AssetKind::Audio, mic, true));
        p.assets
            .push(asset(format!("system-{r}"), AssetKind::Audio, system, true));
        let source = |id: &str, role| RecordingSource {
            id: id.into(),
            role,
            label: format!("capture.source.{id}"),
            runs: vec![SourceRun {
                asset_id: format!("{id}-{r}"),
                offset_us: 0,
                duration_us: secs_to_us(3_600.0),
                drift_ppm: 0.0,
                file_in_us: 0,
            }],
            nudge_us: 0,
            mix: SourceMix::default(),
            synced: None,
        };
        p.recordings.push(Recording {
            id: format!("rec-{r}"),
            kind: RecordingKind::Screen,
            sources: vec![
                source("screen", SourceRole::Screen),
                source("mic", SourceRole::Mic),
                source("system", SourceRole::SystemAudio),
            ],
            primary: "screen".into(),
            cursor: None,
            markers: Vec::new(),
            game: None,
        });
    }
    let clip = secs_to_us(4.8);
    for k in 0..1_500i64 {
        let rec = if k % 2 == 0 { "rec-a" } else { "rec-b" };
        let from = secs_to_us(1.0) + (k / 2 * clip) % secs_to_us(52.0);
        let mut c = Clip::new(ClipSource::Recording { id: rec.into() }, from, from + clip);
        c.id = format!("clip-{k}");
        if k % 2 == 0 {
            c.transition_out = Some(Transition {
                effect: klipolo_model::contrib::builtin::TRANSITION_DISSOLVE.into(),
                duration_us: secs_to_us(0.5),
                params: Default::default(),
            });
        }
        p.timeline.main.push(c);
    }
    let mut titles = Track::new(TrackKind::Overlay, "Titles");
    titles.id = "titles".into();
    titles.items = (0..300)
        .map(|k| Item {
            id: format!("title-{k}"),
            start_us: secs_to_us(24.0 * k as f64),
            duration_us: secs_to_us(3.0),
            attach: None,
            layout: BoxLayout::new(
                Anchor::TopLeft,
                BoxN {
                    x: 0.1,
                    y: 0.1,
                    w: 0.5,
                    h: 0.2,
                },
            ),
            content: ItemContent::Text {
                text: format!("Step {k}"),
                style: TextStyle::default(),
            },
            effects: Vec::new(),
        })
        .collect();
    p.timeline.tracks.push(titles);
    p.audio.loudness = Loudness::Youtube;
    klipolo_model::apply_ops(&p, &[]).unwrap()
}

/// `measure_until` as it ran before the sweep: the mix rendered with the mixer walking every source per block.
fn exact_with_walk(
    plan: &AudioPlan,
    pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>,
) -> Option<f64> {
    let end = plan.sources.iter().map(|s| s.end_us).max()?;
    let start = plan.sources.iter().map(|s| s.start_us).min()?.max(0);
    let (from, to) = (to_frame(start) as u64, to_frame(end) as u64);
    let mut meter = ebur128::EbuR128::new(2, RATE, ebur128::Mode::I).ok()?;
    let mut mixdown = Mixdown::new(Arc::new(plan.clone()));
    mixdown.walk_every_source();
    let mut block = vec![0f32; 2 * 4_096];
    let mut at = from;
    while at < to {
        let n = (to - at).min(4_096) as usize;
        mixdown.render(pcm, at, &mut block[..n * 2]);
        meter.add_frames_f32(&block[..n * 2]).ok()?;
        at += n as u64;
    }
    meter.loudness_global().ok().filter(|l| l.is_finite())
}

/// One full pass of the mix (no meter), the walk or the sweep.
fn full_pass(
    plan: &Arc<AudioPlan>,
    pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>,
    walk: bool,
) -> f32 {
    let mut mixdown = Mixdown::new(plan.clone());
    if walk {
        mixdown.walk_every_source();
    }
    let to = plan
        .sources
        .iter()
        .map(|s| to_frame(s.end_us))
        .max()
        .unwrap_or(0) as u64;
    let mut block = vec![0f32; 2 * 4_096];
    let (mut at, mut sum) = (0, 0f32);
    while at < to {
        mixdown.render(pcm, at, &mut block);
        sum += block[0];
        at += 4_096;
    }
    sum
}

#[test]
#[ignore = "benchmark: run in release (see the module docs)"]
fn pa3_engine_at_real_scale() {
    println!("start: {}", load());
    let dir = tempfile::tempdir().unwrap();
    let (mic, system) = (dir.path().join("mic.wav"), dir.path().join("system.wav"));
    wav(&mic, 1, |n, _| {
        let t = n as f32 / RATE as f32;
        let speaking = (t * 0.5).fract() < 0.8;
        if speaking { 0.3 * noise(1, n) } else { 0.0 }
    });
    wav(&system, 2, |n, c| 0.1 * noise(2 + c as u64, n));
    let p = two_hours(&mic.display().to_string(), &system.display().to_string());
    let registry = Registry::builtin();
    let plan = Arc::new(audio_plan(&p, &registry, Arc::default()));
    let store = PcmStore::new(dir.path().join("cache"));
    let keys = [
        PcmKey {
            path: mic.display().to_string(),
        },
        PcmKey {
            path: system.display().to_string(),
        },
    ];
    let files: Vec<(String, Arc<Pcm>, Energies)> = keys
        .iter()
        .map(|k| {
            let pcm = store.build(k).unwrap();
            let energies = store.energies(k, &pcm).unwrap();
            (k.path.clone(), pcm, energies)
        })
        .collect();
    let pcm = |s: &AudioSource| files.iter().find(|f| f.0 == s.path).map(|f| f.1.clone());
    let energies = |s: &AudioSource| files.iter().find(|f| f.0 == s.path).map(|f| &f.2);
    let effected = |s: &AudioSource, p: &Arc<Pcm>, c: &AtomicBool| store.effected(s, p, c);
    let hours = plan.sources.iter().map(|s| s.end_us).max().unwrap() as f64 / 3.6e9;
    println!(
        "project: {} clips, {} sound sources, {:.2} h; {} titles",
        p.timeline.main.len(),
        plan.sources.len(),
        hours,
        p.timeline.tracks[0].items.len()
    );

    // 1. Preview loudness after a sound-changing edit.
    let cancel = AtomicBool::new(false);
    let exact = exact_with_walk(&plan, &pcm).unwrap();
    let Estimate::Lufs(Some(estimate)) = estimate_until(&plan, &pcm, &energies, &effected, &cancel)
    else {
        panic!("no estimate");
    };
    let now = measure_until(&plan, &pcm, &cancel).unwrap();
    println!("loudness: exact {exact:.3} LUFS (sweep {now:.3}), estimate {estimate:.3} LUFS");
    compare(
        &format!("1. preview loudness per sound-changing edit ({})", load()),
        3,
        || exact_with_walk(&plan, &pcm),
        || estimate_until(&plan, &pcm, &energies, &effected, &cancel),
    );
    compare(
        &format!(
            "   exact measure, the walk → the sweep (export's measure) ({})",
            load()
        ),
        3,
        || exact_with_walk(&plan, &pcm),
        || measure_until(&plan, &pcm, &cancel),
    );

    // 2. A full pass of the mix.
    compare(
        &format!(
            "2. full mix pass, 3,000 sources over 2 h: the walk → the sweep ({})",
            load()
        ),
        3,
        || full_pass(&plan, &pcm, true),
        || full_pass(&plan, &pcm, false),
    );
    // The same pass with every recorded sound drifting: each of the 3,000 voices reads resampled at its
    // run's rate instead of straight. Budget: ≤ 1.5× the straight sweep, and no more allocations than it, give or take
    // the shared read buffers growing: the voices read into one set (`Reads`), not one each.
    let mut drifting = p.clone();
    let runs = drifting
        .recordings
        .iter_mut()
        .flat_map(|r| &mut r.sources)
        .filter(|s| !s.role.is_video())
        .flat_map(|s| &mut s.runs);
    for (k, run) in runs.enumerate() {
        run.drift_ppm = if k % 2 == 0 { 100.0 } else { -80.0 };
    }
    let drifting = Arc::new(audio_plan(&drifting, &registry, Arc::default()));
    let (straight, drifted) = compare(
        &format!(
            "   full mix pass: straight → every recorded sound drift-corrected ({})",
            load()
        ),
        3,
        || full_pass(&plan, &pcm, false),
        || full_pass(&drifting, &pcm, false),
    );
    assert!(
        drifted <= straight + 4,
        "a drifting pass allocates no more than a straight one: {drifted} > {straight}"
    );

    // 3. The decode pass's loudness work, per hour of stereo (a minute, ×60).
    let samples: Vec<f32> = (0..60 * RATE as usize)
        .flat_map(|n| [noise(3, n), noise(4, n)])
        .collect();
    let scratch = dir.path().join("e.energy");
    compare(
        &format!(
            "3. decode-pass loudness work per minute of stereo (×60 per hour): meter → energies ({})",
            load()
        ),
        5,
        || {
            let mut meter =
                ebur128::EbuR128::new(2, RATE, ebur128::Mode::I | ebur128::Mode::HISTOGRAM)
                    .unwrap();
            for block in samples.chunks(2 * 1_024) {
                meter.add_frames_f32(block).unwrap();
            }
            meter.loudness_global().ok()
        },
        || {
            let mut writer = EnergyWriter::create(&scratch, 2, 60 * u64::from(RATE)).unwrap();
            writer.begin(0);
            for (k, block) in samples.chunks(2 * 1_024).enumerate() {
                writer.push(k as u64 * 1_024, block).unwrap();
            }
            writer.end_run(true).unwrap();
            writer.finish(60 * u64::from(RATE)).unwrap()
        },
    );
    let bytes: u64 = files
        .iter()
        .map(|(path, _, _)| {
            let stem = crate::media::key::stem(path, &crate::media::key::CacheKind::Pcm).unwrap();
            std::fs::metadata(dir.path().join("cache").join(format!("{stem}.energy")))
                .unwrap()
                .len()
        })
        .sum();
    println!(
        "   energies on disk: {bytes} bytes for a minute of mono + a minute of stereo (×60: {} per hour)",
        (bytes - 64) * 60 + 64
    );

    // 4. A title drag's dry run while playing.
    let drag = [Op::MoveItem {
        id: "title-150".into(),
        track_id: None,
        start_us: secs_to_us(3_601.0),
    }];
    compare(
        &format!(
            "4. sound side of a title-drag dry run while playing: replan → op-kind check ({})",
            load()
        ),
        20,
        || audio_plan(&p, &registry, Arc::default()),
        || drag.iter().any(|op| may_change_sound(&p, op)),
    );
    println!("end: {}", load());
}
