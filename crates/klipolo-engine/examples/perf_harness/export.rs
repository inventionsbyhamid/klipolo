//! Export scenario: a 60 s 1080p30 project (`screen-60s-1080p30.mp4`, from `scripts/gen-engine-fixtures.sh`) exported
//! by a real worker process (`--export-worker`, as the app runs it) in one format and in all three, at Standard and
//! High, in segments of `KLIPOLO_EXPORT_SEGMENT_S` seconds (default 30). Reports wall time, the
//! realtime factor, CPU (the worker plus VideoToolbox's decoder and encoder services, which do their work out of
//! process) and peak footprints.
//!
//! `cargo run -p klipolo-engine --example perf_harness -- export [<worker binary>]` (default: this harness, which runs
//! the worker for `--export-worker`); `… -- export-job <dir> <standard|high|maximum|master> <hardware|softwareSafe>
//! <seconds> <landscape|portrait|square>…` plans a job (files in `<dir>`, the job in `<dir>/.jobs/<id>`) and prints
//! its manifest's path for running the worker by hand; `… -- export-case layers|scattered <quality> <video>...` exports a
//! 2-minute text-heavy short with four video layers, or 50 scattered ranges of a 2-hour file (`case`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_engine::api::ExportQuality;
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::naming::plan_files;
use klipolo_engine::export::protocol::{WorkerLine, parse_line};
use klipolo_engine::export::runner::Batch;
use klipolo_model::contrib::ContribId;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::motion::Motion;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::speech::{Speech, Transcript, Word, WordKind};
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, TextStyle, Track, TrackKind};

use super::measure::{Usage, mb, usage};
use super::project::{fixture, secs};

/// The fixture's size (`KLIPOLO_EXPORT_ASSET_SIZE=3840x2160` for a 4K one).
fn size() -> (u32, u32) {
    std::env::var("KLIPOLO_EXPORT_ASSET_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1920, 1080))
}

fn project_of(seconds: f64) -> Project {
    let mut p = Project::new("Export bench");
    p.assets = vec![Asset {
        id: "screen".into(),
        kind: AssetKind::Video,
        path: fixture(
            &std::env::var("KLIPOLO_EXPORT_FIXTURE")
                .unwrap_or_else(|_| "screen-60s-1080p30.mp4".into()),
        ),
        name: "screen".into(),
        duration_us: secs(60.0),
        width: size().0,
        height: size().1,
        fps: 30.0,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset {
            id: "screen".into(),
        },
        0,
        secs(seconds),
    )];
    if let Ok(target) = std::env::var("KLIPOLO_EXPORT_LOUDNESS") {
        p.audio.loudness =
            serde_json::from_value(serde_json::Value::String(target)).expect("off|youtube|podcast");
    }
    p
}

fn project() -> Project {
    project_of(60.0)
}

/// `export-job`: writes a job for running the worker by hand.
pub fn write_job(args: &[String]) {
    fn word<T: serde::de::DeserializeOwned>(s: &str) -> T {
        serde_json::from_value(serde_json::Value::String(s.into())).expect(s)
    }
    let dir = PathBuf::from(&args[0]);
    std::fs::create_dir_all(&dir).unwrap();
    let quality: ExportQuality = word(&args[1]);
    let seconds: f64 = args[3].parse().unwrap();
    let formats: Vec<FormatKind> = args[4..]
        .iter()
        .map(|f| word(f))
        .collect::<Vec<FormatKind>>();
    let p = project_of(seconds);
    let files = plan_files(&p, &formats, quality, &dir);
    let batch = Batch {
        project: p,
        fps: 30,
        quality,
        files,
        pcm_dir: None,
        speech: Default::default(),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor: false,
    };
    let mut job = ExportJob::create(&batch, segment_s()).unwrap();
    job.mode = word(&args[2]);
    job.save().unwrap();
    println!("{}", job.manifest_path().display());
}

fn segment_s() -> u32 {
    std::env::var("KLIPOLO_EXPORT_SEGMENT_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(klipolo_engine::export::job::SEGMENT_SECONDS)
}

/// VideoToolbox's XPC services (decode and encode run there, not in the worker).
const SERVICES: [&str; 2] = ["VTDecoderXPCService", "VTEncoderXPCService"];

/// Each running service process and its usage.
fn services() -> Vec<(i32, Usage)> {
    SERVICES
        .iter()
        .filter_map(|name| Command::new("pgrep").args(["-x", name]).output().ok())
        .flat_map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.trim().parse::<i32>().ok())
                .collect::<Vec<_>>()
        })
        .filter_map(|pid| usage(pid).map(|u| (pid, u)))
        .collect()
}

fn run_one(bin: &Path, dir: &Path, formats: &[FormatKind], quality: ExportQuality) {
    run_project(
        bin,
        dir,
        project(),
        Speech::default(),
        formats,
        quality,
        60.0,
    );
}

/// Exports `p` (`seconds` long) in `formats` at `quality` with the worker `bin`, and prints what it cost.
fn run_project(
    bin: &Path,
    dir: &Path,
    p: Project,
    speech: Speech,
    formats: &[FormatKind],
    quality: ExportQuality,
    seconds: f64,
) {
    let files = plan_files(&p, formats, quality, dir);
    let batch = Batch {
        project: p,
        fps: 30,
        quality,
        files,
        pcm_dir: None,
        speech: Arc::new(speech),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor: false,
    };
    let job = ExportJob::create(&batch, segment_s()).unwrap();
    let job_path = job.manifest_path();
    // CPU each service process used during the export (processes come and go with the sessions).
    let mut cpu: HashMap<i32, (u64, u64)> = services()
        .into_iter()
        .map(|(pid, u)| (pid, (u.cpu_ns, u.cpu_ns)))
        .collect();
    let started = Instant::now();
    let mut child = Command::new(bin)
        .arg("--export-worker")
        .arg(&job_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the klipolo binary");
    let pid = child.id() as i32;
    let stdout = child.stdout.take().unwrap();
    let lines = std::thread::spawn(move || {
        BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| parse_line(&l))
            .collect::<Vec<_>>()
    });
    let (mut worker, mut peak_vt) = (Usage::default(), 0u64);
    while child.try_wait().unwrap().is_none() {
        if let Some(u) = usage(pid) {
            worker = u;
        }
        let now = services();
        peak_vt = peak_vt.max(now.iter().map(|(_, u)| u.footprint).sum());
        for (pid, u) in now {
            cpu.entry(pid).or_insert((0, 0)).1 = u.cpu_ns;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let wall = started.elapsed().as_secs_f64();
    let lines = lines.join().unwrap();
    let ok = matches!(lines.last(), Some(WorkerLine::Done { .. }));
    let vt_cpu = cpu.values().map(|(a, b)| b.saturating_sub(*a)).sum::<u64>() as f64 / 1e9;
    let worker_cpu = worker.cpu_ns as f64 / 1e9;
    let mb_out: f64 = job
        .outputs
        .iter()
        .map(|o| std::fs::metadata(job.path(&o.file)).map_or(0, |m| m.len()) as f64)
        .sum::<f64>()
        / 1e6;
    println!(
        "{quality:?} × {} format(s), {} s segments: {} in {wall:.1} s = {:.2}× realtime; CPU worker {worker_cpu:.1} s ({:.2} cores) + VT services {vt_cpu:.1} s ({:.2} cores); peak footprint worker {:.0} MB, VT services {:.0} MB; {mb_out:.0} MB written",
        formats.len(),
        segment_s(),
        if ok { "done" } else { "FAILED" },
        seconds / wall,
        worker_cpu / wall,
        vt_cpu / wall,
        mb(worker.peak_footprint),
        mb(peak_vt),
    );
    let _ = std::fs::remove_dir_all(&job.dir);
}

pub fn run(args: &[String]) {
    let bin = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().expect("this binary"));
    let dir = tempfile::tempdir().unwrap();
    let all = [
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ];
    for quality in [ExportQuality::Standard, ExportQuality::High] {
        run_one(&bin, dir.path(), &all[..1], quality);
        run_one(&bin, dir.path(), &all, quality);
    }
}

/// `export-case layers <quality> <main video> <video>...`: a 2-minute portrait short exported, the main video over its
/// blurred picture with four cropped, moving video layers (`layers::project`), Word pop captions three words a second
/// and a title entering every 10 s. `export-case scattered <quality> <2-hour video>`: what keeping 50 scattered 2 s
/// ranges of a 2-hour file leaves (a seek at every clip), exported in landscape. `<quality>`: standard, high, maximum
/// or master. `--seconds <s>` sets the short's length (120); `--write <file>` writes the project there (and its words
/// beside it, `<file>.words.json`) instead of exporting it.
pub fn case(args: &[String]) {
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let write = flag("--write");
    let length: Option<f64> = flag("--seconds").and_then(|s| s.parse().ok());
    let args: Vec<String> = args
        .iter()
        .scan(false, |value, a| {
            let keep = !*value && !a.starts_with("--");
            *value = a.starts_with("--");
            Some(keep.then(|| a.clone()))
        })
        .flatten()
        .collect();
    let (Some(kind), Some(quality), Some(main)) = (args.first(), args.get(1), args.get(2)) else {
        eprintln!(
            "usage: export-case layers <quality> <main> <video>... | export-case scattered <quality> <2-hour video>"
        );
        std::process::exit(2);
    };
    let quality: ExportQuality =
        serde_json::from_value(serde_json::Value::String(quality.clone())).expect("a quality");
    let bin = std::env::current_exe().expect("this binary");
    let dir = tempfile::tempdir().unwrap();
    let (p, speech, formats, seconds) = match kind.as_str() {
        "layers" => {
            let seconds = length.unwrap_or(120.0);
            let (p, speech) = text_heavy(
                super::layers::project(main, &args[3..], 4, seconds),
                seconds,
            );
            (p, speech, [FormatKind::Portrait], seconds)
        }
        "scattered" => {
            let p = scattered(main);
            let seconds = 50.0 * 2.0;
            (p, Speech::default(), [FormatKind::Landscape], seconds)
        }
        other => panic!("unknown case {other:?}"),
    };
    if let Some(file) = write {
        std::fs::write(&file, serde_json::to_vec(&p).expect("JSON")).expect("writes the project");
        if let Some(t) = speech.transcripts.get("main") {
            std::fs::write(
                format!("{file}.words.json"),
                serde_json::to_vec(t.as_ref()).expect("JSON"),
            )
            .expect("writes the words");
        }
        println!("wrote {file}");
        return;
    }
    println!("export-case {kind}: {seconds:.0} s");
    run_project(&bin, dir.path(), p, speech, &formats, quality, seconds);
}

/// `p` with Word pop captions over its main video (three words a second, a pause every eight) and a 3 s title entering
/// every 10 s.
fn text_heavy(mut p: Project, seconds: f64) -> (Project, Speech) {
    const SAID: [&str; 8] = ["this", "is", "how", "we", "cut", "a", "faster", "short"];
    let words: Vec<Word> = (0..(seconds * 3.0) as usize)
        .filter(|i| i % 8 != 7)
        .map(|i| {
            let at = i as f64 / 3.0;
            Word {
                text: format!("{}{}", SAID[i % 8], if i % 8 == 6 { "!" } else { "" }),
                start_us: secs(at),
                end_us: secs(at + 0.3),
                confidence: 0.9,
                kind: WordKind::Word,
            }
        })
        .collect();
    let speech = Speech {
        transcripts: HashMap::from([(
            "main".to_owned(),
            Arc::new(Transcript {
                key: "main".into(),
                language: "en".into(),
                model: "m".into(),
                covered_us: secs(seconds),
                complete: true,
                words,
            }),
        )]),
        ..Speech::default()
    };
    p.captions.enabled = true;
    p.captions.style = Some(ContribId::builtin("caption.word-pop"));
    let mut titles = Track::new(TrackKind::Overlay, "Titles");
    for k in 0..(seconds / 10.0) as usize {
        let mut layout = Layout::new(
            Anchor::TopLeft,
            BoxN {
                x: 0.08,
                y: 0.3,
                w: 0.84,
                h: 0.12,
            },
        );
        layout.motion = Some(Motion {
            enter: Some(super::project::preset("fade")),
            exit: Some(super::project::preset("slide")),
            ..Motion::default()
        });
        titles.items.push(Item {
            id: format!("title-{k}"),
            start_us: secs(k as f64 * 10.0 + 1.0),
            duration_us: secs(3.0),
            attach: None,
            layout,
            content: ItemContent::Text {
                text: format!("Step {}: keep it short", k + 1),
                style: TextStyle {
                    size: 72.0,
                    ..TextStyle::default()
                },
            },
            effects: Vec::new(),
        });
    }
    p.timeline.tracks.push(titles);
    (
        klipolo_model::apply_ops(&p, &[]).expect("a checked project"),
        speech,
    )
}

/// The 2-hour `video` after keeping 50 scattered 2 s ranges: 50 clips, each from its own place in the file.
fn scattered(video: &str) -> Project {
    let mut p = Project::new("Scattered");
    p.assets = vec![super::layers::asset("talk", video)];
    let length = p.assets[0].duration_us;
    p.timeline.main = (0..50)
        .map(|k| {
            // Out of order, so each clip seeks: 37 is coprime with 50.
            let at = length / 50 * ((k * 37) % 50) + secs(13.0);
            let mut c = Clip::new(ClipSource::Asset { id: "talk".into() }, at, at + secs(2.0));
            c.id = format!("kept-{k}");
            c
        })
        .collect();
    klipolo_model::apply_ops(&p, &[]).expect("a checked project")
}
