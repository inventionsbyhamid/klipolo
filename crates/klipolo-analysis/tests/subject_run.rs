//! Tracking the footage a clip plays: only its stretch of the file, at 2 frames a second, priced before it runs, and
//! cached by the file's content and the stretch so the same footage is never paid for twice. With a fake tracker and a
//! fake small copy (Linux has no H.264 encoder): nothing here reaches Meta.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use klipolo_analysis::VideoError;
use klipolo_analysis::cache::{Dir, content_key};
use klipolo_analysis::providers::{
    BoxFuture, ProviderError, ProviderErrorKind, SubjectTracker, TrackedBox, TrackedFrame, Tracking,
};
use klipolo_analysis::subject::{self, Ask, SubjectCopy};
use klipolo_model::time::{RangeUs, Us};

/// Answers every video with one person on each of its frames, counting the calls and the phrases. `cut_off`: answers
/// only the first half of each video's frames, as an answer Meta stopped part-way does.
#[derive(Default)]
struct FakeSam {
    calls: AtomicUsize,
    phrases: Mutex<Vec<String>>,
    fail: bool,
    cut_off: bool,
    model: Option<&'static str>,
}

impl SubjectTracker for FakeSam {
    fn model(&self) -> &str {
        self.model.unwrap_or("fake-sam")
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
        phrase: &'a str,
    ) -> BoxFuture<'a, Result<Tracking, ProviderError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.phrases.lock().unwrap().push(phrase.to_owned());
            if self.fail {
                return Err(ProviderError::new(
                    ProviderErrorKind::Budget,
                    "No credit left.",
                ));
            }
            // The fake copy holds its frame count.
            let all: u32 = std::fs::read_to_string(video).unwrap().parse().unwrap();
            let frames = if self.cut_off { all / 2 } else { all };
            Ok(Tracking {
                frames: (0..frames)
                    .map(|frame| TrackedFrame {
                        frame,
                        boxes: vec![TrackedBox {
                            id: 1,
                            left: 0.2,
                            top: 0.1,
                            right: 0.4,
                            bottom: 1.0,
                            confidence: Some(0.9),
                        }],
                    })
                    .collect(),
                frames_billed: u64::from(all),
                cost_usd: f64::from(all.max(50)) * 0.0002,
                cut_off: self.cut_off,
            })
        })
    }
}

/// Writes its frame count as the copy and finds a cut 1 s into every stretch.
#[derive(Default)]
struct FakeCopy {
    made: Mutex<Vec<RangeUs>>,
}

impl SubjectCopy for FakeCopy {
    fn copy(
        &self,
        _src: &Path,
        range: RangeUs,
        fps: f64,
        out: &Path,
        _cancel: &AtomicBool,
    ) -> Result<Vec<Us>, VideoError> {
        self.made.lock().unwrap().push(range);
        let frames = subject::frames_for(range.end - range.start, fps);
        std::fs::write(out, frames.to_string()).unwrap();
        Ok(vec![range.start + 1_000_000])
    }
}

struct Setup {
    _dir: tempfile::TempDir,
    src: PathBuf,
    cache: Dir,
    work: PathBuf,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("talk.mp4");
    std::fs::write(&src, b"a talk, as bytes").unwrap();
    let cache = Dir::new(&dir.path().join("cache"), &content_key(&src).unwrap()).unwrap();
    let work = dir.path().join("work");
    Setup {
        _dir: dir,
        src,
        cache,
        work,
    }
}

fn range(start_s: f64, end_s: f64) -> RangeUs {
    RangeUs {
        start: (start_s * 1e6) as Us,
        end: (end_s * 1e6) as Us,
    }
}

fn ask<'a>(s: &'a Setup, range: RangeUs, phrase: &'a str) -> Ask<'a> {
    Ask {
        src: &s.src,
        cache: Some(&s.cache),
        work: &s.work,
        range,
        phrase,
        fps: subject::FPS,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn requests_join_only_stretches_a_second_apart_and_split_long_ones() {
    // A second apart or less: one request (at most two copy frames that don't play). Further: never sent together, so
    // footage that was cut out never goes up.
    let asked = [range(0.0, 10.0), range(11.0, 20.0), range(23.0, 30.0)];
    assert_eq!(
        subject::requests(&asked, 2.0, 15_000),
        [range(0.0, 20.0), range(23.0, 30.0)]
    );
    // 100 frames at most a request: 80 s at 2 a second is two.
    assert_eq!(
        subject::requests(&[range(0.0, 80.0)], 2.0, 100),
        [range(0.0, 50.0), range(50.0, 80.0)]
    );
    assert_eq!(subject::frames_for(60_000_000, 2.0), 120);
    assert_eq!(subject::frames_for(250_000, 2.0), 1);
}

#[test]
fn the_price_is_each_requests_frames_at_the_trackers_price() {
    let sam = FakeSam::default();
    // A 60 s short at 2 frames a second: 120 frames.
    assert!((subject::estimate_usd(&sam, &[range(0.0, 60.0)]) - 0.024).abs() < 1e-12);
    // Two stretches 3 s apart, 10 s each: two requests, each billed at least 50 frames.
    let apart = [range(0.0, 10.0), range(13.0, 23.0)];
    assert!((subject::estimate_usd(&sam, &apart) - 0.02).abs() < 1e-12);
}

#[test]
fn what_is_missing_is_the_stretch_less_what_is_covered() {
    let covered = [range(10.0, 20.0), range(25.0, 30.0)];
    assert_eq!(
        subject::missing(range(0.0, 40.0), &covered, 2.0),
        [range(0.0, 10.0), range(20.0, 25.0), range(30.0, 40.0)]
    );
    // Less than a frame left over is covered.
    assert_eq!(subject::missing(range(10.0, 20.3), &covered, 2.0), []);
    assert_eq!(
        subject::missing(range(12.0, 18.0), &[], 2.0),
        [range(12.0, 18.0)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tracked_stretch_is_cached_and_never_paid_for_twice() {
    let s = setup();
    let (sam, copy) = (FakeSam::default(), Arc::new(FakeCopy::default()));
    let first = subject::track(&sam, copy.clone(), &ask(&s, range(30.0, 40.0), "person"))
        .await
        .unwrap();
    assert_eq!(sam.calls.load(Ordering::SeqCst), 1);
    assert!((first.paid_usd - 0.01).abs() < 1e-12);
    // Frame n is at the stretch's start plus n / 2 s, and the cut was found in the file's time.
    let [t] = first.tracks.as_slice() else {
        panic!("{:?}", first.tracks)
    };
    assert_eq!(
        (t.range, t.fps, t.frames.len()),
        (range(30.0, 40.0), 2.0, 20)
    );
    assert_eq!(
        (t.cuts.as_slice(), t.phrase.as_str()),
        (&[31_000_000][..], "person")
    );
    assert_eq!(*copy.made.lock().unwrap(), [range(30.0, 40.0)]);
    // The copy is gone once used.
    assert!(std::fs::read_dir(&s.work).map_or(true, |mut d| d.next().is_none()));

    // The same stretch, and a part of it: from the cache, free.
    for again in [range(30.0, 40.0), range(32.0, 38.0)] {
        let cached = subject::track(&sam, copy.clone(), &ask(&s, again, "person"))
            .await
            .unwrap();
        assert_eq!(cached.paid_usd, 0.0);
        assert_eq!(cached.tracks, first.tracks);
    }
    assert_eq!(sam.calls.load(Ordering::SeqCst), 1);
    let cover = subject::cover(
        &s.cache,
        "fake-sam",
        "person",
        subject::FPS,
        range(31.0, 39.0),
    );
    assert!(cover.missing.is_empty());
    // Footage past the stretch: only what is missing is sent and paid for (5 s, billed as 50 frames).
    let longer = subject::track(&sam, copy.clone(), &ask(&s, range(35.0, 45.0), "person"))
        .await
        .unwrap();
    assert_eq!(copy.made.lock().unwrap().last(), Some(&range(40.0, 45.0)));
    assert!((longer.paid_usd - 0.01).abs() < 1e-12);
    assert_eq!(longer.tracks.len(), 2);
    // Another phrase, or another model: never served another's answers.
    subject::track(
        &sam,
        copy.clone(),
        &ask(&s, range(30.0, 40.0), "man in red"),
    )
    .await
    .unwrap();
    let newer = FakeSam {
        model: Some("sam-4"),
        ..FakeSam::default()
    };
    subject::track(&newer, copy.clone(), &ask(&s, range(30.0, 40.0), "person"))
        .await
        .unwrap();
    assert_eq!(sam.calls.load(Ordering::SeqCst), 3);
    assert_eq!(newer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *sam.phrases.lock().unwrap(),
        ["person", "person", "man in red"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_cut_off_part_way_caches_only_what_it_covers_and_a_retry_pays_for_the_rest() {
    let s = setup();
    let copy = Arc::new(FakeCopy::default());
    let cut = FakeSam {
        cut_off: true,
        ..FakeSam::default()
    };
    let half = subject::track(&cut, copy.clone(), &ask(&s, range(0.0, 60.0), "person"))
        .await
        .unwrap();
    // 120 frames sent and billed, 60 answered: the cache holds the first 30 s.
    assert!((half.paid_usd - 0.024).abs() < 1e-12);
    assert_eq!(half.tracks[0].range, range(0.0, 30.0));
    let cover = subject::cover(
        &s.cache,
        "fake-sam",
        "person",
        subject::FPS,
        range(0.0, 60.0),
    );
    assert_eq!(cover.missing, [range(30.0, 60.0)]);
    // Asked again: only the 30 s still missing goes.
    let whole = FakeSam::default();
    let rest = subject::track(&whole, copy.clone(), &ask(&s, range(0.0, 60.0), "person"))
        .await
        .unwrap();
    assert_eq!(copy.made.lock().unwrap().last(), Some(&range(30.0, 60.0)));
    assert!((rest.paid_usd - 0.012).abs() < 1e-12);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_call_caches_nothing() {
    let s = setup();
    let sam = FakeSam {
        fail: true,
        ..FakeSam::default()
    };
    let err = subject::track(
        &sam,
        Arc::new(FakeCopy::default()),
        &ask(&s, range(0.0, 5.0), "person"),
    )
    .await
    .unwrap_err();
    assert!(err.is_stop(), "{err}");
    let cover = subject::cover(
        &s.cache,
        "fake-sam",
        "person",
        subject::FPS,
        range(0.0, 5.0),
    );
    assert!(cover.tracks.is_empty());
}
