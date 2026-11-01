//! `MediaJobs` driving a stand-in worker (no processes, no GPU): what the scheduler does with what a worker says.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use klipolo_engine::api::{
    CodedError, MediaJobInfo, MediaJobList, MediaJobState, MediaJobsHold, PauseBy,
};
use klipolo_engine::export::shared::FrameRate;
use klipolo_engine::media::MediaKind;
use klipolo_engine::media::key::{CacheKind, source_id, stem};
use klipolo_engine::media::proxy::decide::ProxyPolicy;
use klipolo_engine::media::proxy::decide::ProxyReason;
use klipolo_engine::media::proxy::facts::{ProbeError, SourceFacts};
use klipolo_engine::media::proxy::job::{CopyRecord, ProxyJob, job_dir, proxy_path};
use klipolo_engine::media::proxy::protocol::ProxyLine;
use klipolo_engine::media::proxy::run::{FailCode, PauseReason};
use klipolo_engine::media::proxy::runner::Attempt;
use klipolo_engine::media::proxy::runner::Launcher;
use klipolo_engine::plan::{MediaCopy, MediaMap};
use klipolo_engine::platform::gpu::GpuClass;
use klipolo_engine::session::ProjectFeed;
use klipolo_model::project::{Asset, AssetKind, Project};
use tokio::sync::{Notify, mpsc, oneshot, watch};

use super::tidy::tidy_proxies;
use super::{Command, MediaJobs, NOT_IN_PROJECT, Preview, lock};

/// One job a stand-in worker ran: its key, the length it was planned for and the frames already in finished segments
/// when it started.
type Launch = (String, i64, u64);

/// A worker that copies at once, as the real one does: progress, then the copy's record and the copy in place, and
/// `Done`. A source named in `edit_first` is overwritten (longer) before its first copy is delivered, as if the person
/// re-exported it meanwhile. With `hold` set, the next job started stays at its progress until the test notifies it,
/// then says it paused (its job folder kept, so starting it again finishes it). The next `crashes` jobs finish one
/// more segment and then end with no word, as a worker killed with `kill -9` does; the next jobs say they failed with
/// the codes in `fails`, in turn (a failure the worker reports itself).
#[derive(Default)]
struct StandIn {
    edit_first: Mutex<HashSet<String>>,
    launched: Mutex<Vec<Launch>>,
    hold: Mutex<Option<Arc<Notify>>>,
    crashes: Mutex<u32>,
    fails: Mutex<Vec<FailCode>>,
}

/// Takes one from `count` if it has any.
fn take_one(count: &Mutex<u32>) -> bool {
    let mut n = count.lock().unwrap();
    let some = *n > 0;
    *n = n.saturating_sub(1);
    some
}

impl Launcher for StandIn {
    fn launch(&self, manifest: &Path) -> Result<Attempt, String> {
        let mut job = ProxyJob::open(manifest.parent().unwrap())?;
        self.launched
            .lock()
            .unwrap()
            .push((job.stem.clone(), job.duration_us, job.frames_done()));
        let edit = self.edit_first.lock().unwrap().remove(&job.source);
        let hold = self.hold.lock().unwrap().take();
        let crash = take_one(&self.crashes);
        let fail = {
            let mut fails = self.fails.lock().unwrap();
            (!fails.is_empty()).then(|| fails.remove(0))
        };
        let (tx, lines) = mpsc::unbounded_channel();
        let (exited, exit) = oneshot::channel();
        tokio::spawn(async move {
            let line = |l: ProxyLine| serde_json::to_string(&l).unwrap();
            let total = job.total_frames;
            let _ = tx.send(line(ProxyLine::Progress {
                done: job.frames_done(),
                total,
            }));
            if crash {
                // One more segment is finished and in the manifest, then the process is gone without a word.
                if let Some(next) = job.next_pending() {
                    job.segment_done(next, (1, 0), None);
                    job.save().unwrap();
                }
                drop(tx);
                let _ = exited.send(false);
                return;
            }
            if let Some(code) = fail {
                let _ = tx.send(line(ProxyLine::Failed {
                    message: format!("{code:?}: as the worker says it"),
                    missing: code == FailCode::Missing,
                    code,
                }));
                drop(tx);
                let _ = exited.send(false);
                return;
            }
            if let Some(hold) = hold {
                hold.notified().await;
                let _ = tx.send(line(ProxyLine::Paused {
                    reason: PauseReason::User,
                }));
                drop(tx);
                let _ = exited.send(true);
                return;
            }
            if edit {
                overwrite(Path::new(&job.source), b"source, re-exported longer");
            }
            CopyRecord::of(&job).write(&job.out).unwrap();
            std::fs::write(&job.out, b"a stand-in copy").unwrap();
            std::fs::remove_dir_all(&job.dir).unwrap();
            let _ = tx.send(line(ProxyLine::Done {
                path: job.out.clone(),
            }));
            drop(tx);
            let _ = exited.send(true);
        });
        Ok(Attempt {
            lines,
            exit,
            kill: Box::new(|| {}),
            pause: Box::new(|| {}),
        })
    }
}

/// Writes `bytes` over `path` in place (its size changes).
fn overwrite(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

#[derive(Default)]
struct FakePreview {
    maps: Mutex<Vec<Arc<MediaMap>>>,
}

impl Preview for FakePreview {
    fn set_media(&self, map: Arc<MediaMap>) {
        self.maps.lock().unwrap().push(map);
    }

    fn media_event(&self, _: String, _: MediaKind) {}

    fn gpu_class(&self) -> GpuClass {
        GpuClass::Integrated
    }
}

impl FakePreview {
    fn last(&self) -> Arc<MediaMap> {
        self.maps
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }
}

/// 4K at 29.97, so Intel needs a copy (`TooLarge`); a second of video per byte of the stand-in file, so an overwrite
/// changes the facts (its length) as a re-export would.
fn facts(path: &Path) -> Result<SourceFacts, ProbeError> {
    let len = std::fs::metadata(path)
        .map_err(|e| ProbeError::Unreadable(e.to_string()))?
        .len();
    Ok(SourceFacts {
        width: 3840,
        height: 2160,
        rate: FrameRate {
            num: 30_000,
            den: 1001,
        },
        codec: "h264".into(),
        ten_bit: false,
        ten_bit_hw: false,
        hdr: false,
        max_key_interval_us: 500_000,
        variable_rate: false,
        hw_decodes: true,
        duration_us: len as i64 * 1_000_000,
        start_us: 0,
        id: path.to_str().and_then(source_id),
    })
}

fn video(path: &Path) -> Asset {
    Asset {
        id: path.file_name().unwrap().to_string_lossy().into(),
        kind: AssetKind::Video,
        path: path.to_string_lossy().into(),
        name: "clip".into(),
        duration_us: 20_000_000,
        width: 3840,
        height: 2160,
        fps: 30_000.0 / 1001.0,
        rotation: None,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
    }
}

fn feed(sources: &[&Path]) -> Option<ProjectFeed> {
    let mut project = Project::new("Copies");
    project.assets = sources.iter().map(|p| video(p)).collect();
    Some(ProjectFeed {
        project_id: project.id.clone(),
        revision: 0,
        opened: 1,
        project: Arc::new(project),
        speech: Arc::default(),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    })
}

async fn eventually(what: &str, jobs: &MediaJobs, mut ok: impl FnMut() -> bool) {
    for _ in 0..500 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never happened: {what}; the list: {:?}", jobs.list().jobs);
}

fn state_of(list: &[MediaJobInfo], path: &Path) -> Option<MediaJobState> {
    let path = path.to_string_lossy();
    list.iter().find(|j| j.path == path).map(|j| j.state)
}

/// The copy `path` would have now (its key from the file as it is).
fn copy_of(root: &Path, path: &Path) -> PathBuf {
    let key = stem(
        &path.to_string_lossy(),
        &CacheKind::Proxy { long_side: 1280 },
    )
    .unwrap();
    proxy_path(root, &key)
}

/// A scheduler on a stand-in worker, listing `sources`, not yet attached.
fn scheduler(
    root: &Path,
    worker: &Arc<StandIn>,
    heard: &Arc<Mutex<Vec<MediaJobList>>>,
) -> MediaJobs {
    let told = heard.clone();
    let jobs = MediaJobs::with(
        root.to_path_buf(),
        worker.clone(),
        Arc::new(move |list| told.lock().unwrap().push(list)),
        Arc::new(facts),
        |class| ProxyPolicy::with_encoder(class, true),
    );
    jobs.start();
    jobs
}

/// A scheduler on a stand-in worker that probes with `probe` (the lists it tells aren't kept), not yet attached.
fn scheduler_probing(
    root: &Path,
    worker: &Arc<StandIn>,
    probe: impl Fn(&Path) -> Result<SourceFacts, ProbeError> + Send + Sync + 'static,
) -> MediaJobs {
    let jobs = MediaJobs::with(
        root.to_path_buf(),
        worker.clone(),
        Arc::new(|_| {}),
        Arc::new(probe),
        |class| ProxyPolicy::with_encoder(class, true),
    );
    jobs.start();
    jobs
}

/// A copy of `source` as it is, made earlier (its record and the copy in place): the copy.
fn finished_copy(root: &Path, source: &Path) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    let planned = ProxyJob::create(
        root,
        &source.to_string_lossy(),
        &facts(source).unwrap(),
        &ProxyPolicy::with_encoder(GpuClass::Integrated, true),
    )
    .unwrap();
    std::fs::remove_dir_all(&planned.dir).unwrap();
    let copy = copy_of(root, source);
    CopyRecord::of(&planned).write(&copy).unwrap();
    std::fs::write(&copy, b"a copy made earlier").unwrap();
    copy
}

/// - Listing writes nothing (a job folder is made only when its copy starts, which waits for the engine's GPU class).
/// - A source overwritten after it was listed is read again as its job starts, so its copy is planned from the file as
///   it is.
/// - A source overwritten while it is copied: the delivered copy is never handed over and is deleted, and the file is
///   copied again from facts read again.
/// - The copy is at the source's exact rate.
/// - Stop is refused for a file the open project doesn't have.
#[test]
fn copies_are_planned_from_the_source_as_it_is_and_only_a_current_copy_reaches_the_preview() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"));
        overwrite(&a, b"source a");
        overwrite(&b, b"source b");
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        worker
            .edit_first
            .lock()
            .unwrap()
            .insert(b.to_string_lossy().into());
        let heard: Arc<Mutex<Vec<MediaJobList>>> = Arc::default();
        let jobs = scheduler(&root, &worker, &heard);
        let (_publish, rx) = watch::channel(feed(&[&a, &b]));
        jobs.watch(rx);
        eventually("both files are listed", &jobs, || {
            jobs.list().jobs.len() == 2
        })
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Each says why it gets a copy (4K on Intel), and has no reason code while it waits.
        assert!(
            jobs.list()
                .jobs
                .iter()
                .all(|j| j.state == MediaJobState::Waiting
                    && j.why == Some(ProxyReason::TooLarge)
                    && j.reason.is_none()),
            "{:?}",
            jobs.list().jobs
        );
        assert!(
            !root.exists() || std::fs::read_dir(&root).unwrap().next().is_none(),
            "listing made no job folder"
        );
        // a is re-exported (longer) after it was listed and before its copy starts.
        overwrite(&a, b"source a, re-exported");

        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        eventually("both copies are done", &jobs, || {
            let list = jobs.list().jobs;
            [&a, &b]
                .iter()
                .all(|p| state_of(&list, p) == Some(MediaJobState::Done))
        })
        .await;
        let launched = worker.launched.lock().unwrap().clone();
        assert_eq!(launched.len(), 3, "{launched:?}");
        let secs = |p: &Path| std::fs::metadata(p).unwrap().len() as i64 * 1_000_000;
        assert_eq!(
            launched[0].1,
            secs(&a),
            "a's job was planned from a as it is now"
        );
        // b's first copy was of the version before the edit: never handed over, deleted, and b copied again from b as
        // it is now.
        assert_eq!(launched[2].1, secs(&b));
        let stale = proxy_path(&root, &launched[1].0);
        assert!(
            !stale.exists(),
            "the stale copy is the cache's own: deleted"
        );
        let maps = preview.maps.lock().unwrap().clone();
        assert!(
            maps.iter()
                .flat_map(|m| m.values())
                .all(|c| *c.path != *stale.to_string_lossy()),
            "the stale copy never reached the preview"
        );
        let copy = |p: &Path| {
            MediaCopy::new(
                copy_of(&root, p).to_string_lossy().as_ref(),
                30_000.0 / 1001.0,
            )
        };
        let last = preview.last();
        assert_eq!(last.get(&*a.to_string_lossy()), Some(&copy(&a)));
        assert_eq!(last.get(&*b.to_string_lossy()), Some(&copy(&b)));
        assert!(
            heard
                .lock()
                .unwrap()
                .iter()
                .any(|l| state_of(&l.jobs, &a) == Some(MediaJobState::Working))
        );

        assert_eq!(
            jobs.command(Command::Stop("/elsewhere/other.mp4".into()))
                .await,
            Err(CodedError::new(NOT_IN_PROJECT))
        );
    });
}

/// A source overwritten while its project is open stops showing its old copy as soon as Klipolo is in front
/// again (`recheck`), the old copy is deleted and a new one made; and a copy found at listing whose record doesn't
/// match its source's facts (made from another version) is never mapped and is deleted.
#[test]
fn an_overwritten_source_drops_its_copy_and_a_mismatched_copy_is_never_mapped() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"));
        overwrite(&a, b"source a");
        overwrite(&b, b"source b");
        let root = dir.path().join("media/proxies");
        std::fs::create_dir_all(&root).unwrap();
        // A copy of b as b is now, but recorded from other facts (a longer version's): stale.
        let b_copy = copy_of(&root, &b);
        let mut job_facts = facts(&b).unwrap();
        job_facts.duration_us *= 2;
        let planned = ProxyJob::create(
            &root,
            &b.to_string_lossy(),
            &job_facts,
            &ProxyPolicy::with_encoder(GpuClass::Integrated, true),
        )
        .unwrap();
        std::fs::remove_dir_all(&planned.dir).unwrap();
        CopyRecord::of(&planned).write(&b_copy).unwrap();
        std::fs::write(&b_copy, b"a copy of another version").unwrap();

        let worker = Arc::new(StandIn::default());
        let heard = Arc::default();
        let jobs = scheduler(&root, &worker, &heard);
        let (_publish, rx) = watch::channel(feed(&[&a, &b]));
        jobs.watch(rx);
        eventually("both files are listed", &jobs, || {
            jobs.list().jobs.len() == 2
        })
        .await;
        assert_eq!(
            state_of(&jobs.list().jobs, &b),
            Some(MediaJobState::Waiting)
        );
        assert!(
            !b_copy.exists(),
            "a copy of another version is deleted at listing"
        );
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        eventually("both copies are done", &jobs, || {
            jobs.list()
                .jobs
                .iter()
                .all(|j| j.state == MediaJobState::Done)
        })
        .await;
        assert!(
            preview.maps.lock().unwrap().iter().all(|m| m.len() <= 2),
            "only copies made here"
        );

        // a is re-exported while the project is open; Klipolo comes back to the front.
        let old = copy_of(&root, &a);
        assert_eq!(
            preview
                .last()
                .get(&*a.to_string_lossy())
                .map(|c| c.path.to_string()),
            Some(old.to_string_lossy().into_owned())
        );
        overwrite(&a, b"source a, re-exported");
        jobs.recheck();
        eventually("a's new copy is in use", &jobs, || {
            preview
                .last()
                .get(&*a.to_string_lossy())
                .is_some_and(|c| *c.path == *copy_of(&root, &a).to_string_lossy())
        })
        .await;
        assert!(!old.exists(), "the old copy is the cache's own: deleted");
        assert!(
            preview
                .maps
                .lock()
                .unwrap()
                .iter()
                .any(|m| !m.contains_key(&*a.to_string_lossy())),
            "the old copy left the preview before the new one came"
        );
    });
}

/// A source that can't be looked at (its drive ejected, its share gone) only takes its copy out of the
/// preview; the copy is kept. Back unchanged, it is mapped again without being made again; back as another version, the
/// old copy is deleted and a new one made.
#[test]
fn a_source_away_keeps_its_copy_and_gets_it_back_when_it_returns_unchanged() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"));
        overwrite(&a, b"source a");
        overwrite(&b, b"source b");
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        let heard = Arc::default();
        let jobs = scheduler(&root, &worker, &heard);
        let (_publish, rx) = watch::channel(feed(&[&a, &b]));
        jobs.watch(rx);
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        eventually("both copies are done", &jobs, || {
            jobs.list().jobs.len() == 2
                && jobs
                    .list()
                    .jobs
                    .iter()
                    .all(|j| j.state == MediaJobState::Done)
        })
        .await;
        let (copy_a, copy_b) = (copy_of(&root, &a), copy_of(&root, &b));
        let mapped = |p: &Path| {
            preview
                .last()
                .get(&*p.to_string_lossy())
                .map(|c| c.path.to_string())
        };
        assert!(mapped(&a).is_some() && mapped(&b).is_some());

        // Both drives go; Klipolo comes back to the front.
        let (a_away, b_away) = (dir.path().join("a.away"), dir.path().join("b.away"));
        std::fs::rename(&a, &a_away).unwrap();
        std::fs::rename(&b, &b_away).unwrap();
        jobs.recheck();
        eventually("both copies leave the preview", &jobs, || {
            mapped(&a).is_none() && mapped(&b).is_none()
        })
        .await;
        assert!(copy_a.exists() && copy_b.exists(), "kept, not deleted");

        // a comes back as it was; b comes back as another version.
        std::fs::rename(&a_away, &a).unwrap();
        overwrite(&b, b"source b, a new version");
        jobs.recheck();
        let new_b = copy_of(&root, &b);
        eventually("a's copy is back and b has a new one", &jobs, || {
            mapped(&a) == Some(copy_a.to_string_lossy().into_owned())
                && mapped(&b) == Some(new_b.to_string_lossy().into_owned())
        })
        .await;
        assert_eq!(
            worker.launched.lock().unwrap().len(),
            3,
            "only b was made again"
        );
        assert!(copy_a.exists());
        assert!(!copy_b.exists(), "b's old copy is the cache's own: deleted");
    });
}

/// With automatic copies off, listing the project's files adds no row for a file to copy (nothing would make it),
/// and probes none of them; a finished copy is still found and used. Turned on, the files are listed again.
#[test]
fn copies_off_list_no_file_to_copy_and_finished_copies_stay_in_use() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            dir.path().join("a.mp4"),
            dir.path().join("b.mp4"),
            dir.path().join("c.mp4"),
        );
        for (p, bytes) in [(&a, b"source a"), (&b, b"source b"), (&c, b"source c")] {
            overwrite(p, bytes);
        }
        let root = dir.path().join("media/proxies");
        // c's copy was made earlier, of c as it is.
        finished_copy(&root, &c);

        let probed: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
        let seen = probed.clone();
        let heard: Arc<Mutex<Vec<MediaJobList>>> = Arc::default();
        let told = heard.clone();
        let worker = Arc::new(StandIn::default());
        let jobs = MediaJobs::with(
            root.clone(),
            worker.clone(),
            Arc::new(move |list| told.lock().unwrap().push(list)),
            Arc::new(move |path: &Path| {
                seen.lock().unwrap().push(path.to_path_buf());
                facts(path)
            }),
            |class| ProxyPolicy::with_encoder(class, true),
        );
        jobs.start();
        assert_eq!(jobs.command(Command::SetAuto(false)).await, Ok(()));
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        let (_publish, rx) = watch::channel(feed(&[&a, &b, &c]));
        jobs.watch(rx);
        // a, b and c are decided in that order: once c's copy is in use, a and b have been.
        eventually("c's finished copy is in use", &jobs, || {
            preview.last().contains_key(&*c.to_string_lossy())
        })
        .await;
        let list = jobs.list();
        assert_eq!(list.held, [MediaJobsHold::AutoOff]);
        assert_eq!(
            list.jobs
                .iter()
                .map(|j| j.name.as_str())
                .collect::<Vec<_>>(),
            ["c.mp4"],
            "no row for a file to copy"
        );
        assert_eq!(list.jobs[0].state, MediaJobState::Done);
        assert!(
            heard
                .lock()
                .unwrap()
                .iter()
                .all(|l| l.jobs.iter().all(|j| j.state == MediaJobState::Done)),
            "no Waiting row was ever told"
        );
        assert_eq!(
            probed.lock().unwrap().as_slice(),
            std::slice::from_ref(&c),
            "only the file with a finished copy is probed"
        );
        assert!(worker.launched.lock().unwrap().is_empty());

        // On again: a and b are listed and copied.
        assert_eq!(jobs.command(Command::SetAuto(true)).await, Ok(()));
        eventually("a and b are copied", &jobs, || {
            let list = jobs.list();
            list.jobs.len() == 3 && list.jobs.iter().all(|j| j.state == MediaJobState::Done)
        })
        .await;
        assert_eq!(jobs.list().held, []);
        assert_eq!(
            worker.launched.lock().unwrap().len(),
            2,
            "c's copy is reused"
        );
    });
}

/// Who paused copies is in the list, so a page loaded again (`media_jobs_list`) knows it, and the event says it too.
/// The person's resume never lifts the action's pause, and the action's never lifts the person's.
#[test]
fn who_paused_copies_survives_a_list_reload() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        overwrite(&a, b"source a");
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        let heard: Arc<Mutex<Vec<MediaJobList>>> = Arc::default();
        let jobs = scheduler(&root, &worker, &heard);
        let (_publish, rx) = watch::channel(feed(&[&a]));
        jobs.watch(rx);
        eventually("a is listed", &jobs, || jobs.list().jobs.len() == 1).await;

        let held = |jobs: &MediaJobs| jobs.list().held;
        let told = |heard: &Arc<Mutex<Vec<MediaJobList>>>| {
            heard.lock().unwrap().last().map(|l| l.held.clone())
        };
        jobs.command(Command::Pause(PauseBy::Person)).await.unwrap();
        jobs.command(Command::Pause(PauseBy::Action)).await.unwrap();
        let both = vec![MediaJobsHold::Person, MediaJobsHold::Action];
        assert_eq!(held(&jobs), both);
        eventually("the event says both", &jobs, || {
            told(&heard) == Some(both.clone())
        })
        .await;
        // What the page reads: the holds by name.
        assert_eq!(
            serde_json::to_value(jobs.list()).unwrap()["held"],
            serde_json::json!(["person", "action"])
        );

        jobs.command(Command::Resume(PauseBy::Person))
            .await
            .unwrap();
        assert_eq!(
            held(&jobs),
            [MediaJobsHold::Action],
            "the action's pause stays"
        );
        jobs.command(Command::Pause(PauseBy::Person)).await.unwrap();
        jobs.command(Command::Resume(PauseBy::Action))
            .await
            .unwrap();
        assert_eq!(
            held(&jobs),
            [MediaJobsHold::Person],
            "the person's pause stays"
        );
        eventually("the event says the person's", &jobs, || {
            told(&heard) == Some(vec![MediaJobsHold::Person])
        })
        .await;
    });
}

/// Turning automatic copies off stops a listing already running: the file it was probing is the last it probes,
/// what it decided is dropped, and a finished copy stays listed and in use.
#[test]
fn turning_copies_off_stops_a_listing_already_running() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (x, g, y) = (
            dir.path().join("x.mp4"),
            dir.path().join("g.mp4"),
            dir.path().join("y.mp4"),
        );
        for (p, bytes) in [(&x, b"source x"), (&g, b"source g"), (&y, b"source y")] {
            overwrite(p, bytes);
        }
        let root = dir.path().join("media/proxies");
        finished_copy(&root, &x);
        // g's probe takes until the test lets it go (a slow file on a slow drive).
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let probed: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
        let (seen, slow) = (probed.clone(), g.clone());
        let worker = Arc::new(StandIn::default());
        let jobs = scheduler_probing(&root, &worker, move |path: &Path| {
            seen.lock().unwrap().push(path.to_path_buf());
            if path == slow {
                let _ = gate.lock().unwrap().recv();
            }
            facts(path)
        });
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        let (_publish, rx) = watch::channel(feed(&[&x, &g, &y]));
        jobs.watch(rx);
        eventually("x's copy is in use and g is being probed", &jobs, || {
            preview.last().contains_key(&*x.to_string_lossy())
                && probed.lock().unwrap().contains(&g)
        })
        .await;

        assert_eq!(jobs.command(Command::SetAuto(false)).await, Ok(()));
        release.send(()).unwrap();
        // Long enough for the listing to have gone on to y, had it not stopped.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            probed.lock().unwrap().as_slice(),
            [x.clone(), g.clone()],
            "y is never probed"
        );
        let list = jobs.list();
        assert_eq!(list.held, [MediaJobsHold::AutoOff]);
        assert_eq!(
            list.jobs
                .iter()
                .map(|j| (j.name.as_str(), j.state))
                .collect::<Vec<_>>(),
            [("x.mp4", MediaJobState::Done)]
        );
        assert!(preview.last().contains_key(&*x.to_string_lossy()));
        assert!(worker.launched.lock().unwrap().is_empty());
    });
}

/// Copies turned off and on again before the running copy has paused: it stays listed and is made on from where it
/// stopped (it used to leave the list once it paused, uncopied until the project's files changed).
#[test]
fn a_copy_turned_off_and_on_before_it_paused_is_made_on() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"));
        overwrite(&a, b"source a");
        overwrite(&b, b"source b");
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        let paused = Arc::new(Notify::new());
        *worker.hold.lock().unwrap() = Some(paused.clone());
        let heard = Arc::default();
        let jobs = scheduler(&root, &worker, &heard);
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        let (_publish, rx) = watch::channel(feed(&[&a, &b]));
        jobs.watch(rx);
        eventually("a's copy is being made", &jobs, || {
            state_of(&jobs.list().jobs, &a) == Some(MediaJobState::Working)
        })
        .await;

        assert_eq!(jobs.command(Command::SetAuto(false)).await, Ok(()));
        assert_eq!(
            state_of(&jobs.list().jobs, &b),
            None,
            "b, waiting, left the list"
        );
        assert_eq!(jobs.command(Command::SetAuto(true)).await, Ok(()));
        // b is listed again after a (one listing, in order): what it decided for a was heard while a was running.
        eventually("b is listed again", &jobs, || {
            state_of(&jobs.list().jobs, &b).is_some()
        })
        .await;
        // Only now does the worker say it paused.
        paused.notify_one();
        eventually("both copies are made", &jobs, || {
            let map = preview.last();
            [&a, &b]
                .iter()
                .all(|p| map.contains_key(&*p.to_string_lossy()))
        })
        .await;
        let launched = worker.launched.lock().unwrap().clone();
        assert_eq!(launched.len(), 3, "{launched:?}");
        assert_eq!(launched[0].0, launched[1].0, "a was made on from its job");
    });
}

/// A finished copy whose source was overwritten with something that isn't video (Klipolo in front, so no recheck)
/// leaves the preview at the next listing (here the project opened again), and its row says why.
#[test]
fn a_finished_copy_whose_source_turns_unreadable_leaves_the_preview() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        overwrite(&a, b"source a");
        let root = dir.path().join("media/proxies");
        let broken: Arc<Mutex<bool>> = Arc::default();
        let is_broken = broken.clone();
        let worker = Arc::new(StandIn::default());
        let jobs = scheduler_probing(&root, &worker, move |path: &Path| {
            if *is_broken.lock().unwrap() {
                return Err(ProbeError::Unreadable("not a video".into()));
            }
            facts(path)
        });
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        let (publish, rx) = watch::channel(feed(&[&a]));
        jobs.watch(rx);
        let mapped = || preview.last().contains_key(&*a.to_string_lossy());
        eventually("a's copy is in use", &jobs, mapped).await;

        overwrite(&a, b"not a video any more");
        *broken.lock().unwrap() = true;
        let mut again = feed(&[&a]).unwrap();
        again.opened = 2;
        publish.send(Some(again)).unwrap();
        eventually("a's copy leaves the preview", &jobs, || !mapped()).await;
        let list = jobs.list();
        assert_eq!(list.jobs[0].state, MediaJobState::Failed);
        assert_eq!(
            list.jobs[0].reason.as_ref().map(|r| r.code.as_str()),
            Some("copies.failed.unreadable")
        );
    });
}

/// The reason code a file's row says now.
fn reason_of(jobs: &MediaJobs, path: &Path) -> Option<String> {
    let path = path.to_string_lossy();
    jobs.list()
        .jobs
        .into_iter()
        .find(|j| j.path == path)
        .and_then(|j| j.reason.map(|r| r.code))
}

/// A worker killed mid-copy (no word of its own) is started again by itself, resuming from its
/// finished segments, twice; the row says the copy maker stopped and is trying again, never that the file is at fault.
/// A third stop fails the copy ("keeps stopping") and nothing restarts it until Try again, which makes it on from its
/// segments.
#[test]
fn a_killed_worker_restarts_from_its_segments_twice_then_waits_for_try_again() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        // 64 s of video for the stand-in: 7 segments of 10 s.
        overwrite(&a, &[b'a'; 64]);
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        *worker.crashes.lock().unwrap() = 3;
        let heard: Arc<Mutex<Vec<MediaJobList>>> = Arc::default();
        let jobs = scheduler(&root, &worker, &heard);
        lock(&jobs.inner.state).restart_waits =
            [Duration::from_millis(150), Duration::from_millis(300)];
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        let (_publish, rx) = watch::channel(feed(&[&a]));
        jobs.watch(rx);
        eventually("the third stop fails the copy", &jobs, || {
            reason_of(&jobs, &a).as_deref() == Some("copies.failed.workerKeepsStopping")
        })
        .await;
        let launched = worker.launched.lock().unwrap().clone();
        assert_eq!(launched.len(), 3, "started twice again: {launched:?}");
        assert!(
            launched.iter().all(|l| l.0 == launched[0].0),
            "the same job"
        );
        let resumed_from: Vec<u64> = launched.iter().map(|l| l.2).collect();
        assert!(
            resumed_from[0] == 0 && resumed_from[1] > 0 && resumed_from[2] > resumed_from[1],
            "each restart resumes from the segments finished before it: {resumed_from:?}"
        );
        assert_eq!(lock(&jobs.inner.state).jobs[0].restarts, 2);
        let said: Vec<Option<String>> = heard
            .lock()
            .unwrap()
            .iter()
            .filter_map(|l| {
                l.jobs
                    .first()
                    .map(|j| j.reason.as_ref().map(|r| r.code.clone()))
            })
            .collect();
        assert!(
            said.iter()
                .any(|c| c.as_deref() == Some("copies.paused.workerStopped")),
            "{said:?}"
        );
        assert!(
            !said
                .iter()
                .any(|c| c.as_deref() == Some("copies.failed.other")),
            "never worded as the file's fault: {said:?}"
        );
        let failed = jobs.list().jobs[0].reason.clone().unwrap();
        assert!(failed.params.is_empty(), "no file name: {failed:?}");
        // Nothing restarts it by itself now.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(worker.launched.lock().unwrap().len(), 3);

        // Try again: made on from its segments.
        jobs.command(Command::Retry(a.to_string_lossy().into()))
            .await
            .unwrap();
        eventually("the copy is made", &jobs, || {
            state_of(&jobs.list().jobs, &a) == Some(MediaJobState::Done)
        })
        .await;
        let launched = worker.launched.lock().unwrap().clone();
        assert_eq!(launched.len(), 4);
        assert!(launched[3].2 > launched[2].2, "{launched:?}");
        assert!(preview.last().contains_key(&*a.to_string_lossy()));
    });
}

/// A failure the worker reports itself (here a source it can't decode) is not restarted: it waits for
/// Try again, as before.
#[test]
fn a_failure_the_worker_reports_is_not_restarted() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        overwrite(&a, b"source a");
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        *worker.fails.lock().unwrap() = vec![FailCode::Other];
        let heard = Arc::default();
        let jobs = scheduler(&root, &worker, &heard);
        lock(&jobs.inner.state).restart_waits =
            [Duration::from_millis(50), Duration::from_millis(50)];
        jobs.attach_preview(Arc::new(FakePreview::default()), None);
        let (_publish, rx) = watch::channel(feed(&[&a]));
        jobs.watch(rx);
        eventually("the copy fails", &jobs, || {
            reason_of(&jobs, &a).as_deref() == Some("copies.failed.other")
        })
        .await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(worker.launched.lock().unwrap().len(), 1, "not restarted");
        assert_eq!(reason_of(&jobs, &a).as_deref(), Some("copies.failed.other"));
    });
}

/// A source that went during its copy ("a.mp4 is missing") and is put back is picked up within
/// `ABSENT_EVERY` by itself, with no edit and no return to the front, and its copy goes on from its job.
#[test]
fn a_source_put_back_is_picked_up_without_an_edit() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        overwrite(&a, &[b'a'; 64]);
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        *worker.fails.lock().unwrap() = vec![FailCode::Missing];
        let heard = Arc::default();
        let jobs = scheduler(&root, &worker, &heard);
        jobs.attach_preview(Arc::new(FakePreview::default()), None);
        let (_publish, rx) = watch::channel(feed(&[&a]));
        jobs.watch(rx);
        // The worker says the source went (as it does when it is moved mid-copy), and it has.
        eventually("a is missing", &jobs, || {
            reason_of(&jobs, &a).as_deref() == Some("copies.failed.missing")
        })
        .await;
        let away = dir.path().join("a.away");
        std::fs::rename(&a, &away).unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(worker.launched.lock().unwrap().len(), 1, "no retry loop");
        assert_eq!(
            reason_of(&jobs, &a).as_deref(),
            Some("copies.failed.missing")
        );

        std::fs::rename(&away, &a).unwrap();
        let back = std::time::Instant::now();
        let limit = super::ABSENT_EVERY + Duration::from_secs(2);
        while state_of(&jobs.list().jobs, &a) != Some(MediaJobState::Done) {
            assert!(
                back.elapsed() < limit,
                "not picked up in {limit:?}: {:?}",
                jobs.list().jobs
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        eprintln!("put back: copied again after {:?}", back.elapsed());
        let launched = worker.launched.lock().unwrap().clone();
        assert_eq!(launched.len(), 2);
        assert_eq!(launched[0].0, launched[1].0, "made on from its job");
    });
}

/// The recorder's hold reaches copies, as the app's sink holds them at arming and lets them go once the take has landed
/// (`recorder::hold_background`): the copy being made waits for the take, and is made on after it.
#[test]
fn a_take_s_hold_pauses_copies_until_it_lets_go() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        overwrite(&a, b"source a");
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        let stops = Arc::new(Notify::new());
        *worker.hold.lock().unwrap() = Some(stops.clone());
        let jobs = scheduler(&root, &worker, &Arc::default());
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        let (_publish, rx) = watch::channel(feed(&[&a]));
        jobs.watch(rx);
        eventually("a's copy is being made", &jobs, || {
            state_of(&jobs.list().jobs, &a) == Some(MediaJobState::Working)
        })
        .await;

        crate::recorder::hold_background(None, Some(&jobs), true);
        stops.notify_one();
        eventually("a's copy waits for the take", &jobs, || {
            reason_of(&jobs, &a).as_deref() == Some("copies.paused.recording")
        })
        .await;

        crate::recorder::hold_background(None, Some(&jobs), false);
        eventually("a's copy is made after the take", &jobs, || {
            preview.last().contains_key(&*a.to_string_lossy())
        })
        .await;
    });
}

/// A take holds copies from arming until it has landed: the copy being made pauses (its finished segments kept), its
/// row says why, nothing else starts meanwhile, and it is made on from its job once the take ends.
#[test]
fn a_take_pauses_the_copy_being_made_until_it_ends() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"));
        overwrite(&a, b"source a");
        overwrite(&b, b"source b");
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        let stops = Arc::new(Notify::new());
        *worker.hold.lock().unwrap() = Some(stops.clone());
        let jobs = scheduler(&root, &worker, &Arc::default());
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        let (_publish, rx) = watch::channel(feed(&[&a, &b]));
        jobs.watch(rx);
        eventually("a's copy is being made", &jobs, || {
            state_of(&jobs.list().jobs, &a) == Some(MediaJobState::Working)
        })
        .await;

        jobs.set_recording(true);
        // The worker stops after its current frame, as asked.
        stops.notify_one();
        eventually("a's copy waits for the take", &jobs, || {
            reason_of(&jobs, &a).as_deref() == Some("copies.paused.recording")
        })
        .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            worker.launched.lock().unwrap().len(),
            1,
            "nothing starts during the take"
        );

        jobs.set_recording(false);
        eventually("both copies are made after the take", &jobs, || {
            let map = preview.last();
            [&a, &b]
                .iter()
                .all(|p| map.contains_key(&*p.to_string_lossy()))
        })
        .await;
        let launched = worker.launched.lock().unwrap().clone();
        assert_eq!(launched.len(), 3, "{launched:?}");
        assert_eq!(launched[0].0, launched[1].0, "a was made on from its job");
    });
}

/// Analysis is the person's own action: while it decodes a clip on this Mac (its sound, its small copy), the copy being
/// made pauses, keeping its finished segments, and its row says why; once analysis stops decoding, the copy is made on
/// from its job.
#[test]
fn a_copy_being_made_pauses_while_analysis_decodes_and_goes_on_after() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        overwrite(&a, b"source a");
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        let stops = Arc::new(Notify::new());
        *worker.hold.lock().unwrap() = Some(stops.clone());
        let jobs = scheduler(&root, &worker, &Arc::default());
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        let (_publish, rx) = watch::channel(feed(&[&a]));
        jobs.watch(rx);
        eventually("a's copy is being made", &jobs, || {
            state_of(&jobs.list().jobs, &a) == Some(MediaJobState::Working)
        })
        .await;

        jobs.set_analyzing(true);
        // The worker stops after its current frame, as asked.
        stops.notify_one();
        eventually("a's copy waits for analysis", &jobs, || {
            reason_of(&jobs, &a).as_deref() == Some("copies.paused.analyzing")
        })
        .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(worker.launched.lock().unwrap().len(), 1);

        jobs.set_analyzing(false);
        eventually("a's copy is made after analysis", &jobs, || {
            preview.last().contains_key(&*a.to_string_lossy())
        })
        .await;
        let launched = worker.launched.lock().unwrap().clone();
        assert_eq!(launched.len(), 2, "{launched:?}");
        assert_eq!(launched[0].0, launched[1].0, "made on from its job");
    });
}

/// While analysis decodes a clip on this Mac, no copy starts, though the files are listed; once it stops, they are
/// made.
#[test]
fn no_copy_starts_while_analysis_decodes() {
    tauri::async_runtime::block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        overwrite(&a, b"source a");
        let root = dir.path().join("media/proxies");
        let worker = Arc::new(StandIn::default());
        let jobs = scheduler(&root, &worker, &Arc::default());
        jobs.set_analyzing(true);
        let preview = Arc::new(FakePreview::default());
        jobs.attach_preview(preview.clone(), None);
        let (_publish, rx) = watch::channel(feed(&[&a]));
        jobs.watch(rx);
        eventually("a is listed", &jobs, || jobs.list().jobs.len() == 1).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            worker.launched.lock().unwrap().is_empty(),
            "nothing starts while analysis decodes"
        );
        assert_eq!(
            state_of(&jobs.list().jobs, &a),
            Some(MediaJobState::Waiting)
        );

        jobs.set_analyzing(false);
        eventually("a's copy is made", &jobs, || {
            state_of(&jobs.list().jobs, &a) == Some(MediaJobState::Done)
        })
        .await;
    });
}

/// A video layer (picture in picture) gets its copy as main clips do: the timeline's files are listed in its order,
/// layers' included, before files it doesn't use; and once the copy is in the preview's map, the layer reads it with
/// the same times, while a plan without the map (export's) reads the file itself.
#[test]
fn a_video_layer_gets_its_copy_and_plays_from_it() {
    use klipolo_engine::plan::{Layer, PlanContext, VideoRole, frame_plan};
    use klipolo_model::layout::{Anchor, BoxN, Layout};
    use klipolo_model::project::FormatKind;
    use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, Track, TrackKind};

    let (main, layer, unused) = (
        Path::new("/v/main.mp4"),
        Path::new("/v/layer.mp4"),
        Path::new("/v/unused.mp4"),
    );
    let mut project = Project::new("Layers");
    project.assets = vec![video(unused), video(layer), video(main)];
    project.timeline.main = vec![Clip::new(
        ClipSource::Asset {
            id: "main.mp4".into(),
        },
        0,
        20_000_000,
    )];
    let mut track = Track::new(TrackKind::Overlay, "Picture in picture");
    track.items.push(Item {
        id: "pip".into(),
        start_us: 2_000_000,
        duration_us: 10_000_000,
        attach: None,
        layout: Layout::new(
            Anchor::TopLeft,
            BoxN {
                x: 0.6,
                y: 0.6,
                w: 0.35,
                h: 0.35,
            },
        ),
        content: ItemContent::Video {
            asset_id: "layer.mp4".into(),
            source_in_us: 1_000_000,
            speed: 1.0,
            volume: 1.0,
            muted: true,
            mirror: false,
            tie: None,
        },
        effects: Vec::new(),
    });
    project.timeline.tracks = vec![track];
    let view = super::decide::view_of(&project);
    let listed: Vec<&str> = view.paths.iter().map(|p| &**p).collect();
    assert_eq!(
        listed,
        ["/v/main.mp4", "/v/layer.mp4", "/v/unused.mp4"],
        "the layer's file is listed after the main footage under it"
    );

    let copy = MediaCopy::new("/cache/layer.mp4", 30_000.0 / 1001.0);
    let map: MediaMap = [(Arc::<str>::from("/v/layer.mp4"), copy.clone())]
        .into_iter()
        .collect();
    let project = Arc::new(project);
    let layer_source = |ctx: &PlanContext| {
        frame_plan(ctx, 5_000_000, FormatKind::Landscape)
            .layers
            .into_iter()
            .find_map(|l| match l {
                Layer::Video(v) if v.role == VideoRole::Overlay => Some(v.source),
                _ => None,
            })
            .expect("the layer is drawn at 5 s")
    };
    let preview = layer_source(&PlanContext::new(project.clone()).with_media(Arc::new(map)));
    assert_eq!(
        &*preview.path, "/cache/layer.mp4",
        "the preview reads the copy"
    );
    assert_eq!(&*preview.original, "/v/layer.mp4");
    assert_eq!(preview.time_us, 4_000_000, "at the same time in the file");
    let export = layer_source(&PlanContext::new(project));
    assert_eq!(
        &*export.path, "/v/layer.mp4",
        "export reads the file itself"
    );
}

/// The proxies folder keeps to its budget, least recently used first, never touching the open project's copies; a job
/// whose source changed (it can never resume) goes at once, one whose source is missing now (an unplugged drive) stays.
#[test]
fn the_tidy_keeps_the_budget_and_the_open_projects_copies() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proxies");
    std::fs::create_dir_all(&root).unwrap();
    let copy = |name: &str, age_s: u64| -> PathBuf {
        let path = root.join(format!("{name}.mp4"));
        std::fs::write(&path, vec![0u8; 100]).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(age_s))
            .unwrap();
        path
    };
    let (oldest, old, newer, newest) = (
        copy("oldest", 400),
        copy("old", 300),
        copy("newer", 200),
        copy("newest", 100),
    );
    // A job whose source was edited since, and one whose source isn't there now.
    let source = dir.path().join("talk.mp4");
    std::fs::write(&source, b"talk").unwrap();
    let policy = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
    let changed = ProxyJob::create(
        &root,
        &source.to_string_lossy(),
        &facts(&source).unwrap(),
        &policy,
    )
    .unwrap();
    std::fs::write(&source, b"talk, edited").unwrap();
    let gone_source = dir.path().join("drive/clip.mp4");
    std::fs::create_dir_all(gone_source.parent().unwrap()).unwrap();
    std::fs::write(&gone_source, b"clip").unwrap();
    let waiting = ProxyJob::create(
        &root,
        &gone_source.to_string_lossy(),
        &facts(&gone_source).unwrap(),
        &policy,
    )
    .unwrap();
    std::fs::remove_dir_all(gone_source.parent().unwrap()).unwrap();

    // 400 bytes of copies and the waiting job's manifest; room for all but two copies. The oldest is the open
    // project's.
    let manifest = std::fs::metadata(waiting.manifest_path()).unwrap().len();
    let keep = HashSet::from(["oldest".to_string()]);
    tidy_proxies(&root, 200 + manifest, &keep);
    assert!(
        !job_dir(&root, &changed.stem).exists(),
        "a changed source's job can never resume"
    );
    assert!(
        waiting.dir.exists(),
        "a missing source's job waits for its drive"
    );
    assert!(oldest.exists(), "the open project's copy stays");
    assert!(
        !old.exists() && !newer.exists(),
        "least recently used first"
    );
    assert!(newest.exists());
}

mod numbering;
