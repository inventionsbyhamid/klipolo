//! Following whoever speaks in a clip with two people: the voices matched to them priced with the tracking, asked
//! only where the people don't all fit, cached so a second follow is free, and who the card keeps (one person, or
//! both) made from the cache at no charge. Fakes for SAM, Muse Spark and the small copy: nothing here reaches Meta.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_analysis::VideoError;
use klipolo_analysis::providers::{
    BoxFuture, Heard, ProviderError, ProviderErrorKind, ScreenBox, SubjectTracker, TrackedBox,
    TrackedFrame, Tracking, VoiceAnswer, VoiceMatcher,
};
use klipolo_analysis::subject::voices::VoiceTurn;
use klipolo_analysis::subject::{SubjectCopy, frames_for};
use klipolo_app::speaker::{LOW_CREDIT_VOICES, NOT_TRACKED, Speaker, Trackers, UNMATCHED, Voices};
use klipolo_model::apply_ops;
use klipolo_model::fill_focus::{FillFocus, FocusPath, Keep, Person, Voice};
use klipolo_model::project::FormatKind::{Landscape, Portrait};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource};

const SPARK: f64 = 0.003;
const ONE_MINUTE: f64 = 0.024;

fn s(seconds: f64) -> Us {
    (seconds * 1e6).round() as Us
}

/// Sees two people far apart on every frame: tracked as 7 on the left and 3 on the right.
#[derive(Default)]
struct TwoPeople {
    calls: AtomicUsize,
}

impl SubjectTracker for TwoPeople {
    fn model(&self) -> &str {
        "fake-sam"
    }

    fn max_frames(&self) -> u64 {
        15_000
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
            let frames: u32 = std::fs::read_to_string(video).unwrap().parse().unwrap();
            let at = |id, x: f64| TrackedBox {
                id,
                left: x - 0.1,
                top: 0.1,
                right: x + 0.1,
                bottom: 1.0,
                confidence: Some(0.9),
            };
            Ok(Tracking {
                frames: (0..frames)
                    .map(|frame| TrackedFrame {
                        frame,
                        boxes: vec![at(3, 0.75), at(7, 0.25)],
                    })
                    .collect(),
                frames_billed: u64::from(frames),
                cost_usd: f64::from(frames.max(50)) * 0.0002,
                cut_off: false,
            })
        })
    }
}

/// Says the person on the left speaks before 80 s and from 100 s, the one on the right in between; or fails from its
/// `fail_from`th call (counting from 1), or says nothing clear.
struct FakeSpark {
    asked: AtomicUsize,
    fail_from: usize,
    unclear: bool,
}

impl Default for FakeSpark {
    fn default() -> Self {
        Self {
            asked: AtomicUsize::new(0),
            fail_from: usize::MAX,
            unclear: false,
        }
    }
}

impl VoiceMatcher for FakeSpark {
    fn model(&self) -> &str {
        "fake-spark"
    }

    fn estimate_usd(&self, seconds: f64) -> f64 {
        SPARK * seconds / 3.0
    }

    fn who_speaks<'a>(
        &'a self,
        video: &'a Path,
        _people: &'a [ScreenBox],
    ) -> BoxFuture<'a, Result<VoiceAnswer, ProviderError>> {
        Box::pin(async move {
            let n = self.asked.fetch_add(1, Ordering::SeqCst) + 1;
            if n >= self.fail_from {
                return Err(ProviderError::new(
                    ProviderErrorKind::Failed,
                    "Meta failed.",
                ));
            }
            let start: Us = std::fs::read_to_string(video).unwrap().parse().unwrap();
            let person = if (s(80.0)..s(100.0)).contains(&start) {
                2
            } else {
                1
            };
            Ok(VoiceAnswer {
                heard: if self.unclear {
                    Heard::Unclear
                } else {
                    Heard::Person { person }
                },
                cost_usd: SPARK,
            })
        })
    }
}

struct FakeTrackers {
    sam: Arc<TwoPeople>,
    spark: Option<Arc<FakeSpark>>,
    credit_left: Option<f64>,
}

impl Trackers for FakeTrackers {
    fn tracker(&self) -> BoxFuture<'_, Result<Arc<dyn SubjectTracker>, String>> {
        Box::pin(async move { Ok(self.sam.clone() as Arc<dyn SubjectTracker>) })
    }

    fn credit_left(&self) -> BoxFuture<'_, Option<f64>> {
        Box::pin(async move { self.credit_left })
    }

    fn matcher(&self) -> BoxFuture<'_, Option<Arc<dyn VoiceMatcher>>> {
        Box::pin(async move { self.spark.clone().map(|s| s as Arc<dyn VoiceMatcher>) })
    }
}

/// The copy holds its frame count; the copy with sound, where it starts.
struct FakeCopy;

impl SubjectCopy for FakeCopy {
    fn copy(
        &self,
        _src: &Path,
        range: RangeUs,
        fps: f64,
        out: &Path,
        _cancel: &AtomicBool,
    ) -> Result<Vec<Us>, VideoError> {
        std::fs::write(out, frames_for(range.end - range.start, fps).to_string()).unwrap();
        Ok(vec![])
    }

    fn copy_with_sound(
        &self,
        _src: &Path,
        range: RangeUs,
        _fps: f64,
        out: &Path,
        _cancel: &AtomicBool,
    ) -> Result<(), VideoError> {
        std::fs::write(out, range.start.to_string()).unwrap();
        Ok(())
    }
}

/// The talk's transcript: A (part 0) to 80 s and from 100 s, B in between, in its sound time.
struct Heard2 {
    asked: Mutex<Vec<String>>,
    labels: (String, String),
}

impl Voices for Heard2 {
    fn turns(&self, asset_id: &str) -> Vec<VoiceTurn> {
        self.asked.lock().unwrap().push(asset_id.to_owned());
        let turn = |from: f64, to: f64, label: &str| VoiceTurn {
            start_us: s(from),
            end_us: s(to),
            voice: Voice {
                part: 0,
                label: label.into(),
            },
        };
        let (a, b) = (&self.labels.0, &self.labels.1);
        vec![
            turn(60.0, 80.0, a),
            turn(80.0, 100.0, b),
            turn(100.0, 120.0, a),
        ]
    }
}

struct Setup {
    _dir: tempfile::TempDir,
    project: Project,
    sam: Arc<TwoPeople>,
    spark: Arc<FakeSpark>,
}

/// A two-person talk (16:9) cut to 60 to 120 s, in a vertical version, Spark failing from its `fail_from`th call.
fn setup(fail_from: usize) -> Setup {
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
        sam: Arc::default(),
        spark: Arc::new(FakeSpark {
            fail_from,
            ..FakeSpark::default()
        }),
    }
}

fn speaker(setup: &Setup, matcher: bool) -> Speaker {
    speaker_with(setup, matcher, None, ("A", "B"))
}

/// The speaker following with Spark when `matcher`, an access code's `credit_left`, and the transcript's `labels`.
fn speaker_with(
    setup: &Setup,
    matcher: bool,
    credit_left: Option<f64>,
    labels: (&str, &str),
) -> Speaker {
    let root = setup._dir.path();
    Speaker::new(
        Arc::new(FakeTrackers {
            sam: setup.sam.clone(),
            spark: matcher.then(|| setup.spark.clone()),
            credit_left,
        }),
        Arc::new(FakeCopy),
        root.join("analysis"),
        root.join("work"),
        Arc::new(|_| {}),
    )
    .with_voices(Arc::new(Heard2 {
        asked: Mutex::new(Vec::new()),
        labels: (labels.0.to_owned(), labels.1.to_owned()),
    }))
}

fn focus(p: &Project) -> (FocusPath, Keep, Vec<Person>) {
    match p.timeline.main[0].fill_focus.get(&Portrait) {
        Some(FillFocus::Speaker { path, keep, people }) => {
            (path.clone(), keep.clone(), people.clone())
        }
        other => panic!("{other:?}"),
    }
}

fn x(path: &FocusPath, at: f64) -> f64 {
    path.centre_at(s(at)).unwrap().0
}

fn ids() -> Vec<String> {
    vec!["c1".to_owned()]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_two_person_clip_follows_whoever_speaks_priced_with_the_tracking() {
    let setup = setup(usize::MAX);
    let speaker = speaker(&setup, true);
    let p = &setup.project;
    // Before anything is tracked: the minute's frames, and at most three windows for each voice (each talks alone for
    // 20 s or more).
    let estimate = speaker.estimate(p, &ids(), &[Portrait], "person").await;
    assert!(
        (estimate.usd - (ONE_MINUTE + 6.0 * SPARK)).abs() < 1e-9,
        "{estimate:?}"
    );
    assert_eq!(
        setup.spark.asked.load(Ordering::SeqCst),
        0,
        "pricing asks nothing"
    );

    let followed = speaker
        .follow(p, &ids(), &[Portrait], "person")
        .await
        .unwrap();
    assert_eq!(setup.spark.asked.load(Ordering::SeqCst), 3);
    assert!(
        (followed.paid_usd - (ONE_MINUTE + 3.0 * SPARK)).abs() < 1e-9,
        "{}",
        followed.paid_usd
    );
    assert!((speaker.spent_usd() - followed.paid_usd).abs() < 1e-12);
    let next = apply_ops(p, &followed.ops(p)).unwrap();
    let (path, keep, people) = focus(&next);
    assert!((x(&path, 70.0) - 0.25).abs() < 0.03, "{path:?}");
    assert!((x(&path, 90.0) - 0.75).abs() < 0.03, "{path:?}");
    assert!((x(&path, 110.0) - 0.25).abs() < 0.03, "{path:?}");
    assert_eq!(keep, Keep::Speaker);
    let voice = |label: &str| Voice {
        part: 0,
        label: label.into(),
    };
    assert_eq!(
        people,
        [
            Person {
                voices: vec![voice("A")],
                share: 67,
            },
            Person {
                voices: vec![voice("B")],
                share: 33,
            },
        ]
    );

    // Again: all from the cache, free.
    let again = speaker.estimate(p, &ids(), &[Portrait], "person").await;
    assert!(again.cached && again.usd == 0.0, "{again:?}");
    let followed = speaker
        .follow(p, &ids(), &[Portrait], "person")
        .await
        .unwrap();
    assert_eq!(followed.paid_usd, 0.0);
    assert_eq!(setup.spark.asked.load(Ordering::SeqCst), 3);
    assert_eq!(setup.sam.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn who_the_card_keeps_is_made_from_the_cache_at_no_charge() {
    let setup = setup(usize::MAX);
    let speaker = speaker(&setup, true);
    let p = &setup.project;
    speaker
        .follow(p, &ids(), &[Portrait], "person")
        .await
        .unwrap();
    for keep in [Keep::Person { person: 2 }, Keep::Both] {
        let keeps = BTreeMap::from([("c1".to_owned(), keep.clone())]);
        let followed = speaker
            .follow_keeping(p, &ids(), &[Portrait], "person", &keeps)
            .await
            .unwrap();
        assert_eq!(followed.paid_usd, 0.0);
        let next = apply_ops(p, &followed.ops(p)).unwrap();
        let (path, kept, people) = focus(&next);
        assert_eq!(kept, keep);
        assert_eq!(people.len(), 2);
        // The person on the right said less: Both stays with the one on the left; P2 is on the right all along.
        let want = if keep == Keep::Both { 0.25 } else { 0.75 };
        for at in [70.0, 90.0, 110.0] {
            assert!(
                (x(&path, at) - want).abs() < 0.03,
                "{keep:?} at {at}: {path:?}"
            );
        }
    }
    assert_eq!(setup.spark.asked.load(Ordering::SeqCst), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_matcher_or_when_it_fails_the_follow_still_lands() {
    for (matcher, fail_from) in [(false, usize::MAX), (true, 1)] {
        let setup = setup(fail_from);
        let speaker = speaker(&setup, matcher);
        let p = &setup.project;
        let estimate = speaker.estimate(p, &ids(), &[Portrait], "person").await;
        let want = if matcher {
            ONE_MINUTE + 6.0 * SPARK
        } else {
            ONE_MINUTE
        };
        assert!((estimate.usd - want).abs() < 1e-9, "{estimate:?}");
        let followed = speaker
            .follow(p, &ids(), &[Portrait], "person")
            .await
            .unwrap();
        let next = apply_ops(p, &followed.ops(p)).unwrap();
        let (path, _, people) = focus(&next);
        // Nobody heard: the main subject as before, both people still named.
        assert!(path.is_valid());
        assert_eq!(people.len(), 2);
        assert!(people.iter().all(|p| p.voices.is_empty()));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn another_phrase_follows_that_and_asks_nobody_whose_voice_it_is() {
    let setup = setup(usize::MAX);
    let speaker = speaker(&setup, true);
    let p = &setup.project;
    let estimate = speaker.estimate(p, &ids(), &[Portrait], "red car").await;
    assert!((estimate.usd - ONE_MINUTE).abs() < 1e-9, "{estimate:?}");
    let followed = speaker
        .follow(p, &ids(), &[Portrait], "red car")
        .await
        .unwrap();
    assert_eq!(setup.spark.asked.load(Ordering::SeqCst), 0);
    // Two of them on screen, but no people to choose between on the card.
    let next = apply_ops(p, &followed.ops(p)).unwrap();
    assert!(focus(&next).2.is_empty());
}

#[test]
fn chat_reads_each_matched_voice_as_its_persons_number() {
    use klipolo_analysis::transcript::Turn;
    use klipolo_app::analysis::chat::{people_voices, person_turns};
    let setup = setup(usize::MAX);
    let mut p = setup.project.clone();
    let voice = |part: u32, label: &str| Voice {
        part,
        label: label.into(),
    };
    p.timeline.main[0].fill_focus.insert(
        Portrait,
        FillFocus::Speaker {
            path: FocusPath::default(),
            keep: Keep::Speaker,
            people: vec![
                Person {
                    voices: vec![voice(0, "A"), voice(1, "B")],
                    share: 60,
                },
                Person {
                    voices: vec![voice(0, "B")],
                    share: 30,
                },
            ],
        },
    );
    let people = people_voices(&p, "talk");
    let turn = |chunk: u32, label: &str| Turn {
        start_us: 0,
        end_us: 1,
        text: String::new(),
        speaker: Some(label.into()),
        chunk,
        aligned: true,
    };
    let said: Vec<Option<String>> = person_turns(
        &[
            turn(0, "A"),
            turn(0, "B"),
            turn(1, "A"),
            turn(1, "B"),
            turn(0, "C"),
        ],
        &people,
    )
    .into_iter()
    .map(|t| t.speaker)
    .collect();
    // Each part's own "A" and "B"; a voice nobody was matched to keeps its label.
    let want = ["P1", "P2", "A", "P1", "C"].map(|l| Some(l.to_owned()));
    assert_eq!(said, want);
    assert!(people_voices(&p, "other").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_match_that_stops_partway_follows_the_main_person_and_says_so() {
    // A's two windows agree, then B's first call fails.
    let setup = setup(3);
    let speaker = speaker(&setup, true);
    let p = &setup.project;
    let followed = speaker
        .follow(p, &ids(), &[Portrait], "person")
        .await
        .unwrap();
    assert_eq!(setup.spark.asked.load(Ordering::SeqCst), 3);
    assert_eq!(followed.note.as_deref(), Some(UNMATCHED));
    // What was asked is paid for, and counted.
    assert!(
        (followed.paid_usd - (ONE_MINUTE + 2.0 * SPARK)).abs() < 1e-9,
        "{}",
        followed.paid_usd
    );
    // No voice is used: the main person all along, never a half state that stays on A.
    let next = apply_ops(p, &followed.ops(p)).unwrap();
    let (path, _, people) = focus(&next);
    assert!((x(&path, 70.0) - x(&path, 90.0)).abs() < 1e-9, "{path:?}");
    assert!(people.iter().all(|p| p.voices.is_empty()));
    assert!(UNMATCHED.contains("main person") && !UNMATCHED.contains('\u{2014}'));
}

#[tokio::test(flavor = "multi_thread")]
async fn one_file_in_many_clips_asks_at_most_three_windows_a_voice_and_is_priced_once() {
    let mut setup = setup(usize::MAX);
    // Six clips of one file, 10 s each; Spark never says clearly, so every window would be asked.
    let clip = setup.project.timeline.main[0].clone();
    setup.project.timeline.main = (0..6)
        .map(|i| {
            let mut c = clip.clone();
            c.id = format!("c{}", i + 1);
            c.source_in_us = s(60.0 + 10.0 * f64::from(i));
            c.source_out_us = c.source_in_us + s(10.0);
            c
        })
        .collect();
    setup.spark = Arc::new(FakeSpark {
        unclear: true,
        ..FakeSpark::default()
    });
    let speaker = speaker(&setup, true);
    let p = &setup.project;
    let all: Vec<String> = p.timeline.main.iter().map(|c| c.id.clone()).collect();
    let estimate = speaker.estimate(p, &all, &[Portrait], "person").await;
    assert!(
        (estimate.usd - (ONE_MINUTE + 6.0 * SPARK)).abs() < 1e-9,
        "{estimate:?}"
    );
    let followed = speaker
        .follow(p, &all, &[Portrait], "person")
        .await
        .unwrap();
    // A plays in four clips and B in two: three windows for A, two for B.
    assert_eq!(setup.spark.asked.load(Ordering::SeqCst), 5);
    assert!(followed.paid_usd <= estimate.usd + 1e-9);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_card_keeps_from_the_tracking_alone_and_never_pays() {
    // A follow whose matching stopped partway: the card's choices still work, from the cache, asking nothing.
    let setup = setup(3);
    let speaker = speaker(&setup, true);
    let p = &setup.project;
    let keeps = BTreeMap::from([("c1".to_owned(), Keep::Person { person: 2 })]);
    // Nothing tracked yet: refused, and nothing sent.
    assert_eq!(
        speaker
            .keep(p, &ids(), &[Portrait], &keeps)
            .await
            .unwrap_err(),
        NOT_TRACKED
    );
    assert_eq!(setup.sam.calls.load(Ordering::SeqCst), 0);
    speaker
        .follow(p, &ids(), &[Portrait], "person")
        .await
        .unwrap();
    let asked = setup.spark.asked.load(Ordering::SeqCst);
    let spent = speaker.spent_usd();
    for keep in [Keep::Person { person: 2 }, Keep::Both, Keep::Speaker] {
        let keeps = BTreeMap::from([("c1".to_owned(), keep.clone())]);
        let followed = speaker.keep(p, &ids(), &[Portrait], &keeps).await.unwrap();
        assert_eq!(followed.paid_usd, 0.0);
        let next = apply_ops(p, &followed.ops(p)).unwrap();
        let (path, kept, _) = focus(&next);
        assert_eq!(kept, keep);
        if keep == (Keep::Person { person: 2 }) {
            assert!((x(&path, 70.0) - 0.75).abs() < 0.03, "{path:?}");
        }
    }
    assert_eq!(setup.spark.asked.load(Ordering::SeqCst), asked);
    assert_eq!(setup.sam.calls.load(Ordering::SeqCst), 1);
    assert_eq!(speaker.spent_usd(), spent);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_access_code_too_low_for_a_media_call_skips_matching_and_says_so() {
    let setup = setup(usize::MAX);
    let speaker = speaker_with(&setup, true, Some(0.5), ("A", "B"));
    let p = &setup.project;
    let estimate = speaker.estimate(p, &ids(), &[Portrait], "person").await;
    assert!((estimate.usd - ONE_MINUTE).abs() < 1e-9, "{estimate:?}");
    let followed = speaker
        .follow(p, &ids(), &[Portrait], "person")
        .await
        .unwrap();
    assert_eq!(setup.spark.asked.load(Ordering::SeqCst), 0);
    assert_eq!(followed.note.as_deref(), Some(LOW_CREDIT_VOICES));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_label_the_project_cant_keep_is_left_off_the_people() {
    let setup = setup(usize::MAX);
    let long = "a voice label far too long to keep";
    let speaker = speaker_with(&setup, true, None, ("", long));
    let p = &setup.project;
    let followed = speaker
        .follow(p, &ids(), &[Portrait], "person")
        .await
        .unwrap();
    // The follow lands: its people just don't name those voices.
    let next = apply_ops(p, &followed.ops(p)).unwrap();
    let (_, _, people) = focus(&next);
    assert_eq!(people.len(), 2);
    assert!(people.iter().all(|p| p.voices.is_empty()), "{people:?}");
}
