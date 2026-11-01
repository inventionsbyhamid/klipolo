mod support;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_engine::api::Said;
use klipolo_engine::api::{
    ExportCompatibility, ExportEvent, ExportJobState, ExportQuality, PlannedFile, ResumeMode,
};
use klipolo_engine::export::job::{ExportJob, ExportMode, JobState, PartStatus};
use klipolo_engine::export::jobs::{jobs_root, list_jobs};
use klipolo_engine::export::naming::{plan_files, sanitize};
use klipolo_engine::export::quality::{
    AudioCodec, Container, EncoderCaps, VideoCodec, encode_settings, output_size,
};
use klipolo_engine::export::runner::{
    Attempt, Batch, DECODE_FAILED, DISK_FULL, ENCODER_BUSY, Launcher, Outcome, resume_job,
    resume_job_with, run_batch, run_batch_with,
};
use klipolo_model::project::FormatKind;
use support::*;
use tokio::sync::{mpsc, oneshot, watch};

/// What a scripted worker does: its lines, whether it succeeds and whether it hangs (until killed or paused).
type Script = Box<dyn Fn(&ExportJob) -> (Vec<String>, bool, bool) + Send + Sync>;

/// A scripted worker: `script(job)` decides what it prints, writes and whether it succeeds.
struct Fake {
    launches: AtomicU32,
    jobs: Mutex<Vec<ExportJob>>,
    script: Script,
    /// A pause makes the worker stop at once (else it keeps winding down until killed).
    pause_stops: bool,
}

impl Fake {
    fn new(script: Script) -> Self {
        Self {
            launches: AtomicU32::new(0),
            jobs: Mutex::new(Vec::new()),
            script,
            pause_stops: true,
        }
    }

    fn modes(&self) -> Vec<ExportMode> {
        self.jobs.lock().unwrap().iter().map(|j| j.mode).collect()
    }
}

impl Launcher for Fake {
    fn launch(&self, job_path: &Path) -> Result<Attempt, String> {
        let job = ExportJob::open(job_path.parent().unwrap())?;
        self.jobs.lock().unwrap().push(job.clone());
        let (lines, success, hang) = (self.script)(&job);
        let (tx, rx) = mpsc::unbounded_channel();
        for l in lines {
            tx.send(l).unwrap();
        }
        let (exit_tx, exit) = oneshot::channel();
        let held = Arc::new(Mutex::new(Some((tx, exit_tx))));
        if !hang {
            let (tx, exit_tx) = held.lock().unwrap().take().unwrap();
            drop(tx);
            exit_tx.send(success).unwrap();
        }
        let for_pause = held.clone();
        let pause_stops = self.pause_stops;
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(Attempt {
            lines: rx,
            exit,
            kill: Box::new(move || {
                if let Some((tx, exit_tx)) = held.lock().unwrap().take() {
                    drop(tx);
                    let _ = exit_tx.send(false);
                }
            }),
            pause: Box::new(move || {
                if !pause_stops {
                    return;
                }
                if let Some((tx, exit_tx)) = for_pause.lock().unwrap().take() {
                    tx.send(r#"{"type":"paused"}"#.into()).unwrap();
                    drop(tx);
                    let _ = exit_tx.send(true);
                }
            }),
        })
    }
}

/// What a worker does when a segment is finished: its file, and its record in the manifest.
fn finish_segment(job: &ExportJob, output: usize, index: usize) {
    let mut job = ExportJob::open(&job.dir).unwrap();
    let s = &mut job.outputs[output].segments[index];
    std::fs::write(job.dir.join(&s.file), b"segment").unwrap();
    s.status = PartStatus::Done;
    job.save().unwrap();
}

/// A worker that finishes: joined files in the job folder, progress, done.
fn ok_lines(job: &ExportJob) -> Vec<String> {
    let mut lines = vec![
        r#"{"type":"progress","frame":900,"totalFrames":1800,"fps":60.0,"segmentsDone":1,"segmentsTotal":2}"#
            .to_string(),
        r#"{"type":"sound","doneS":4.0,"totalS":30.0}"#.to_string(),
        r#"{"type":"finishing"}"#.to_string(),
    ];
    for o in &job.outputs {
        std::fs::write(job.path(&o.file), b"video").unwrap();
        lines.push(
            serde_json::json!({ "type": "formatDone", "format": o.format, "tempPath": job.path(&o.file), "bytes": 5 })
                .to_string(),
        );
    }
    lines.push(r#"{"type":"done","elapsedS":1.0}"#.into());
    lines
}

fn error_line(code: &str, message: &str, retryable: bool) -> String {
    serde_json::json!({ "type": "error", "code": code, "message": message, "retryable": retryable })
        .to_string()
}

fn batch(dir: &Path) -> (Batch, Vec<PlannedFile>) {
    let formats = [FormatKind::Landscape, FormatKind::Portrait];
    let files = plan_files(&project(), &formats, ExportQuality::High, dir);
    let b = Batch {
        project: project(),
        fps: 30,
        quality: ExportQuality::High,
        files: files.clone(),
        pcm_dir: None,
        speech: Arc::default(),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor: false,
    };
    (b, files)
}

type Events = Arc<Mutex<Vec<ExportEvent>>>;

fn recorder() -> (Events, impl Fn(ExportEvent) + Send + Sync) {
    let events: Events = Arc::default();
    let sink = events.clone();
    (events, move |e| sink.lock().unwrap().push(e))
}

/// `run` in 10 s segments (three per format of the 30 s fixture).
async fn run_short(fake: &Fake, b: Batch) -> (Outcome, Vec<ExportEvent>) {
    let (events, emit) = recorder();
    let outcome = run_batch_with(fake, b, 10, watch::channel(false).1, &emit).await;
    let events = events.lock().unwrap().clone();
    (outcome, events)
}

async fn run(fake: &Fake, b: Batch, cancel: watch::Receiver<bool>) -> (Outcome, Vec<ExportEvent>) {
    let (events, emit) = recorder();
    let outcome = run_batch(fake, b, cancel, &emit).await;
    let events = events.lock().unwrap().clone();
    (outcome, events)
}

/// The one job folder in `dir/.jobs`.
fn the_job(dir: &Path) -> ExportJob {
    let jobs = list_jobs(&jobs_root(dir));
    assert_eq!(jobs.len(), 1, "one job folder");
    jobs.into_iter().next().unwrap().unwrap()
}

#[test]
fn names_are_sanitized_and_never_collide() {
    assert_eq!(sanitize("  a/b:c\u{7}.  "), "a-b-c");
    assert_eq!(sanitize(" ... "), "Untitled video");
    assert_eq!(sanitize(&"é".repeat(300)).chars().count(), 100);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Engine fixture (Vertical).mp4"), b"x").unwrap();
    let all = [
        FormatKind::Landscape,
        FormatKind::Portrait,
        FormatKind::Square,
    ];
    let files = plan_files(&project(), &all, ExportQuality::High, dir.path());
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Engine fixture (Landscape).mp4",
            "Engine fixture (Vertical) 2.mp4",
            "Engine fixture (Square).mp4"
        ]
    );
    assert_eq!((files[1].width, files[1].height), (1080, 1920));
    // Master is ProRes in a .mov (and the .mp4 taken above doesn't block the .mov name).
    let master = plan_files(&project(), &all, ExportQuality::Master, dir.path());
    assert_eq!(master[1].name, "Engine fixture (Vertical).mov");
}

#[test]
fn quality_sizes_follow_sources() {
    let mut p = project();
    assert_eq!(output_size(&p, FormatKind::Landscape), (1920, 1080));
    let screen = p.assets.iter_mut().find(|a| a.id == "screen").unwrap();
    (screen.width, screen.height) = (3840, 2160);
    assert_eq!(output_size(&p, FormatKind::Landscape), (3840, 2160));
    assert_eq!(output_size(&p, FormatKind::Portrait), (1080, 1920));
    assert_eq!(output_size(&p, FormatKind::Square), (1080, 1080));
    for a in p.assets.iter_mut() {
        (a.width, a.height) = (1280, 720);
    }
    assert_eq!(
        output_size(&p, FormatKind::Landscape),
        (1920, 1080),
        "never below 1080p"
    );
}

#[test]
fn quality_settings_per_preset() {
    let all = EncoderCaps {
        h264: true,
        hevc_main10: true,
        prores: true,
        prores_hardware: false,
    };
    let standard = encode_settings(ExportQuality::Standard, (1920, 1080), 30, &all);
    let high = encode_settings(ExportQuality::High, (1920, 1080), 30, &all);
    assert_eq!(
        (standard.bitrate, standard.b_frames, standard.gop),
        (8_000_000, 0, 60)
    );
    assert_eq!(
        (high.bitrate, high.max_bitrate, high.b_frames),
        (14_000_000, 21_000_000, 2)
    );
    assert_eq!(
        encode_settings(ExportQuality::Standard, (3840, 2160), 60, &all).bitrate,
        60_000_000
    );
    let maximum = encode_settings(ExportQuality::Maximum, (1920, 1080), 30, &all);
    assert_eq!(
        (maximum.codec, maximum.bitrate),
        (VideoCodec::HevcMain10, 24_000_000)
    );
    let no_hevc = EncoderCaps {
        hevc_main10: false,
        ..all
    };
    let fallback = encode_settings(ExportQuality::Maximum, (1920, 1080), 30, &no_hevc);
    assert_eq!(
        (fallback.codec, fallback.bitrate),
        (VideoCodec::H264High, 24_000_000)
    );
    let master = encode_settings(ExportQuality::Master, (1920, 1080), 30, &all);
    assert_eq!(
        (master.codec, master.container, master.audio),
        (VideoCodec::ProRes422Hq, Container::Mov, AudioCodec::Pcm24)
    );
}

#[tokio::test]
async fn progress_is_forwarded_and_files_renamed_into_place() {
    let dir = tempfile::tempdir().unwrap();
    let (b, files) = batch(dir.path());
    let fake = Fake::new(Box::new(|job| (ok_lines(job), true, false)));
    let (outcome, events) = run(&fake, b, watch::channel(false).1).await;
    assert!(
        matches!(outcome, Outcome::Finished(ref p) if p.len() == 2),
        "{events:?}"
    );
    // 30 s at 30 fps in two formats: 1800 frames, one 30 s segment each.
    assert!(matches!(
        events.first(),
        Some(ExportEvent::Started {
            total_frames: 1800,
            segments_total: 2,
            ..
        })
    ));
    assert!(events.contains(&ExportEvent::Progress {
        frame: 900,
        total_frames: 1800,
        eta_s: None,
        segments_done: 1,
        segments_total: 2,
    }));
    // Once, whether the sound or the join comes first.
    let finishing = events.iter().filter(|e| **e == ExportEvent::Finishing);
    assert_eq!(finishing.count(), 1);
    assert!(matches!(events.last(), Some(ExportEvent::Finished { .. })));
    for f in &files {
        assert_eq!(std::fs::read(&f.path).unwrap(), b"video", "{}", f.path);
    }
    assert!(
        list_jobs(&jobs_root(dir.path())).is_empty(),
        "job folder removed"
    );
}

#[tokio::test]
async fn a_failure_continues_in_fresh_hardware_then_software_safe() {
    let dir = tempfile::tempdir().unwrap();
    // Nothing finished in hardware yet: falling back to the software encoder loses nothing.
    let fake = Fake::new(Box::new(|job| match job.mode {
        ExportMode::Hardware => (
            vec![error_line("encoderUnavailable", "VT busy", true)],
            false,
            false,
        ),
        ExportMode::SoftwareSafe => {
            assert!(job.outputs.iter().all(|o| o.parts_to_redo() == 0));
            (ok_lines(job), true, false)
        }
    }));
    let (outcome, events) = run(&fake, batch(dir.path()).0, watch::channel(false).1).await;
    assert!(matches!(outcome, Outcome::Finished(_)), "{events:?}");
    let retries = events.iter().filter(|e| matches!(e, ExportEvent::Retrying));
    assert_eq!(retries.count(), 2);
    use ExportMode::*;
    assert_eq!(fake.modes(), [Hardware, Hardware, SoftwareSafe]);
    let jobs = fake.jobs.lock().unwrap();
    assert!(jobs.iter().all(|j| j.job_id == jobs[0].job_id), "one job");
}

#[tokio::test]
async fn a_busy_encoder_pauses_rather_than_discard_hardware_segments() {
    let dir = tempfile::tempdir().unwrap();
    let busy = || vec![error_line("encoderUnavailable", "VT busy", true)];
    let fake = Fake::new(Box::new(move |job| {
        if job.outputs[0].segments[0].status != PartStatus::Done {
            finish_segment(job, 0, 0);
        }
        (busy(), false, false)
    }));
    let (outcome, events) = run_short(&fake, batch(dir.path()).0).await;
    assert!(matches!(outcome, Outcome::Paused), "{events:?}");
    // Compatibility mode would redo the one finished hardware part.
    let compatibility = Some(ExportCompatibility {
        parts_to_redo: 1,
        chosen: false,
    });
    assert!(events.contains(&ExportEvent::Paused {
        message: Some(ENCODER_BUSY.into()),
        compatibility,
    }));
    // One retry (fresh hardware), and no "Retrying" for the step that pauses instead.
    let retries = events.iter().filter(|e| matches!(e, ExportEvent::Retrying));
    assert_eq!(retries.count(), 1);
    use ExportMode::*;
    assert_eq!(
        fake.modes(),
        [Hardware, Hardware],
        "never the software encoder"
    );
    let job = the_job(dir.path());
    assert_eq!(job.outputs[0].segments[0].status, PartStatus::Done);
    assert_eq!(job.mode, Hardware);

    // The encoder is free again: Resume continues in hardware from the finished segment.
    let resumed = Fake::new(Box::new(|job| (ok_lines(job), true, false)));
    let outcome = resume_job(&resumed, &job.dir, watch::channel(false).1, &|_| {}).await;
    assert!(matches!(outcome, Outcome::Finished(_)));
    let seen = resumed.jobs.lock().unwrap()[0].clone();
    assert_eq!(seen.mode, Hardware);
    assert_eq!(seen.outputs[0].segments[0].status, PartStatus::Done);
}

/// A hardware failure that keeps happening (a clip VideoToolbox can't decode) after a hardware part is done: it
/// pauses saying so, and the user can finish in compatibility mode, which then completes.
#[tokio::test]
async fn a_repeating_hardware_failure_can_finish_in_compatibility_mode() {
    let dir = tempfile::tempdir().unwrap();
    let decode = || vec![error_line("decode", "intro.mp4 couldn't be decoded", true)];
    let fake = Fake::new(Box::new(move |job| {
        if job.outputs[0].segments[0].status != PartStatus::Done {
            finish_segment(job, 0, 0);
        }
        (decode(), false, false)
    }));
    let (outcome, events) = run_short(&fake, batch(dir.path()).0).await;
    assert!(matches!(outcome, Outcome::Paused), "{events:?}");
    assert!(
        matches!(events.last(), Some(ExportEvent::Paused { message: Some(Said::Text(m)), compatibility: Some(_) })
        if m == DECODE_FAILED)
    );
    let job = the_job(dir.path());
    let info = job.info(false, false);
    assert_eq!(
        info.compatibility,
        Some(ExportCompatibility {
            parts_to_redo: 1,
            chosen: false,
        })
    );

    // Resume as before would meet the same failure; compatibility mode finishes (software encoder).
    let resumed = Fake::new(Box::new(|job| match job.mode {
        ExportMode::SoftwareSafe => (ok_lines(job), true, false),
        ExportMode::Hardware => unreachable!("compatibility mode never tries hardware"),
    }));
    let outcome = resume_job_with(
        &resumed,
        &job.dir,
        ResumeMode::Compatible,
        watch::channel(false).1,
        &|_| {},
    )
    .await;
    assert!(matches!(outcome, Outcome::Finished(_)));
    assert_eq!(resumed.modes(), [ExportMode::SoftwareSafe]);
}

/// Runs `steps[n]` on the worker's n-th launch.
fn in_turn(steps: Vec<Script>) -> Script {
    let n = AtomicUsize::new(0);
    Box::new(move |job| steps[n.fetch_add(1, Ordering::SeqCst)](job))
}

/// Every part of every format finished in hardware, then the join fails (retryable).
fn complete_then_fail_join() -> Script {
    Box::new(|job| {
        for (o, output) in job.outputs.iter().enumerate() {
            (0..output.segments.len()).for_each(|s| finish_segment(job, o, s));
        }
        (
            vec![error_line("internal", "joining failed", true)],
            false,
            false,
        )
    })
}

fn fail_retryable() -> Script {
    Box::new(|_| (vec![error_line("encode", "VT busy", true)], false, false))
}

/// A run that finishes, and only in compatibility mode.
fn ok_software() -> Script {
    Box::new(|job| {
        assert_eq!(job.mode, ExportMode::SoftwareSafe);
        (ok_lines(job), true, false)
    })
}

/// The user pauses the run before it has verified a part.
fn pause_before_first_part() -> Script {
    Box::new(|_| (vec![r#"{"type":"paused"}"#.into()], true, false))
}

/// Records `parts` of format `output` as written by `encoder` (its config CRC, software or not).
fn finish_parts(
    handle: &klipolo_engine::export::shared::JobHandle,
    output: usize,
    parts: &[u32],
    encoder: (u32, bool),
) {
    for &i in parts {
        handle.segment_done(output, i, (1, 1), encoder).unwrap();
    }
}

/// A job in `dir` paused after hardware failures, with one finished hardware part, as `paused` leaves it; then
/// resumed in `mode`.
async fn run_resume(fake: &Fake, dir: &Path, mode: ResumeMode) -> (Outcome, Vec<ExportEvent>) {
    let mut job = ExportJob::create(&batch(dir).0, 10).unwrap();
    job.outputs[0].segments[0].status = PartStatus::Done;
    (job.state, job.offer_compatible) = (JobState::Paused, true);
    job.save().unwrap();
    let (events, emit) = recorder();
    let outcome = resume_job_with(fake, &job.dir, mode, watch::channel(false).1, &emit).await;
    let events = events.lock().unwrap().clone();
    (outcome, events)
}

#[test]
fn a_redone_part_is_taken_off_the_progress_at_once() {
    // Two hardware parts are done, one of them the short last part; a software part verifies: both are set back, and
    // segment_done says how many frames, so the bar steps back by that much now rather than overshooting until the
    // next round.
    use klipolo_engine::export::shared::JobHandle;
    let dir = tempfile::tempdir().unwrap();
    // 30 s in 7 s parts: four of 210 frames and a last one of 60.
    let handle = JobHandle::new(ExportJob::create(&batch(dir.path()).0, 7).unwrap());
    finish_parts(&handle, 0, &[0, 4], (7, false));
    let set_back = handle.read(|j| {
        [0, 4]
            .iter()
            .map(|&i| j.outputs[0].segments[i].frames)
            .sum::<u64>()
    });
    assert_eq!(set_back, 270);
    let redone = handle.segment_done(0, 2, (100, 1), (9, true)).unwrap();
    assert_eq!(
        redone, set_back,
        "the frames of the parts set back, each part's own length"
    );
}

#[tokio::test]
async fn a_failure_with_nothing_to_redo_moves_on_to_compatibility_mode_by_itself() {
    // Every hardware part is done and the format is complete, but not joined: the automatic software run loses
    // nothing, so no pause and no offer (without finished parts to redo, SoftwareSafe runs automatically).
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(in_turn(vec![
        complete_then_fail_join(),
        fail_retryable(),
        ok_software(),
    ]));
    let (outcome, events) = run_short(&fake, batch(dir.path()).0).await;
    assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ExportEvent::Paused { .. }))
    );
    use ExportMode::*;
    assert_eq!(fake.modes(), [Hardware, Hardware, SoftwareSafe]);
}

#[tokio::test]
async fn pausing_a_compatibility_run_keeps_the_offer() {
    // The user chose "Finish in compatibility mode", then paused before its first verified part: the paused job still
    // offers compatibility mode, and resuming normally continues in it.
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(in_turn(vec![pause_before_first_part()]));
    let (outcome, _) = run_resume(&fake, dir.path(), ResumeMode::Compatible).await;
    assert!(matches!(outcome, Outcome::Paused), "{outcome:?}");
    assert_eq!(fake.modes(), [ExportMode::SoftwareSafe]);
    let job = the_job(dir.path());
    assert!(job.offer_compatible && job.chosen_compatible);
    // Shown as chosen: the UI offers the one action that continues in it.
    assert!(
        job.info(false, false)
            .compatibility
            .is_some_and(|c| c.chosen)
    );

    let resumed = Fake::new(ok_software());
    let outcome = resume_job(&resumed, &job.dir, watch::channel(false).1, &|_| {}).await;
    assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
    assert_eq!(resumed.modes(), [ExportMode::SoftwareSafe]);
}

/// The worker draws from the fonts folder the app records in the job: at creation, and again when a paused
/// job resumes in an app that has moved since (the recorded folder gone), so a resume never draws every text as a
/// stand-in.
#[tokio::test]
async fn the_worker_draws_from_the_fonts_folder_of_the_app_running_the_job() {
    use klipolo_engine::export::worker::font_dirs;
    use klipolo_engine::render::fonts::{FontDirs, dirs};
    let dir = tempfile::tempdir().unwrap();
    let mut job = ExportJob::create(&batch(dir.path()).0, 10).unwrap();
    let here = dirs().bundled;
    assert!(
        here.as_ref()
            .is_some_and(|d| d.join("Inter/Inter-Regular.ttf").is_file())
    );
    assert_eq!(job.fonts_dir, here);
    assert_eq!(
        font_dirs(&ExportJob::open(&job.dir).unwrap()),
        FontDirs {
            bundled: here.clone()
        }
    );
    job.fonts_dir = Some(dir.path().join("an app that moved"));
    job.save().unwrap();
    let fake = Fake::new(pause_before_first_part());
    resume_job(&fake, &job.dir, watch::channel(false).1, &|_| {}).await;
    let started = fake.jobs.lock().unwrap()[0].clone();
    assert_eq!(font_dirs(&started), FontDirs { bundled: here });
}

/// A format's finished segments from one encoder stay finished while another encoder runs, until it has produced a
/// verified segment of its own; and a format mostly built by the software encoder keeps finishing with it.
#[test]
fn finished_segments_stay_until_a_new_encoder_has_verified_output() {
    use klipolo_engine::export::shared::JobHandle;
    let dir = tempfile::tempdir().unwrap();
    // 30 s in 5 s segments: six per format.
    let mut job = ExportJob::create(&batch(dir.path()).0, 5).unwrap();
    for s in job.outputs[0].segments.iter_mut().take(2) {
        (s.status, s.config, s.software) = (PartStatus::Done, Some(7), true);
    }
    assert!(
        job.outputs[0].built_in_software(),
        "keeps the software encoder"
    );
    assert!(
        !job.outputs[1].built_in_software(),
        "a new format starts in hardware"
    );
    let handle = JobHandle::new(job);
    // The same encoder: earlier segments kept.
    handle.segment_done(0, 2, (1, 1), (7, true)).unwrap();
    let statuses = |h: &JobHandle| {
        h.read(|j| {
            j.outputs[0]
                .segments
                .iter()
                .map(|s| s.status)
                .collect::<Vec<_>>()
        })
    };
    use PartStatus::*;
    assert_eq!(statuses(&handle)[..3], [Done, Done, Done]);
    // Another encoder's first verified segment: only now are the ones that can't join it rendered again.
    handle.segment_done(0, 3, (1, 1), (9, false)).unwrap();
    assert_eq!(statuses(&handle)[..4], [Pending, Pending, Pending, Done]);
}

#[tokio::test]
async fn missing_media_fails_without_retry_and_keeps_segments() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(Box::new(|job| {
        finish_segment(job, 0, 0);
        let message = "Some media is missing: intro.mp4";
        (
            vec![error_line("missingMedia", message, false)],
            false,
            false,
        )
    }));
    let (outcome, events) = run(&fake, batch(dir.path()).0, watch::channel(false).1).await;
    assert!(
        matches!(outcome, Outcome::Failed(Said::Text(ref m)) if m == "Some media is missing: intro.mp4")
    );
    assert!(!events.iter().any(|e| matches!(e, ExportEvent::Retrying)));
    assert_eq!(fake.launches.load(Ordering::SeqCst), 1);
    let job = the_job(dir.path());
    assert_eq!(job.state, JobState::Failed);
    assert!(
        job.dir.join(&job.outputs[0].segments[0].file).exists(),
        "segment kept"
    );
    let info = job.info(false, false);
    assert_eq!(
        (info.state, info.segments_done),
        (ExportJobState::Failed, 1)
    );
}

/// A file whose disk doesn't answer pauses the job with a message naming it (no retry, no
/// compatibility mode, which can't help), keeping its segments; Resume continues once the disk is back.
#[tokio::test]
async fn a_disk_that_doesnt_answer_pauses_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let (b, _) = batch(dir.path());
    let message = klipolo_engine::render::pool::not_answering("/Volumes/Archive/intro.mp4");
    let line = error_line("notAnswering", &message, false);
    let fake = Fake::new(Box::new(move |job| {
        finish_segment(job, 0, 0);
        (vec![line.clone()], false, false)
    }));
    let (outcome, events) = run(&fake, b, watch::channel(false).1).await;
    assert!(matches!(outcome, Outcome::Paused), "{events:?}");
    assert!(
        message.contains("intro.mp4") && message.contains("isn't responding"),
        "{message}"
    );
    assert!(events.contains(&ExportEvent::Paused {
        message: Some(Said::Text(message.clone())),
        compatibility: None,
    }));
    assert!(!events.iter().any(|e| matches!(e, ExportEvent::Retrying)));
    assert_eq!(fake.launches.load(Ordering::SeqCst), 1, "not run again");
    let job = the_job(dir.path());
    assert_eq!(
        (job.state, job.message),
        (JobState::Paused, Some(Said::Text(message)))
    );
    assert!(
        job.dir.join(&job.outputs[0].segments[0].file).exists(),
        "segment kept"
    );
}

#[tokio::test]
async fn disk_full_pauses_with_segments_kept_then_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let (b, files) = batch(dir.path());
    let fake = Fake::new(Box::new(|job| {
        finish_segment(job, 0, 0);
        (
            vec![error_line("diskFull", "disk full", false)],
            false,
            false,
        )
    }));
    let (outcome, events) = run(&fake, b, watch::channel(false).1).await;
    assert!(matches!(outcome, Outcome::Paused), "{events:?}");
    assert!(events.contains(&ExportEvent::Paused {
        message: Some(DISK_FULL.into()),
        compatibility: None,
    }));
    assert_eq!(
        fake.launches.load(Ordering::SeqCst),
        1,
        "no retry on a full disk"
    );
    let job = the_job(dir.path());
    assert_eq!(
        (job.state, job.message),
        (JobState::Paused, Some(Said::Text(DISK_FULL.into())))
    );
    assert!(
        job.dir.join(&job.outputs[0].segments[0].file).exists(),
        "segment kept"
    );

    // Space freed: Resume continues the same job from its finished segment.
    let resumed = Fake::new(Box::new(|job| (ok_lines(job), true, false)));
    let (events, emit) = recorder();
    let outcome = resume_job(&resumed, &job.dir, watch::channel(false).1, &emit).await;
    assert!(
        matches!(outcome, Outcome::Finished(_)),
        "{:?}",
        events.lock().unwrap()
    );
    let seen = resumed.jobs.lock().unwrap()[0].clone();
    assert_eq!(seen.outputs[0].segments[0].status, PartStatus::Done);
    assert!(matches!(
        events.lock().unwrap().get(1),
        Some(ExportEvent::Progress {
            frame: 900,
            segments_done: 1,
            ..
        })
    ));
    for f in &files {
        assert!(Path::new(&f.path).exists());
    }
    assert!(!job.dir.exists());
}

#[tokio::test]
async fn pause_stops_the_worker_and_keeps_the_job() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake::new(Box::new(|_| (vec![], false, true))));
    let (ids_tx, mut ids) = mpsc::unbounded_channel();
    let emit = move |e: ExportEvent| {
        if let ExportEvent::Started { job_id, .. } = e {
            let _ = ids_tx.send(job_id);
        }
    };
    let worker = fake.clone();
    let b = batch(dir.path()).0;
    let running =
        tokio::spawn(async move { run_batch(&*worker, b, watch::channel(false).1, &emit).await });
    let id = ids.recv().await.unwrap();
    // Started comes before the worker launches: wait until it runs.
    while fake.launches.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert!(klipolo_engine::export::runner::pause(&id));
    assert!(matches!(running.await.unwrap(), Outcome::Paused));
    let job = the_job(dir.path());
    assert_eq!(job.state, JobState::Paused);
    assert!(!klipolo_engine::export::runner::is_running(&id));
}

#[tokio::test]
async fn discard_while_a_pause_winds_down_deletes_the_job() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = Fake::new(Box::new(|_| (vec![], false, true)));
    fake.pause_stops = false;
    let fake = Arc::new(fake);
    let (ids_tx, mut ids) = mpsc::unbounded_channel();
    let emit = move |e: ExportEvent| {
        if let ExportEvent::Started { job_id, .. } = e {
            let _ = ids_tx.send(job_id);
        }
    };
    let worker = fake.clone();
    let b = batch(dir.path()).0;
    let running =
        tokio::spawn(async move { run_batch(&*worker, b, watch::channel(false).1, &emit).await });
    let id = ids.recv().await.unwrap();
    while fake.launches.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    use klipolo_engine::export::runner::{cancel, pause};
    assert!(pause(&id));
    tokio::task::yield_now().await;
    assert!(cancel(&id), "still running while it winds down");
    assert!(matches!(running.await.unwrap(), Outcome::Cancelled));
    assert!(list_jobs(&jobs_root(dir.path())).is_empty(), "deleted");
}

#[tokio::test]
async fn cancel_discards_the_job() {
    let dir = tempfile::tempdir().unwrap();
    let (b, files) = batch(dir.path());
    let fake = Fake::new(Box::new(|job| {
        finish_segment(job, 0, 0);
        (vec![], false, true)
    }));
    let (stop, cancel) = watch::channel(false);
    let stopper = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stop.send(true).unwrap();
        stop
    });
    let (outcome, events) = run(&fake, b, cancel).await;
    drop(stopper.await);
    assert!(matches!(outcome, Outcome::Cancelled));
    assert!(matches!(events.last(), Some(ExportEvent::Cancelled)));
    assert!(
        list_jobs(&jobs_root(dir.path())).is_empty(),
        "nothing left behind"
    );
    assert!(!Path::new(&files[0].path).exists());
}

#[test]
fn a_changed_project_offers_start_over() {
    let dir = tempfile::tempdir().unwrap();
    let job = ExportJob::create(&batch(dir.path()).0, 30).unwrap();
    let mut saved = project();
    assert!(job.is_current(&saved, &Default::default()));
    saved.name = "Renamed".into();
    assert!(
        job.is_current(&saved, &Default::default()),
        "a new name renders the same video"
    );
    saved.settings.formats.push(FormatKind::Square);
    assert!(
        job.is_current(&saved, &Default::default()),
        "so does another format in the project"
    );
    saved.timeline.main[1].speed = 2.0;
    assert!(!job.is_current(&saved, &Default::default()));
    assert!(
        job.info(false, !job.is_current(&saved, &Default::default()))
            .stale
    );
}

/// A job freezes the caption pages of exactly its files' formats (the project lists only landscape
/// here); a resumed job reads them back, new words make it stale (start over with them), and a job with captions on
/// never reads without its pages, so an export can't finish silently without captions.
#[test]
fn a_job_freezes_its_captions_and_never_runs_without_them() {
    use std::collections::HashMap;

    use klipolo_engine::export::job::CAPTIONS_FILE;
    use klipolo_model::speech::{Speech, Transcript, Word, WordKind};

    let dir = tempfile::tempdir().unwrap();
    let (mut b, _) = batch(dir.path());
    b.project.captions.enabled = true;
    let hi = Word {
        text: "Hi".into(),
        start_us: secs(1.0),
        end_us: secs(1.2),
        confidence: 0.9,
        kind: WordKind::Word,
    };
    let transcript = Transcript {
        key: "k".into(),
        language: "en".into(),
        model: "m".into(),
        covered_us: secs(60.0),
        complete: true,
        words: vec![hi],
    };
    let speech = Speech {
        transcripts: HashMap::from([("mic".to_owned(), Arc::new(transcript))]),
        ..Speech::default()
    };
    b.speech = Arc::new(speech.clone());
    let job = ExportJob::create(&b, 30).unwrap();
    assert_eq!(
        job.captions.pages.keys().copied().collect::<Vec<_>>(),
        [FormatKind::Landscape, FormatKind::Portrait]
    );
    let portrait = &job.captions.pages[&FormatKind::Portrait];
    assert_eq!(portrait.shown_text(&portrait.pages[0]), "Hi");
    assert_eq!(
        job.captions.style.as_str(),
        "klipolo.builtin/caption.clean",
        "a landscape-only project's style"
    );
    let opened = ExportJob::open(&job.dir).unwrap();
    assert_eq!(opened.captions, job.captions, "read back as frozen");
    assert!(job.is_current(&b.project, &speech));
    assert!(
        !job.is_current(&b.project, &Speech::default()),
        "other words: start over"
    );
    std::fs::remove_file(job.dir.join(CAPTIONS_FILE)).unwrap();
    assert!(ExportJob::open(&job.dir).is_err());
}

/// An export draws the pages frozen in its job, read back as they were when it is resumed, whatever the
/// speech is by then.
#[test]
fn export_draws_the_frozen_pages() {
    use klipolo_engine::plan::{Layer, PlanContext, frame_plan};

    let (p, sp) =
        klipolo_engine::test_support::captions_project("caption.bold", &[("frozen", 0.5, 0.9)]);
    let dir = tempfile::tempdir().unwrap();
    let job = ExportJob::create_for_test(dir.path(), &p, Arc::new(sp.clone()));
    let frozen = &job.captions;
    let now = klipolo_model::captions::captions(&p, &sp);
    assert_eq!(
        frozen.pages[&FormatKind::Landscape],
        now.pages[&FormatKind::Landscape]
    );
    // Reopened from disk (a resumed export): the same pages, and the worker's context draws them.
    let reopened = ExportJob::open(&job.dir).unwrap();
    assert_eq!(reopened.captions.pages, frozen.pages);
    let ctx = PlanContext::new(Arc::new(reopened.project.clone())).with_captions(reopened.captions);
    let shown = frame_plan(&ctx, secs(0.7), FormatKind::Landscape)
        .layers
        .into_iter()
        .find_map(|l| match l {
            Layer::Caption(c) => Some(c.shown_text()),
            _ => None,
        });
    assert_eq!(shown.as_deref(), Some("FROZEN"));
}

#[tokio::test]
async fn an_unreadable_job_says_so_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("0000-1111");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("job.json"), b"{ not json").unwrap();
    let fake = Fake::new(Box::new(|_| unreachable!("nothing to run")));
    let outcome = resume_job(&fake, &broken, watch::channel(false).1, &|_| {}).await;
    assert!(
        matches!(outcome, Outcome::Failed(Said::Text(ref m)) if m == klipolo_engine::export::jobs::UNREADABLE)
    );
}

/// A run renamed a joined file into place and crashed before recording it: the next run delivers that file instead
/// of rendering the format again (and writing "… 2.mp4").
#[tokio::test]
async fn a_file_moved_into_place_before_a_crash_is_delivered_not_redone() {
    use klipolo_engine::export::protocol::Lines;
    use klipolo_engine::export::shared::Control;
    use klipolo_engine::export::worker::{Ended, export};
    ffmpeg::init().unwrap();
    if !h264_encoder() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (b, files) = batch(dir.path());
    let mut job = ExportJob::create(&b, 30).unwrap();
    // Every output joined; the first one already moved to its final name (a real 4 s video: 120 frames).
    (job.fps, job.total_frames) = (30, 120);
    job.audio.status = PartStatus::Done;
    for (i, o) in job.outputs.iter_mut().enumerate() {
        o.joined = true;
        for s in o.segments.iter_mut() {
            s.status = PartStatus::Done;
        }
        if i == 0 {
            std::fs::copy(fixture("bars-30fps-0.5gop.mp4"), &files[0].path).unwrap();
            o.delivering = Some(files[0].path.clone().into());
        } else {
            o.delivered = Some(dir.path().join("elsewhere.mp4"));
        }
    }
    job.save().unwrap();
    let mut out = Vec::new();
    let ended = export(
        job.clone(),
        &mut Lines::new(&mut out),
        Arc::new(Control::default()),
    )
    .await;
    assert!(matches!(ended, Ok(Ended::Done)), "{ended:?}");
    let after = ExportJob::open(&job.dir).unwrap();
    assert_eq!(
        after.outputs[0].delivered.as_deref(),
        Some(Path::new(&files[0].path))
    );
    assert!(after.outputs[0].joined, "not rendered again");
}

// ---------------------------------------------------------------------------------------------------------------
// The real worker. Needs a GPU, VideoToolbox, `scripts/gen-engine-fixtures.sh --bench` and
// `cargo build -p klipolo-engine --example perf_harness` (that binary runs the worker for `--export-worker`).

/// False (the test is skipped) where this computer has no H.264 encoder: Windows and Linux until W and L.
fn h264_encoder() -> bool {
    let found = klipolo_engine::export::quality::probe_caps_for(&[]).h264;
    if !found {
        eprintln!("skipped: no H.264 encoder on this computer");
    }
    found
}

fn worker() -> klipolo_engine::export::runner::ProcessLauncher {
    let exe =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/examples/perf_harness");
    assert!(
        exe.exists(),
        "build it: cargo build -p klipolo-engine --example perf_harness"
    );
    klipolo_engine::export::runner::ProcessLauncher::export(exe)
}

/// 10 s of `peaks-10s-1080p30.mov`: synthetic pictures, quiet noise with 10 ms bursts that the loudness gain pushes
/// past the limiter's ceiling.
fn peaks_batch(dir: &Path) -> Batch {
    use klipolo_model::timeline::{Clip, ClipSource};
    let mut p = project();
    let a = p.assets.iter_mut().find(|a| a.id == "broll").unwrap();
    a.path = fixture("peaks-10s-1080p30.mov").display().to_string();
    a.duration_us = secs(10.0);
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "broll".into() },
        0,
        secs(10.0),
    )];
    let formats = [FormatKind::Landscape, FormatKind::Portrait];
    Batch {
        files: plan_files(&p, &formats, ExportQuality::High, dir),
        project: p,
        fps: 30,
        quality: ExportQuality::High,
        pcm_dir: None,
        speech: Arc::default(),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor: false,
    }
}

/// Waits until a job in `root` has at least two finished segments, one or more in every format (not all of them).
async fn first_segments(root: &Path) -> PathBuf {
    for _ in 0..2_400 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Some(Ok(job)) = list_jobs(root).into_iter().next() {
            let each = job
                .outputs
                .iter()
                .all(|o| o.segments.iter().any(|s| s.status == PartStatus::Done));
            if each && job.segments_done() < job.segments_total() {
                return job.dir;
            }
        }
    }
    panic!("no segment finished in two minutes");
}

/// Interleaved stereo samples of the decoded sound.
fn sound(path: &Path) -> Vec<f32> {
    let mut input = ffmpeg::format::input(path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Audio).unwrap();
    let index = stream.index();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .audio()
        .unwrap();
    let (mut frame, mut out) = (ffmpeg::frame::Audio::empty(), Vec::new());
    let mut take = |decoder: &mut ffmpeg::decoder::Audio, out: &mut Vec<f32>| {
        while decoder.receive_frame(&mut frame).is_ok() {
            let (l, r) = (frame.plane::<f32>(0), frame.plane::<f32>(1));
            out.extend(l.iter().zip(r).flat_map(|(a, b)| [*a, *b]));
        }
    };
    for (s, packet) in input.packets() {
        if s.index() == index {
            decoder.send_packet(&packet).unwrap();
            take(&mut decoder, &mut out);
        }
    }
    decoder.send_eof().unwrap();
    take(&mut decoder, &mut out);
    out
}

fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len().max(1) as f64)
        .sqrt()
}

/// Kill the worker mid-export (as an app crash would), resume, and compare with a single run.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU, VideoToolbox, the bench fixtures and the perf_harness example"]
async fn a_killed_export_resumes_and_matches_a_single_run() {
    ffmpeg::init().unwrap();
    if !h264_encoder() {
        return;
    }
    let launcher = worker();
    let segment_s = 2;
    let single_dir = tempfile::tempdir().unwrap();
    let batch = peaks_batch(single_dir.path());
    let no_cancel = watch::channel(false).1;
    let single = match run_batch_with(&launcher, batch, segment_s, no_cancel, &|_| {}).await {
        Outcome::Finished(paths) => paths,
        other => panic!("single run: {other:?}"),
    };

    let dir = tempfile::tempdir().unwrap();
    let root = jobs_root(dir.path());
    let batch = peaks_batch(dir.path());
    let no_cancel = watch::channel(false).1;
    let job_dir = tokio::select! {
        outcome = run_batch_with(&launcher, batch, segment_s, no_cancel, &|_| {}) => {
            panic!("finished before the kill: {outcome:?}")
        }
        // Dropping the run kills the worker (SIGKILL, as when the app dies).
        dir = first_segments(&root) => dir,
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let before = ExportJob::open(&job_dir).unwrap();
    let (done, total) = (before.segments_done(), before.segments_total());
    eprintln!("killed with {done} of {total} segments finished");
    assert!(done >= 2 && done < total);

    let (events, emit) = recorder();
    let resumed = match resume_job(&launcher, &job_dir, watch::channel(false).1, &emit).await {
        Outcome::Finished(paths) => paths,
        other => panic!("resume: {other:?} {:?}", events.lock().unwrap()),
    };
    // The resumed run started from the finished segments.
    let events = events.lock().unwrap().clone();
    assert!(
        matches!(events.get(1), Some(ExportEvent::Progress { frame, .. }) if *frame >= before.frames_done())
    );
    assert!(!job_dir.exists());

    let seams: Vec<usize> = (1..5).map(|k| k * 60).collect();
    for (a, b) in single.iter().zip(&resumed) {
        let (fa, da) = klipolo_engine::export::segment::probe(a).unwrap();
        let (fb, db) = klipolo_engine::export::segment::probe(b).unwrap();
        assert_eq!((fa, fb), (300, 300), "{}", b.display());
        assert!(
            (da - db).abs() < 1e-6 && (db - 10.0).abs() < 0.05,
            "{da} {db}"
        );
        // Keyframes at every seam of the resumed file.
        let mut input = ffmpeg::format::input(b).unwrap();
        let video = input.streams().best(ffmpeg::media::Type::Video).unwrap();
        let (vi, tb) = (video.index(), video.time_base());
        let to_frame = |pts: i64| {
            (pts as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator()) * 30.0).round()
                as usize
        };
        let keys: Vec<usize> = input
            .packets()
            .filter(|(s, p)| s.index() == vi && p.is_key())
            .map(|(_, p)| to_frame(p.pts().unwrap()))
            .collect();
        for s in &seams {
            assert!(keys.contains(s), "keyframe at {s}: {keys:?}");
        }
        // No seam artefacts: around each seam the resumed pictures match the single run's (both lossy encodes).
        let wanted: Vec<usize> = seams.iter().flat_map(|s| [s - 1, *s, s + 1]).collect();
        for (x, y) in luma(a, &wanted).iter().zip(luma(b, &wanted)) {
            let sum: u64 = x
                .iter()
                .zip(&y)
                .map(|(p, q)| u64::from(p.abs_diff(*q)))
                .sum();
            let mae = sum as f64 / x.len() as f64;
            assert!(mae < 2.0, "luma difference {mae:.2} at a seam");
        }
        // The sound is one track: equal to the single run's, and no click at the seams.
        let (sa, sb) = (sound(a), sound(b));
        assert_eq!(sa.len(), sb.len());
        let diff = sa
            .iter()
            .zip(&sb)
            .map(|(p, q)| (p - q).abs())
            .fold(0f32, f32::max);
        assert!(diff < 1e-4, "sound differs by {diff}");
        // RMS in 20 ms around each seam, against the single run (the fixture's bursts fall on whole seconds).
        for s in &seams {
            let at = s * 1600 * 2;
            let (ra, rb) = (rms(&sa[at - 960..at + 960]), rms(&sb[at - 960..at + 960]));
            assert!((ra - rb).abs() < 1e-3, "a click at frame {s}: {rb} vs {ra}");
        }
        // Finalize: index first, no edit list (both tracks at 0 is checked by the worker), true peak ≤ −1 dBTP.
        let file = std::fs::read(b).unwrap();
        let at = |tag: &[u8]| file.windows(4).position(|w| w == tag).unwrap();
        let (moov, mdat) = (at(b"moov"), at(b"mdat"));
        assert!(moov < mdat, "faststart");
        let index = &file[moov..mdat];
        assert!(
            !index.windows(4).any(|w| w == b"edts" || w == b"elst"),
            "no edit list"
        );
        let peak = klipolo_engine::export::sound::true_peak(b).unwrap();
        eprintln!("{}: true peak {peak:.2} dBTP", b.display());
        assert!(peak <= -1.0, "true peak {peak:.2} dBTP");
    }
}

/// A job running in compatibility mode is paused, then resumed as Resume does (auto): its software-encoded parts are
/// kept and it keeps finishing with the software encoder, rendering only the missing parts.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU, VideoToolbox and the bench fixtures"]
async fn a_compatibility_mode_job_resumes_without_redoing_its_parts() {
    use klipolo_engine::export::protocol::{Lines, WorkerLine, parse_line};
    use klipolo_engine::export::shared::Control;
    use klipolo_engine::export::worker::{Ended, export};
    ffmpeg::init().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut b = peaks_batch(dir.path());
    b.files.truncate(1);
    b.project.timeline.main[0].source_out_us = secs(4.0);
    let mut job = ExportJob::create(&b, 1).unwrap();
    job.mode = ExportMode::SoftwareSafe;
    job.save().unwrap();
    // Pause once two parts are finished.
    let control = Arc::new(Control::default());
    let (watched, pauser) = (job.dir.clone(), control.clone());
    let watch = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if ExportJob::open(&watched).is_ok_and(|j| j.segments_done() >= 2) {
                pauser.pause();
                return;
            }
        }
    });
    let first = export(job.clone(), &mut Lines::new(Vec::new()), control).await;
    watch.abort();
    assert!(matches!(first, Ok(Ended::Paused)), "{first:?}");
    let paused = ExportJob::open(&job.dir).unwrap();
    let done: Vec<_> = paused.outputs[0]
        .segments
        .iter()
        .filter(|s| s.status == PartStatus::Done)
        .map(|s| (s.index, s.checksum))
        .collect();
    assert!(done.len() >= 2 && done.len() < 4, "{done:?}");
    assert!(
        paused.outputs[0]
            .segments
            .iter()
            .all(|s| s.status != PartStatus::Done || s.software)
    );

    // Resume (auto: the runner sets Hardware): the software parts are kept, not redone.
    let mut resumed = paused.clone();
    resumed.mode = ExportMode::Hardware;
    resumed.save().unwrap();
    let mut out = Vec::new();
    let second = export(
        resumed,
        &mut Lines::new(&mut out),
        Arc::new(Control::default()),
    )
    .await;
    assert!(matches!(second, Ok(Ended::Done)), "{second:?}");
    let lines: Vec<WorkerLine> = String::from_utf8(out)
        .unwrap()
        .lines()
        .filter_map(parse_line)
        .collect();
    assert!(
        matches!(lines.first(), Some(WorkerLine::Started { frames_done, .. })
        if *frames_done == done.len() as u64 * 30)
    );
    let finished = ExportJob::open(&job.dir).unwrap();
    let o = &finished.outputs[0];
    assert!(o.joined);
    assert!(
        o.segments.iter().all(|s| s.software),
        "all finished in compatibility mode"
    );
    for (index, checksum) in done {
        assert_eq!(
            o.segments[index as usize].checksum, checksum,
            "part {index} kept as it was"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Caption files beside landscape exports: made when the job is, delivered beside the video, never
// overwriting, never half-written, never outside the exports folder.

/// "Story": a 60 s clip of voice file `v` with captions in `style` and a finished transcript of `words`.
fn story(
    style: &str,
    words: &[(&str, f64, f64)],
) -> (
    klipolo_model::project::Project,
    klipolo_model::speech::Speech,
) {
    let (mut p, sp) = klipolo_engine::test_support::captions_project(style, words);
    p.name = "Story".into();
    (p, sp)
}

#[test]
fn the_caption_file_is_delivered_beside_the_video_and_never_overwrites() {
    let (p, sp) = story("caption.bold", &[("hello", 0.5, 0.9)]);
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("Story (Landscape).srt"), "someone else's").unwrap();
    let job = ExportJob::create_for_test_with(
        out.path(),
        &p,
        Arc::new(sp),
        Some(klipolo_model::captions::sidecar::CaptionFile::Srt),
    );
    let delivered = klipolo_engine::test_support::finish_fake_export(job); // joins a fake encoded file and runs deliver::finish
    let srt = delivered
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "srt"))
        .expect("an .srt delivered");
    assert_ne!(
        srt.file_name().unwrap(),
        "Story (Landscape).srt",
        "the existing file is kept"
    );
    assert_eq!(srt.parent(), Some(out.path()), "beside the video");
    assert!(std::fs::read_to_string(srt).unwrap().contains("hello"));
    assert_eq!(
        std::fs::read_to_string(out.path().join("Story (Landscape).srt")).unwrap(),
        "someone else's"
    );
}

#[test]
fn a_caption_file_that_cant_be_saved_keeps_the_video_and_says_so() {
    use klipolo_engine::export::caption_file::frozen_name;
    use klipolo_model::captions::sidecar::CaptionFile;
    let (p, sp) = story("caption.clean", &[("hello", 0.5, 0.9)]);
    let out = tempfile::tempdir().unwrap();
    let job = ExportJob::create_for_test_with(out.path(), &p, Arc::new(sp), Some(CaptionFile::Vtt));
    assert_eq!(job.caption_file, Some(CaptionFile::Vtt));
    std::fs::remove_file(job.path(&frozen_name(CaptionFile::Vtt))).unwrap();
    let (delivered, said) = klipolo_engine::test_support::finish_fake_export_said(job);
    assert_eq!(delivered.len(), 1, "the video only: {delivered:?}");
    assert!(delivered[0].is_file());
    assert!(said.contains(&ExportEvent::CaptionFileFailed {
        index: 0,
        disk_full: false
    }));
    assert!(matches!(said.last(), Some(ExportEvent::Finished { .. })));
    let left: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(
        !left.iter().any(|n| n.to_string_lossy().ends_with(".vtt")),
        "no caption file, half or whole: {left:?}"
    );
}

#[test]
fn a_caption_file_moved_into_place_before_a_crash_is_delivered_once() {
    use klipolo_engine::export::caption_file::frozen_name;
    use klipolo_model::captions::sidecar::CaptionFile;
    let (p, sp) = story("caption.bold", &[("hello", 0.5, 0.9)]);
    let out = tempfile::tempdir().unwrap();
    let mut job =
        ExportJob::create_for_test_with(out.path(), &p, Arc::new(sp), Some(CaptionFile::Srt));
    // A run moved it, then crashed before recording the move.
    let target = out.path().join("Story (Landscape).srt");
    std::fs::copy(job.path(&frozen_name(CaptionFile::Srt)), &target).unwrap();
    job.outputs[0].caption_delivering = Some(target.clone());
    job.save().unwrap();
    let delivered = klipolo_engine::test_support::finish_fake_export(job);
    assert!(delivered.contains(&target));
    let srts = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "srt"))
        .count();
    assert_eq!(srts, 1, "not \"Story (Landscape) 2.srt\"");
}

#[test]
fn a_cancelled_export_leaves_no_caption_file() {
    use klipolo_model::captions::sidecar::CaptionFile;
    let (p, sp) = story("caption.bold", &[("hello", 0.5, 0.9)]);
    let out = tempfile::tempdir().unwrap();
    let job = ExportJob::create_for_test_with(out.path(), &p, Arc::new(sp), Some(CaptionFile::Srt));
    klipolo_engine::export::runner::discard_job(&job.dir).unwrap();
    let mut files = Vec::new();
    for dir in [out.path().to_path_buf(), jobs_root(out.path())] {
        files.extend(std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()));
    }
    assert!(
        !files
            .iter()
            .any(|f| f.extension().is_some_and(|x| x == "srt")),
        "{files:?}"
    );
}

#[test]
fn a_caption_file_is_never_written_outside_the_exports_folder() {
    use klipolo_model::captions::sidecar::CaptionFile;
    let (p, sp) = story("caption.bold", &[("hello", 0.5, 0.9)]);
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("Exports");
    std::fs::create_dir_all(&out).unwrap();
    let mut job = ExportJob::create_for_test_with(&out, &p, Arc::new(sp), Some(CaptionFile::Srt));
    // A damaged job whose video goes elsewhere: the caption file doesn't follow it.
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    job.outputs[0].final_path = elsewhere.join("Story (Landscape).mp4");
    let (_, said) = klipolo_engine::test_support::finish_fake_export_said(job);
    assert!(said.contains(&ExportEvent::CaptionFileFailed {
        index: 0,
        disk_full: false
    }));
    for dir in [&out, &elsewhere] {
        assert!(
            !std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "srt")),
            "{dir:?}"
        );
    }
}

#[test]
fn a_job_with_a_caption_file_is_changed_when_what_was_said_is() {
    // Captions off in the video, a caption file asked for: a transcript finished since makes the job's file stale.
    use klipolo_model::captions::sidecar::CaptionFile;
    let (mut p, sp) = story("caption.bold", &[("hello", 0.5, 0.9)]);
    p.captions.enabled = false;
    let out = tempfile::tempdir().unwrap();
    let job = ExportJob::create_for_test_with(
        out.path(),
        &p,
        Arc::new(sp.clone()),
        Some(CaptionFile::Srt),
    );
    assert!(job.is_current(&p, &sp));
    let (_, more) = story("caption.bold", &[("hello", 0.5, 0.9), ("again", 1.0, 1.4)]);
    assert!(!job.is_current(&p, &more));
    let plain = ExportJob::create_for_test(out.path(), &p, Arc::new(sp.clone()));
    assert!(
        plain.is_current(&p, &more),
        "no caption file: what was said changes nothing"
    );
}

/// A caption file asked for when none of the transcribed words are in the video is never
/// silent: the export says there were no captions to save, then finishes with the video.
#[test]
fn a_caption_file_with_no_words_to_save_says_so() {
    use klipolo_model::captions::sidecar::CaptionFile;
    // The transcript's only word is past the clip's end: nothing to caption.
    let (p, sp) = story("caption.clean", &[("later", 70.0, 70.4)]);
    let out = tempfile::tempdir().unwrap();
    let job = ExportJob::create_for_test_with(out.path(), &p, Arc::new(sp), Some(CaptionFile::Srt));
    assert_eq!(job.caption_file, None, "no file frozen");
    let (delivered, said) = klipolo_engine::test_support::finish_fake_export_said(job);
    assert_eq!(delivered.len(), 1, "the video only");
    assert!(said.contains(&ExportEvent::CaptionFileEmpty), "{said:?}");
    assert!(matches!(said.last(), Some(ExportEvent::Finished { .. })));
    // Nothing asked for: nothing said.
    let (p, sp) = story("caption.clean", &[("later", 70.0, 70.4)]);
    let job = ExportJob::create_for_test_with(out.path(), &p, Arc::new(sp), None);
    let (_, said) = klipolo_engine::test_support::finish_fake_export_said(job);
    assert!(!said.contains(&ExportEvent::CaptionFileEmpty));
}

/// A caption destination recorded in the job (`job.json`) is used only while it is in the
/// exports folder, as a new one is (`beside`): never outside it.
#[test]
fn a_recorded_caption_destination_outside_the_exports_folder_is_never_used() {
    use klipolo_model::captions::sidecar::CaptionFile;
    let (p, sp) = story("caption.bold", &[("hello", 0.5, 0.9)]);
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("Exports");
    let elsewhere = root.path().join("elsewhere");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut job = ExportJob::create_for_test_with(&out, &p, Arc::new(sp), Some(CaptionFile::Srt));
    job.outputs[0].caption_delivering = Some(elsewhere.join("Story (Landscape).srt"));
    job.save().unwrap();
    let delivered = klipolo_engine::test_support::finish_fake_export(job);
    let srt = delivered
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "srt"))
        .expect("an .srt delivered");
    assert_eq!(srt.parent(), Some(out.as_path()), "beside the video");
    assert!(
        std::fs::read_dir(&elsewhere).unwrap().next().is_none(),
        "nothing written outside the exports folder"
    );
}

/// Stop export takes its job out all at once, as Discard does (renamed in its own
/// folder, then deleted in the background), never leaving a half-deleted job folder that still lists. Here one of the
/// job's folders can't be emptied (read-only), and still its folder is gone the moment the Stop ends.
#[cfg(unix)]
#[tokio::test]
async fn a_stopped_export_goes_all_at_once_even_when_its_files_cant_all_be_deleted() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (b, _files) = batch(dir.path());
    let job_dir = Arc::new(Mutex::new(None::<PathBuf>));
    let seen = job_dir.clone();
    let fake = Fake::new(Box::new(move |job| {
        let locked = job.dir.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("part"), b"part").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        *seen.lock().unwrap() = Some(job.dir.clone());
        (vec![], false, true)
    }));
    let (stop, cancel) = watch::channel(false);
    let stopper = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stop.send(true).unwrap();
        stop
    });
    let (outcome, _) = run(&fake, b, cancel).await;
    drop(stopper.await);
    assert!(matches!(outcome, Outcome::Cancelled));
    let job_dir = job_dir.lock().unwrap().clone().expect("it ran");
    let root = jobs_root(dir.path());
    let left: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    // Let the test's folder go: the read-only folder, wherever it is now, is made writable.
    for name in &left {
        let locked = root.join(name).join("locked");
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    }
    assert!(
        !job_dir.exists(),
        "its folder is gone at once, never half deleted: {left:?}"
    );
    assert!(list_jobs(&root).is_empty(), "nothing listed");
}

/// Quit pausing a job whose run hasn't started yet (Start over's new job, written a few ms before its run starts)
/// pauses it the moment its run starts: it is kept and paused, its worker never launched.
#[tokio::test]
async fn a_job_paused_before_its_run_starts_pauses_as_it_starts() {
    use klipolo_engine::export::runner::{pause_when_it_runs, run_created};
    let dir = tempfile::tempdir().unwrap();
    let job = ExportJob::create(&batch(dir.path()).0, 30).unwrap();
    assert!(!pause_when_it_runs(&job.job_id), "not running yet");
    let fake = Fake::new(Box::new(|job| (ok_lines(job), true, false)));
    let (events, emit) = recorder();
    let outcome = run_created(&fake, job, watch::channel(false).1, &emit).await;
    assert!(
        matches!(outcome, Outcome::Paused),
        "paused as it starts: {:?}",
        events.lock().unwrap().last()
    );
    assert_eq!(
        fake.launches.load(Ordering::SeqCst),
        0,
        "its worker never launched"
    );
    assert_eq!(
        the_job(dir.path()).state,
        JobState::Paused,
        "kept: it resumes"
    );
}

/// A Stop whose job can't be set aside (its folder held open elsewhere, here the name it would take already in use)
/// keeps the job whole and says it paused with why, never stopped while it still lists.
#[tokio::test]
async fn a_stop_that_cant_set_its_job_aside_says_it_paused_and_keeps_it() {
    use klipolo_engine::export::jobs::discarded_name;
    let dir = tempfile::tempdir().unwrap();
    let (b, _files) = batch(dir.path());
    let fake = Fake::new(Box::new(|job| {
        let id = job.dir.file_name().unwrap().to_str().unwrap();
        let taken = job.dir.with_file_name(discarded_name(id));
        std::fs::create_dir_all(&taken).unwrap();
        std::fs::write(taken.join("x"), b"x").unwrap();
        (vec![], false, true)
    }));
    let (stop, cancel) = watch::channel(false);
    let stopper = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stop.send(true).unwrap();
        stop
    });
    let (outcome, events) = run(&fake, b, cancel).await;
    drop(stopper.await);
    assert!(matches!(outcome, Outcome::Paused), "{outcome:?}");
    assert!(
        matches!(
            events.last(),
            Some(ExportEvent::Paused {
                message: Some(_),
                ..
            })
        ),
        "paused, saying why: {:?}",
        events.last()
    );
    assert_eq!(the_job(dir.path()).state, JobState::Paused, "kept whole");
}

/// A pause queued for a run that never started (its start dropped the job, or the slot was released first) is let go
/// with its slot, so a later run of that job starts as asked.
#[tokio::test]
async fn a_pause_queued_for_a_run_that_never_started_is_let_go() {
    use klipolo_engine::export::runner::{forget_pause, pause_when_it_runs, run_created};
    let dir = tempfile::tempdir().unwrap();
    let job = ExportJob::create(&batch(dir.path()).0, 30).unwrap();
    assert!(!pause_when_it_runs(&job.job_id));
    forget_pause(&job.job_id);
    let fake = Fake::new(Box::new(|job| (ok_lines(job), true, false)));
    let (_, emit) = recorder();
    let outcome = run_created(&fake, job, watch::channel(false).1, &emit).await;
    assert!(matches!(outcome, Outcome::Finished(_)), "{outcome:?}");
}
