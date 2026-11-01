//! Follow the speaker in the app: the price before it runs (none for footage tracked before), the credit an access
//! code has left, the run itself (only what the clips play, each stretch once), and the paths it makes. The tracker is
//! a fake that sees one person, the small copy a fake (Linux has no H.264 encoder): nothing here reaches Meta.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_analysis::VideoError;
use klipolo_analysis::providers::{
    BoxFuture, ProviderError, SubjectTracker, TrackedBox, TrackedFrame, Tracking,
};
use klipolo_analysis::subject::{SubjectCopy, frames_for};
use klipolo_app::speaker::{NO_ONE, Speaker, SpeakerProgress, Trackers};
use klipolo_model::apply_ops;
use klipolo_model::fill_focus::FillFocus;
use klipolo_model::project::FormatKind::{Landscape, Portrait, Square};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource, FramingMode};

fn s(seconds: f64) -> Us {
    (seconds * 1e6) as Us
}

/// Sees one person at `x` on every frame (none when `x` is None), or at 0.7 in a copy of "b.mp4", counting its calls.
struct FakeSam {
    x: Option<f64>,
    calls: AtomicUsize,
    max_frames: std::sync::atomic::AtomicU64,
}

impl SubjectTracker for FakeSam {
    fn model(&self) -> &str {
        "fake-sam"
    }

    fn max_frames(&self) -> u64 {
        self.max_frames.load(Ordering::SeqCst)
    }

    fn estimate_usd(&self, frames: u64) -> f64 {
        frames.max(50) as f64 * 0.0002
    }

    fn track<'a>(
        &'a self,
        video: &'a Path,
        _phrase: &'a str,
    ) -> BoxFuture<'a, Result<Tracking, ProviderError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let copy = std::fs::read_to_string(video).unwrap();
            let (frames, src) = copy.split_once(' ').unwrap();
            let frames: u32 = frames.parse().unwrap();
            let x = if src.ends_with("b.mp4") {
                Some(0.7)
            } else {
                self.x
            };
            Ok(Tracking {
                frames: (0..frames)
                    .map(|frame| TrackedFrame {
                        frame,
                        boxes: (x.iter())
                            .map(|x| TrackedBox {
                                id: 1,
                                left: x - 0.1,
                                top: 0.1,
                                right: x + 0.1,
                                bottom: 1.0,
                                confidence: Some(0.9),
                            })
                            .collect(),
                    })
                    .collect(),
                frames_billed: u64::from(frames),
                cost_usd: f64::from(frames.max(50)) * 0.0002,
                cut_off: false,
            })
        })
    }
}

/// Its tracker, or why there is none, and the credit an access code has left.
struct FakeTrackers {
    sam: Option<Arc<FakeSam>>,
    credit_left: Option<f64>,
}

impl Trackers for FakeTrackers {
    fn tracker(&self) -> BoxFuture<'_, Result<Arc<dyn SubjectTracker>, String>> {
        Box::pin(async move {
            (self.sam.clone())
                .map(|sam| sam as Arc<dyn SubjectTracker>)
                .ok_or_else(|| "Add your Meta API key or access code in Settings.".to_owned())
        })
    }

    fn credit_left(&self) -> BoxFuture<'_, Option<f64>> {
        Box::pin(async move { self.credit_left })
    }
}

/// Writes its frame count and its source as the copy.
#[derive(Default)]
struct FakeCopy {
    made: Mutex<Vec<RangeUs>>,
}

impl SubjectCopy for FakeCopy {
    fn copy(
        &self,
        src: &Path,
        range: RangeUs,
        fps: f64,
        out: &Path,
        _cancel: &AtomicBool,
    ) -> Result<Vec<Us>, VideoError> {
        self.made.lock().unwrap().push(range);
        let frames = frames_for(range.end - range.start, fps);
        std::fs::write(out, format!("{frames} {}", src.display())).unwrap();
        Ok(vec![])
    }
}

struct Setup {
    _dir: tempfile::TempDir,
    project: Project,
    sam: Arc<FakeSam>,
    copy: Arc<FakeCopy>,
    progress: Arc<Mutex<Vec<SpeakerProgress>>>,
}

/// A 10 minute talk (an imported 16:9 video, its file real so it has a cache key) cut to one minute, 60 to 120 s.
fn setup(x: Option<f64>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("talk.mp4");
    std::fs::write(&file, b"the talk's bytes").unwrap();
    let mut project = Project::new("Talk");
    project.assets = vec![Asset {
        id: "talk".into(),
        kind: AssetKind::Video,
        path: file.to_string_lossy().into_owned(),
        name: "talk.mp4".into(),
        duration_us: s(600.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }];
    project.settings.formats = vec![Landscape, Portrait];
    project.settings.primary_format = Portrait;
    let mut clip = Clip::new(ClipSource::Asset { id: "talk".into() }, s(60.0), s(120.0));
    clip.id = "c1".into();
    project.timeline.main = vec![clip];
    Setup {
        _dir: dir,
        project,
        sam: Arc::new(FakeSam {
            x,
            calls: AtomicUsize::new(0),
            max_frames: std::sync::atomic::AtomicU64::new(15_000),
        }),
        copy: Arc::new(FakeCopy::default()),
        progress: Arc::default(),
    }
}

fn speaker(setup: &Setup, credit_left: Option<f64>, with_key: bool) -> Speaker {
    let root = setup._dir.path();
    let heard = setup.progress.clone();
    Speaker::new(
        Arc::new(FakeTrackers {
            sam: with_key.then(|| setup.sam.clone()),
            credit_left,
        }),
        setup.copy.clone(),
        root.join("analysis"),
        root.join("work"),
        Arc::new(move |p| heard.lock().unwrap().push(p)),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_price_comes_first_and_footage_tracked_before_is_free() {
    let setup = setup(Some(0.3));
    let speaker = speaker(&setup, None, true);
    let p = &setup.project;
    let ids = vec!["c1".to_owned()];
    // One minute at 2 frames a second: 120 frames, $0.024.
    let estimate = speaker.estimate(p, &ids, &[Portrait], "person").await;
    assert!((estimate.usd - 0.024).abs() < 1e-12, "{estimate:?}");
    assert_eq!(estimate.clips, ids);
    assert!(!estimate.cached && estimate.refusal.is_none());
    assert_eq!(
        setup.sam.calls.load(Ordering::SeqCst),
        0,
        "pricing calls nothing"
    );

    let followed = speaker
        .follow(p, &ids, &[Portrait], "person")
        .await
        .unwrap();
    assert!((followed.paid_usd - 0.024).abs() < 1e-12);
    assert!((speaker.spent_usd() - 0.024).abs() < 1e-12);
    // Only the minute the clip plays was copied and sent.
    assert_eq!(
        *setup.copy.made.lock().unwrap(),
        [RangeUs {
            start: s(60.0),
            end: s(120.0)
        }]
    );
    // Its progress went out, ending done.
    let heard = setup.progress.lock().unwrap().clone();
    assert!(heard.last().is_some_and(|p| p.pct == 100), "{heard:?}");

    // Again: from the cache, free, nothing sent.
    let again = speaker
        .estimate(p, &ids, &[Portrait, Square], "person")
        .await;
    assert!(again.cached && again.usd == 0.0, "{again:?}");
    let followed = speaker
        .follow(p, &ids, &[Portrait, Square], "person")
        .await
        .unwrap();
    assert_eq!(followed.paid_usd, 0.0);
    assert_eq!(setup.sam.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_paths_make_the_clip_follow_in_one_step() {
    let setup = setup(Some(0.3));
    let speaker = speaker(&setup, None, true);
    let p = &setup.project;
    let followed = speaker
        .follow(p, &["c1".to_owned()], &[Landscape, Portrait], "person")
        .await
        .unwrap();
    // 16:9 footage fills 16:9 whole: only the vertical version follows.
    let ops = followed.ops(p);
    assert_eq!(ops.len(), 1);
    let next = apply_ops(p, &ops).unwrap();
    let clip = &next.timeline.main[0];
    assert_eq!(clip.framing[&Portrait].mode, FramingMode::Fill);
    let Some(FillFocus::Speaker { path }) = clip.fill_focus.get(&Portrait) else {
        panic!("{:?}", clip.fill_focus)
    };
    assert!(!clip.fill_focus.contains_key(&Landscape));
    assert!((path.centre_at(s(90.0)).unwrap().0 - 0.3).abs() < 0.02);
    // A clip that's gone by the time it lands is left out.
    let mut gone = p.clone();
    gone.timeline.main.clear();
    assert!(followed.ops(&gone).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_found_is_said_and_changes_nothing() {
    let setup = setup(None);
    let speaker = speaker(&setup, None, true);
    let followed = speaker
        .follow(&setup.project, &["c1".to_owned()], &[Portrait], "person")
        .await
        .unwrap();
    assert_eq!(followed.not_found, ["c1"]);
    assert!(followed.ops(&setup.project).is_empty());
    // Still paid for.
    assert!(followed.paid_usd > 0.0);
    assert!(NO_ONE.contains("person"), "{NO_ONE}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codes_credit_and_a_missing_key_refuse_before_anything_is_sent() {
    let setup = setup(Some(0.3));
    let ids = vec!["c1".to_owned()];
    let low = speaker(&setup, Some(0.01), true);
    let estimate = low
        .estimate(&setup.project, &ids, &[Portrait], "person")
        .await;
    let refusal = estimate.refusal.expect("over the credit left");
    assert!(
        refusal.contains("$0.01") && refusal.contains("$0.02"),
        "{refusal}"
    );
    assert_eq!(
        low.follow(&setup.project, &ids, &[Portrait], "person")
            .await
            .unwrap_err(),
        refusal
    );

    let none = speaker(&setup, None, false);
    let estimate = none
        .estimate(&setup.project, &ids, &[Portrait], "person")
        .await;
    assert_eq!(
        estimate.refusal.as_deref(),
        Some("Add your Meta API key or access code in Settings.")
    );
    assert!(
        none.follow(&setup.project, &ids, &[Portrait], "person")
            .await
            .is_err()
    );
    assert_eq!(setup.sam.calls.load(Ordering::SeqCst), 0);
    assert!(setup.copy.made.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn only_clips_that_can_follow_are_priced() {
    let mut setup = setup(Some(0.3));
    // A picture on the main track can't follow anyone.
    setup.project.assets.push(Asset {
        id: "logo".into(),
        kind: AssetKind::Image,
        path: PathBuf::from("/media/logo.png")
            .to_string_lossy()
            .into_owned(),
        name: "logo.png".into(),
        duration_us: 0,
        width: 512,
        height: 512,
        fps: 0.0,
        rotation: None,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
    });
    let mut logo = Clip::new(ClipSource::Asset { id: "logo".into() }, 0, s(5.0));
    logo.id = "logo1".into();
    setup.project.timeline.main.push(logo);
    let speaker = speaker(&setup, None, true);
    let ids = vec!["c1".to_owned(), "logo1".to_owned()];
    let estimate = speaker
        .estimate(&setup.project, &ids, &[Portrait], "person")
        .await;
    assert_eq!(estimate.clips, ["c1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_clip_follows_the_speaker_of_its_own_file() {
    // Two files whose clips play overlapping stretches of each: the talk's 60 to 120 s, and "b.mp4"'s 60 to 90 s.
    let mut setup = setup(Some(0.3));
    let file = setup._dir.path().join("b.mp4");
    std::fs::write(&file, b"another video's bytes").unwrap();
    let mut other = setup.project.assets[0].clone();
    other.id = "other".into();
    other.path = file.to_string_lossy().into_owned();
    setup.project.assets.push(other);
    let mut clip = Clip::new(ClipSource::Asset { id: "other".into() }, s(60.0), s(90.0));
    clip.id = "c2".into();
    setup.project.timeline.main.push(clip);
    let speaker = speaker(&setup, None, true);
    let p = &setup.project;
    let ids = vec!["c1".to_owned(), "c2".to_owned()];
    let followed = speaker
        .follow(p, &ids, &[Portrait], "person")
        .await
        .unwrap();
    let next = apply_ops(p, &followed.ops(p)).unwrap();
    let x = |i: usize, at: f64| match next.timeline.main[i].fill_focus.get(&Portrait) {
        Some(FillFocus::Speaker { path }) => path.centre_at(s(at)).unwrap().0,
        other => panic!("{other:?}"),
    };
    assert!((x(0, 80.0) - 0.3).abs() < 0.02, "{}", x(0, 80.0));
    assert!((x(1, 80.0) - 0.7).abs() < 0.02, "{}", x(1, 80.0));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_format_on_fit_is_left_alone() {
    let mut setup = setup(Some(0.3));
    setup.project.settings.formats = vec![Landscape, Portrait, Square];
    let fit = klipolo_model::timeline::Framing {
        mode: FramingMode::Fit,
        focus_x: 0.5,
        focus_y: 0.5,
        follow: None,
    };
    setup.project.timeline.main[0].framing.insert(Square, fit);
    let speaker = speaker(&setup, None, true);
    let p = &setup.project;
    let followed = speaker
        .follow(
            p,
            &["c1".to_owned()],
            &[Landscape, Portrait, Square],
            "person",
        )
        .await
        .unwrap();
    let next = apply_ops(p, &followed.ops(p)).unwrap();
    let clip = &next.timeline.main[0];
    assert!(clip.fill_focus.contains_key(&Portrait));
    assert!(!clip.fill_focus.contains_key(&Square));
    assert_eq!(clip.framing[&Square].mode, FramingMode::Fit);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_format_turned_to_fit_while_tracking_stays_fit() {
    let setup = setup(Some(0.3));
    let speaker = speaker(&setup, None, true);
    let p = &setup.project;
    let followed = speaker
        .follow(p, &["c1".to_owned()], &[Portrait], "person")
        .await
        .unwrap();
    // The person turned the vertical version to Fit while it tracked: landing leaves it so.
    let mut now = p.clone();
    let fit = klipolo_model::timeline::Framing {
        mode: FramingMode::Fit,
        focus_x: 0.5,
        focus_y: 0.5,
        follow: None,
    };
    now.timeline.main[0].framing.insert(Portrait, fit);
    assert!(followed.ops(&now).is_empty());
    assert_eq!(followed.ops(p).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn footage_longer_than_the_tracker_takes_goes_in_parts_priced_together() {
    // 15 minutes of the talk, through a tracker that takes 1,180 frames a video (the relay's): two parts.
    let mut setup = setup(Some(0.3));
    setup.sam.max_frames.store(1_180, Ordering::SeqCst);
    setup.project.assets[0].duration_us = s(1_200.0);
    let clip = &mut setup.project.timeline.main[0];
    (clip.source_in_us, clip.source_out_us) = (0, s(900.0));
    let speaker = speaker(&setup, None, true);
    let p = &setup.project;
    let ids = vec!["c1".to_owned()];
    let estimate = speaker.estimate(p, &ids, &[Portrait], "person").await;
    assert!(
        (estimate.usd - 1_800.0 * 0.0002).abs() < 1e-9,
        "{estimate:?}"
    );
    let followed = speaker
        .follow(p, &ids, &[Portrait], "person")
        .await
        .unwrap();
    assert_eq!(
        *setup.copy.made.lock().unwrap(),
        [
            RangeUs {
                start: 0,
                end: s(590.0)
            },
            RangeUs {
                start: s(590.0),
                end: s(900.0)
            }
        ]
    );
    assert_eq!(setup.sam.calls.load(Ordering::SeqCst), 2);
    assert!((followed.paid_usd - estimate.usd).abs() < 1e-9);
    // The parts' tracks make one path across the whole clip.
    let next = apply_ops(p, &followed.ops(p)).unwrap();
    let Some(FillFocus::Speaker { path }) = next.timeline.main[0].fill_focus.get(&Portrait) else {
        panic!()
    };
    for at in [10.0, 600.0, 890.0] {
        assert!(
            (path.centre_at(s(at)).unwrap().0 - 0.3).abs() < 0.02,
            "at {at}"
        );
    }
}
