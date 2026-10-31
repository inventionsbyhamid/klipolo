//! Export on a Mac, end to end: the app's start and run (`klipolo_app::export`) with the real worker, this package's own
//! binary run as `--export-worker`, drawing on the GPU and encoding with VideoToolbox. Mac only; run it under the Mac
//! build lock after `scripts/gen-engine-fixtures.sh`:
//!   lockf -k /tmp/klipolo-mac-build.lock cargo test -p klipolo-app --test export_mac
//! `ffprobe` (any ffmpeg CLI) reads the files back.
#![cfg(target_os = "macos")]

#[path = "../../klipolo-engine/tests/support/fixture_set.rs"]
mod fixture_set;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use klipolo_app::export::{
    ExportState, StartSteps, Started, create_job, freeze, run, start_on_slot,
};
use klipolo_engine::api::{ExportEvent, ExportQuality, ExportRequest};
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::runner::{Batch, Outcome, ProcessLauncher, discard_job};
use klipolo_engine::render::probe::probe_media;
use klipolo_model::coded::Said;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::speech::Speech;
use klipolo_model::time::secs_to_us;
use klipolo_model::timeline::{Clip, ClipSource};

/// The 4 s 1080p30 clip with a beep each second.
fn source() -> PathBuf {
    let file = fixture_set::fixture_dir().join("bars-30fps-0.5gop.mp4");
    assert!(
        file.is_file(),
        "make the test media first: scripts/gen-engine-fixtures.sh"
    );
    file
}

/// "Rosa studio visit": the fixture clip `clips` times in a row.
fn project(clips: usize) -> Project {
    let path = source();
    let info = probe_media(&path).expect("the fixture probes");
    let asset = Asset {
        id: "bars".into(),
        kind: AssetKind::Video,
        path: path.display().to_string(),
        name: "bars.mp4".into(),
        duration_us: secs_to_us(info.duration),
        width: info.width,
        height: info.height,
        fps: info.fps,
        rotation: Some(info.rotation),
        has_audio: info.has_audio,
        audio_lead_us: Some(info.audio_lead_us),
    };
    let mut p = Project::new("Rosa studio visit");
    p.timeline.main = (0..clips)
        .map(|_| {
            Clip::new(
                ClipSource::Asset { id: "bars".into() },
                0,
                asset.duration_us,
            )
        })
        .collect();
    p.assets = vec![asset];
    klipolo_model::apply_ops(&p, &[]).expect("a checked project")
}

/// The app's side of a start for `project`, exporting into `dir` (no save to wait for: it is saved nowhere).
struct Steps {
    project: Arc<Project>,
    dir: PathBuf,
}

impl StartSteps for Steps {
    fn open(&self) -> Option<(String, String)> {
        Some((self.project.id.clone(), self.project.name.clone()))
    }

    fn prepare(&self, req: &ExportRequest) -> impl Future<Output = Result<Batch, Said>> + Send {
        let batch = freeze(
            self.project.clone(),
            &Speech::default(),
            "en".into(),
            req,
            &self.dir,
            None,
        );
        async move { Ok(batch) }
    }

    fn create(&self, batch: Batch) -> impl Future<Output = Result<ExportJob, Said>> + Send {
        create_job(batch)
    }

    fn drop_new(&self, job: &ExportJob) {
        discard_job(&job.dir).unwrap();
    }
}

fn portrait() -> ExportRequest {
    ExportRequest {
        formats: vec![FormatKind::Portrait],
        quality: ExportQuality::High,
        caption_file: None,
        allow_stand_ins: false,
        replaces: None,
        allow_missing_cursor: false,
    }
}

/// Exports `project` into `dir` as the app does: the slot, the job, then the real worker. `on_event` sees each event
/// (and may cancel through the state). The outcome and every event, in order.
async fn export(
    state: &ExportState,
    project: Project,
    dir: &Path,
    on_event: impl Fn(&ExportState, &ExportEvent) + Send + Sync,
) -> (Outcome, Vec<ExportEvent>) {
    klipolo_engine::render::fonts::configure_for_tests();
    let steps = Steps {
        project: Arc::new(project),
        dir: dir.to_path_buf(),
    };
    let Started::Run(job, cancel) = start_on_slot(state, &portrait(), &steps)
        .await
        .expect("it starts")
    else {
        panic!("it runs");
    };
    let launcher = ProcessLauncher::export(PathBuf::from(env!("CARGO_BIN_EXE_klipolo-app")));
    let heard = Mutex::new(Vec::new());
    let emit = |event: ExportEvent| {
        state.record(&event);
        on_event(state, &event);
        heard.lock().unwrap().push(event);
    };
    let outcome = run(&launcher, *job, cancel, &emit).await;
    state.release();
    (outcome, heard.into_inner().unwrap())
}

/// `ffprobe`'s word on the first video stream of `file`: codec, width, height and the file's duration.
fn ffprobe(file: &Path) -> (String, u32, u32, f64) {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height:format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(file)
        .output()
        .expect("ffprobe runs (any ffmpeg CLI)");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name} in {text}"))
            .to_string()
    };
    (
        field("codec_name"),
        field("width").parse().unwrap(),
        field("height").parse().unwrap(),
        field("duration").parse().unwrap(),
    )
}

/// The files directly in `dir`, by name (the job folder `.jobs` is a folder).
fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Jobs still in `dir/.jobs` (none once a run has ended).
fn jobs_in(dir: &Path) -> usize {
    std::fs::read_dir(dir.join(".jobs")).map_or(0, |d| d.count())
}

/// `jobs_in` once a cancelled job's parts are deleted: Cancel sets its folder aside at once (out of every list) and
/// deletes it on a thread of its own, so it goes moments later. Waits 10 s at most.
async fn jobs_left(dir: &Path) -> usize {
    for _ in 0..100 {
        if jobs_in(dir) == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    jobs_in(dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn exports_h264_mp4_for_portrait() {
    let dir = tempfile::tempdir().unwrap();
    let state = ExportState::default();
    let (outcome, events) = export(&state, project(1), dir.path(), |_, _| {}).await;
    assert!(
        matches!(outcome, Outcome::Finished(_)),
        "{outcome:?}: {events:?}"
    );
    assert_eq!(files_in(dir.path()), ["Rosa studio visit 9x16.mp4"]);
    let file = dir.path().join("Rosa studio visit 9x16.mp4");
    let (codec, width, height, duration) = ffprobe(&file);
    assert_eq!((codec.as_str(), width, height), ("h264", 1080, 1920));
    assert!((duration - 4.0).abs() < 0.1, "the clip's 4 s: {duration}");
    assert_eq!(jobs_in(dir.path()), 0, "no job left");
    assert!(!dir.path().join(".jobs").exists(), "nor an empty .jobs");
    assert!(events.contains(&ExportEvent::FileReady {
        index: 0,
        path: file.display().to_string()
    }));
    assert_eq!(state.files(), [file], "Show in Finder reaches it");
}

#[tokio::test(flavor = "multi_thread")]
async fn existing_file_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let earlier = dir.path().join("Rosa studio visit 9x16.mp4");
    std::fs::write(&earlier, b"an earlier export").unwrap();
    let state = ExportState::default();
    let (outcome, _) = export(&state, project(1), dir.path(), |_, _| {}).await;
    assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
    assert_eq!(
        files_in(dir.path()),
        ["Rosa studio visit 9x16 2.mp4", "Rosa studio visit 9x16.mp4"]
    );
    assert_eq!(
        std::fs::read(&earlier).unwrap(),
        b"an earlier export",
        "left as it was"
    );
    assert_eq!(
        ffprobe(&dir.path().join("Rosa studio visit 9x16 2.mp4")).0,
        "h264"
    );
    // Exported again, the next free name.
    let (outcome, _) = export(&state, project(1), dir.path(), |_, _| {}).await;
    assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
    assert!(dir.path().join("Rosa studio visit 9x16 3.mp4").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_removes_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let state = ExportState::default();
    // 20 s: still rendering when its first frames are in, which is when Cancel export is pressed (naming the job, as
    // the dialog does).
    let job = Mutex::new(None::<String>);
    let (outcome, events) = export(&state, project(5), dir.path(), |state, event| match event {
        ExportEvent::Started { job_id, .. } => *job.lock().unwrap() = Some(job_id.clone()),
        ExportEvent::Progress { frame, .. } if *frame > 0 => {
            let job = job.lock().unwrap().clone();
            state.cancel_for(job.as_deref(), None).unwrap();
        }
        _ => {}
    })
    .await;
    assert!(
        matches!(outcome, Outcome::Cancelled),
        "{outcome:?}: {events:?}"
    );
    assert_eq!(events.last(), Some(&ExportEvent::Cancelled));
    assert!(
        files_in(dir.path()).is_empty(),
        "no file: {:?}",
        files_in(dir.path())
    );
    assert_eq!(jobs_left(dir.path()).await, 0, "no job or part of one left");
    assert!(!dir.path().join(".jobs").exists(), "nor an empty .jobs");
    assert!(state.files().is_empty());
}
