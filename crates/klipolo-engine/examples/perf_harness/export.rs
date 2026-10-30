//! Export scenario: a 60 s 1080p30 project (`screen-60s-1080p30.mp4`)
//! exported by a real worker process (`--export-worker`, as the app runs it) in one format and in all three,
//! High, in segments of `KLIPOLO_EXPORT_SEGMENT_S` seconds (default 30). Reports wall time, the
//! realtime factor, CPU (the worker plus VideoToolbox's decoder and encoder services, which do their work out of
//! process) and peak footprints.
//!
//! `cargo run -p klipolo-engine --example perf_harness -- export [<worker binary>]` (default: this harness, which runs
//! the worker for `--export-worker`); `… -- export-job <dir> <standard|high|maximum|master> <hardware|softwareSafe>
//! <seconds> <landscape|portrait|square>…` plans a job (files in `<dir>`, the job in `<dir>/.jobs/<id>`) and prints
//! its manifest's path for running the worker by hand.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use klipolo_engine::api::ExportQuality;
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::naming::plan_files;
use klipolo_engine::export::protocol::{WorkerLine, parse_line};
use klipolo_engine::export::runner::Batch;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::timeline::{Clip, ClipSource};

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
    let p = project();
    let files = plan_files(&p, formats, quality, dir);
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
        60.0 / wall,
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
