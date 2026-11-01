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

/// Answers every video with one person on each of its frames, counting the calls and the phrases.
#[derive(Default)]
struct FakeSam {
    calls: AtomicUsize,
    phrases: Mutex<Vec<String>>,
    fail: bool,
}

impl SubjectTracker for FakeSam {
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
            let frames: u32 = std::fs::read_to_string(video).unwrap().parse().unwrap();
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
                frames_billed: u64::from(frames),
                cost_usd: f64::from(frames.max(50)) * 0.0002,
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
fn requests_join_close_stretches_and_split_long_ones() {
    // Two stretches 3 s apart are one request; one 30 s further on is its own.
    let asked = [range(0.0, 10.0), range(13.0, 20.0), range(50.0, 55.0)];
    assert_eq!(
        subject::requests(&asked, 2.0, 15_000),
        [range(0.0, 20.0), range(50.0, 55.0)]
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
    // Two stretches far apart, 10 s each: two requests, each billed at least 50 frames.
    let apart = [range(0.0, 10.0), range(100.0, 110.0)];
    assert!((subject::estimate_usd(&sam, &apart) - 0.02).abs() < 1e-12);
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
    let t = &first.track;
    assert_eq!(
        (t.range, t.fps, t.frames.len()),
        (range(30.0, 40.0), 2.0, 20)
    );
    assert_eq!(t.cuts, [31_000_000]);
    assert_eq!(t.phrase, "person");
    assert_eq!(*copy.made.lock().unwrap(), [range(30.0, 40.0)]);
    // The copy is gone once used.
    assert!(std::fs::read_dir(&s.work).map_or(true, |mut d| d.next().is_none()));

    // The same stretch, and a part of it: from the cache, free.
    for again in [range(30.0, 40.0), range(32.0, 38.0)] {
        let cached = subject::track(&sam, copy.clone(), &ask(&s, again, "person"))
            .await
            .unwrap();
        assert_eq!(cached.paid_usd, 0.0);
        assert_eq!(cached.track, first.track);
    }
    assert_eq!(sam.calls.load(Ordering::SeqCst), 1);
    assert!(subject::cached(&s.cache, "person", subject::FPS, range(31.0, 39.0)).is_some());
    // Another phrase, or footage past the stretch: a call each.
    subject::track(
        &sam,
        copy.clone(),
        &ask(&s, range(30.0, 40.0), "man in red"),
    )
    .await
    .unwrap();
    subject::track(&sam, copy.clone(), &ask(&s, range(35.0, 45.0), "person"))
        .await
        .unwrap();
    assert_eq!(sam.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        *sam.phrases.lock().unwrap(),
        ["person", "man in red", "person"]
    );
    // Both stretches of "person" are kept.
    assert!(subject::cached(&s.cache, "person", subject::FPS, range(36.0, 44.0)).is_some());
    assert!(subject::cached(&s.cache, "person", subject::FPS, range(30.0, 40.0)).is_some());
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
    assert!(subject::cached(&s.cache, "person", subject::FPS, range(0.0, 5.0)).is_none());
}
