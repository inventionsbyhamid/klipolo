//! Who speaks, as a person: the windows where one voice talks alone in a shot whose people don't all fit the crop,
//! the vote over what each window's answer says, the price first, and the cache that never pays twice. With a fake
//! matcher and a fake small copy: nothing here reaches Meta.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use klipolo_analysis::VideoError;
use klipolo_analysis::cache::{Dir, content_key};
use klipolo_analysis::providers::{
    BoxFuture, Heard, ProviderError, ScreenBox, TrackedBox, TrackedFrame, VoiceAnswer, VoiceMatcher,
};
use klipolo_analysis::subject::SubjectCopy;
use klipolo_analysis::subject::path::Talk;
use klipolo_analysis::subject::voices::{
    self, Asking, MAX_WINDOWS, VoiceTurn, WINDOW_US, Window, talk, vote,
};
use klipolo_model::fill_focus::Voice;
use klipolo_model::time::{RangeUs, Us};

const PORTRAIT: f64 = (1080.0 / 1920.0) / (1920.0 / 1080.0);
const PRICE: f64 = 0.003;

fn s(seconds: f64) -> Us {
    (seconds * 1e6).round() as Us
}

fn range(from: f64, to: f64) -> RangeUs {
    RangeUs {
        start: s(from),
        end: s(to),
    }
}

fn voice(part: u32, label: &str) -> Voice {
    Voice {
        part,
        label: label.to_owned(),
    }
}

fn turn(from: f64, to: f64, part: u32, label: &str) -> VoiceTurn {
    VoiceTurn {
        start_us: s(from),
        end_us: s(to),
        voice: voice(part, label),
    }
}

fn person(id: u32, cx: f64, width: f64) -> TrackedBox {
    TrackedBox {
        id,
        left: cx - width / 2.0,
        top: 0.1,
        right: cx + width / 2.0,
        bottom: 1.0,
        confidence: Some(0.9),
    }
}

/// `seconds` of frames at 2 a second, each holding `boxes`.
fn frames(seconds: f64, boxes: &[TrackedBox]) -> Vec<TrackedFrame> {
    (0..(seconds * 2.0).round() as u32)
        .map(|frame| TrackedFrame {
            frame,
            boxes: boxes.to_vec(),
        })
        .collect()
}

fn timed(frames: &[TrackedFrame]) -> Vec<(Us, &TrackedFrame)> {
    (frames.iter())
        .map(|f| ((f64::from(f.frame) / 2.0 * 1e6) as Us, f))
        .collect()
}

/// Two people far apart: tracked as 3 on the right and 7 on the left.
fn two_apart(seconds: f64) -> Vec<TrackedFrame> {
    frames(seconds, &[person(3, 0.75, 0.2), person(7, 0.25, 0.2)])
}

fn windows_of(f: &[TrackedFrame], turns: &[VoiceTurn], crop_w: f64) -> Vec<Window> {
    let seconds = f.len() as f64 / 2.0;
    voices::windows(&timed(f), &[], range(0.0, seconds), turns, crop_w)
}

/// The talk of an interview: A alone 0-5, B's "mm-hm" 5-6, A 6-9, B 9-14, A over B's end 13-16, B 16-20.
fn interview() -> Vec<VoiceTurn> {
    vec![
        turn(0.0, 5.0, 0, "A"),
        turn(5.0, 6.0, 0, "B"),
        turn(6.0, 9.0, 0, "A"),
        turn(9.0, 14.0, 0, "B"),
        turn(13.0, 16.0, 0, "A"),
        turn(16.0, 20.0, 0, "B"),
    ]
}

#[test]
fn windows_are_one_voice_alone_for_two_seconds_where_people_dont_fit() {
    let f = two_apart(20.0);
    let w = windows_of(&f, &interview(), PORTRAIT);
    let of = |v: &Voice| -> Vec<RangeUs> {
        w.iter()
            .filter(|w| &w.voice == v)
            .map(|w| w.range)
            .collect()
    };
    // Longest first, each at most WINDOW_US from the middle of its stretch: B's "mm-hm" and A's talk over B are left out.
    assert_eq!(
        of(&voice(0, "A")),
        [range(1.0, 4.0), range(6.0, 9.0), range(14.0, 16.0)]
    );
    assert_eq!(of(&voice(0, "B")), [range(9.5, 12.5), range(16.5, 19.5)]);
    assert!(w.iter().all(|w| w.range.end - w.range.start <= WINDOW_US));
    // The people, left to right, with their boxes.
    let people = &w[0].people;
    assert_eq!(people.len(), 2);
    assert!((people[0].left - 0.15).abs() < 1e-9 && (people[1].left - 0.65).abs() < 1e-9);
}

#[test]
fn no_window_where_everyone_fits_or_one_person_is_there() {
    let fit = frames(20.0, &[person(1, 0.38, 0.12), person(2, 0.62, 0.12)]);
    assert!(windows_of(&fit, &interview(), 0.5625).is_empty());
    let one = frames(20.0, &[person(1, 0.4, 0.2)]);
    assert!(windows_of(&one, &interview(), PORTRAIT).is_empty());
}

#[test]
fn a_voice_gets_at_most_three_windows() {
    let f = two_apart(40.0);
    let turns: Vec<VoiceTurn> = (0..8)
        .map(|i| {
            let at = f64::from(i) * 5.0;
            turn(at, at + 4.0, 0, if i % 2 == 0 { "A" } else { "B" })
        })
        .collect();
    let w = windows_of(&f, &turns, PORTRAIT);
    for v in [voice(0, "A"), voice(0, "B")] {
        assert_eq!(w.iter().filter(|w| w.voice == v).count(), MAX_WINDOWS);
    }
}

#[test]
fn the_vote_takes_what_most_windows_said() {
    let p = |person| Heard::Person { person };
    assert_eq!(vote(&[p(1), p(1), p(2)]), Some(p(1)));
    assert_eq!(
        vote(&[Heard::Nobody, p(2), Heard::Nobody]),
        Some(Heard::Nobody)
    );
    assert_eq!(vote(&[p(2)]), Some(p(2)));
    // A tie says nothing, and an answer that didn't say doesn't count.
    assert_eq!(vote(&[p(1), p(2)]), None);
    assert_eq!(vote(&[Heard::Unclear, p(2)]), Some(p(2)));
    assert_eq!(vote(&[Heard::Unclear]), None);
    assert_eq!(vote(&[]), None);
}

/// Answers by where the window starts (`answers`: from that second on), counting its calls.
struct FakeSpark {
    answers: Vec<(f64, Heard)>,
    asked: Mutex<Vec<RangeUs>>,
    people: Mutex<Vec<usize>>,
}

impl FakeSpark {
    fn new(answers: Vec<(f64, Heard)>) -> Self {
        Self {
            answers,
            asked: Mutex::new(Vec::new()),
            people: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

impl VoiceMatcher for FakeSpark {
    fn model(&self) -> &str {
        "fake-spark"
    }

    fn estimate_usd(&self, seconds: f64) -> f64 {
        PRICE * seconds / 3.0
    }

    fn who_speaks<'a>(
        &'a self,
        video: &'a Path,
        people: &'a [ScreenBox],
    ) -> BoxFuture<'a, Result<VoiceAnswer, ProviderError>> {
        Box::pin(async move {
            // The fake copy holds its range.
            let text = std::fs::read_to_string(video).unwrap();
            let (a, b) = text.split_once('-').unwrap();
            let window = RangeUs {
                start: a.parse().unwrap(),
                end: b.parse().unwrap(),
            };
            self.asked.lock().unwrap().push(window);
            self.people.lock().unwrap().push(people.len());
            let heard = (self.answers.iter())
                .rev()
                .find(|(from, _)| s(*from) <= window.start)
                .map_or(Heard::Unclear, |(_, h)| *h);
            Ok(VoiceAnswer {
                heard,
                cost_usd: PRICE * (window.end - window.start) as f64 / 3e6,
            })
        })
    }
}

/// Writes the range it copies, with "sound".
#[derive(Default)]
struct FakeCopy {
    sound: Mutex<Vec<RangeUs>>,
}

impl SubjectCopy for FakeCopy {
    fn copy(
        &self,
        _: &Path,
        _: RangeUs,
        _: f64,
        _: &Path,
        _: &AtomicBool,
    ) -> Result<Vec<Us>, VideoError> {
        unreachable!("voices are copied with their sound")
    }

    fn copy_with_sound(
        &self,
        _src: &Path,
        range: RangeUs,
        _fps: f64,
        out: &Path,
        _cancel: &AtomicBool,
    ) -> Result<(), VideoError> {
        self.sound.lock().unwrap().push(range);
        std::fs::write(out, format!("{}-{}", range.start, range.end)).unwrap();
        Ok(())
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
    std::fs::write(&src, b"an interview, as bytes").unwrap();
    let cache = Dir::new(&dir.path().join("cache"), &content_key(&src).unwrap()).unwrap();
    let work = dir.path().join("work");
    Setup {
        _dir: dir,
        src,
        cache,
        work,
    }
}

fn asking(s: &Setup) -> Asking<'_> {
    Asking {
        src: &s.src,
        cache: Some(&s.cache),
        work: &s.work,
        cancel: Arc::new(AtomicBool::new(false)),
        sound: &[],
    }
}

#[tokio::test]
async fn voices_are_matched_priced_first_and_never_paid_for_twice() {
    let setup = setup();
    let f = two_apart(20.0);
    let w = windows_of(&f, &interview(), PORTRAIT);
    // A on the left all along; B on the right.
    let spark = FakeSpark::new(vec![
        (0.0, Heard::Person { person: 1 }),
        (9.0, Heard::Person { person: 2 }),
        (14.0, Heard::Person { person: 1 }),
        (16.0, Heard::Person { person: 2 }),
    ]);
    let copy = Arc::new(FakeCopy::default());
    // Priced first: every window, at most.
    let before = voices::uncached_usd(&spark, Some(&setup.cache), &w);
    assert!(
        (before - (PRICE * 4.0 + PRICE * 2.0 / 3.0)).abs() < 1e-9,
        "{before}"
    );

    let matched = voices::match_voices(&spark, copy.clone(), &asking(&setup), &w).await;
    assert_eq!(
        matched.voices,
        BTreeMap::from([
            (voice(0, "A"), Heard::Person { person: 1 }),
            (voice(0, "B"), Heard::Person { person: 2 }),
        ])
    );
    // A's first two windows agree: its third isn't asked.
    assert_eq!(spark.asked(), 4);
    assert!(
        (matched.paid_usd - PRICE * 4.0).abs() < 1e-9,
        "{}",
        matched.paid_usd
    );
    assert!(spark.people.lock().unwrap().iter().all(|n| *n == 2));
    assert_eq!(copy.sound.lock().unwrap().len(), 4);
    // The copies are gone once sent.
    assert!(std::fs::read_dir(&setup.work).unwrap().next().is_none());

    // Again: all from the cache, at no charge.
    assert_eq!(voices::uncached_usd(&spark, Some(&setup.cache), &w), 0.0);
    let again = voices::match_voices(&spark, copy, &asking(&setup), &w).await;
    assert_eq!(again.voices, matched.voices);
    assert_eq!(again.paid_usd, 0.0);
    assert_eq!(spark.asked(), 4);
}

#[tokio::test]
async fn windows_that_disagree_ask_a_third_and_unclear_answers_are_kept_too() {
    let setup = setup();
    let f = two_apart(20.0);
    let w = windows_of(&f, &interview(), PORTRAIT);
    // A's windows say 1, then unclear, then 1; B's say 2, then nobody.
    let spark = FakeSpark::new(vec![
        (0.0, Heard::Person { person: 1 }),
        (6.0, Heard::Unclear),
        (9.0, Heard::Person { person: 2 }),
        (14.0, Heard::Person { person: 1 }),
        (16.0, Heard::Nobody),
    ]);
    let matched =
        voices::match_voices(&spark, Arc::new(FakeCopy::default()), &asking(&setup), &w).await;
    assert_eq!(matched.voices[&voice(0, "A")], Heard::Person { person: 1 });
    // B's two windows disagree: no one.
    assert!(!matched.voices.contains_key(&voice(0, "B")));
    assert_eq!(spark.asked(), 5);
    // The unclear answer was paid for: it is kept, and nothing is asked again.
    let again =
        voices::match_voices(&spark, Arc::new(FakeCopy::default()), &asking(&setup), &w).await;
    assert_eq!(again.paid_usd, 0.0);
    assert_eq!(spark.asked(), 5);
}

#[tokio::test]
async fn a_long_files_parts_keep_their_own_voices() {
    let setup = setup();
    // Two transcription parts: "A" is the left person in the first and the right one in the second.
    let turns = [
        turn(0.0, 10.0, 0, "A"),
        turn(10.0, 20.0, 0, "B"),
        turn(20.0, 30.0, 1, "A"),
        turn(30.0, 40.0, 1, "B"),
    ];
    let f = two_apart(40.0);
    let w = windows_of(&f, &turns, PORTRAIT);
    let spark = FakeSpark::new(vec![
        (0.0, Heard::Person { person: 1 }),
        (10.0, Heard::Person { person: 2 }),
        (20.0, Heard::Person { person: 2 }),
        (30.0, Heard::Person { person: 1 }),
    ]);
    let matched =
        voices::match_voices(&spark, Arc::new(FakeCopy::default()), &asking(&setup), &w).await;
    assert_eq!(matched.voices[&voice(0, "A")], Heard::Person { person: 1 });
    assert_eq!(matched.voices[&voice(1, "A")], Heard::Person { person: 2 });
    // Who speaks when, in the clip's time (the file's plus 5 s here).
    let said = talk(&turns, &matched.voices, s(5.0));
    let at = |from: f64, to: f64, person: u8| Talk {
        start_us: s(from),
        end_us: s(to),
        person,
    };
    assert_eq!(
        said,
        [
            at(5.0, 15.0, 1),
            at(15.0, 25.0, 2),
            at(25.0, 35.0, 2),
            at(35.0, 45.0, 1)
        ]
    );
}

#[test]
fn a_voice_nobody_on_screen_has_is_no_ones_talk() {
    let turns = [turn(0.0, 10.0, 0, "A"), turn(10.0, 20.0, 0, "B")];
    let heard = BTreeMap::from([
        (voice(0, "A"), Heard::Nobody),
        (voice(0, "B"), Heard::Person { person: 2 }),
    ]);
    assert_eq!(
        talk(&turns, &heard, 0),
        [Talk {
            start_us: s(10.0),
            end_us: s(20.0),
            person: 2
        }]
    );
}

#[test]
fn before_anything_is_tracked_the_price_is_at_most_three_windows_a_voice() {
    let spark = FakeSpark::new(Vec::new());
    // A window needs 2 s alone: A's stretches could make 2 + 1 + 1 of them, B's 2 + 2; at most three each.
    let most = voices::most_usd(&spark, &interview(), &[range(0.0, 20.0)]);
    assert!((most - PRICE * 6.0).abs() < 1e-9, "{most}");
    // Only the footage the clips play: A's 2 + 1, B's "mm-hm" none.
    let most = voices::most_usd(&spark, &interview(), &[range(0.0, 9.0)]);
    assert!((most - PRICE * 3.0).abs() < 1e-9, "{most}");
    assert_eq!(spark.asked(), 0);
}

#[test]
fn the_price_before_tracking_covers_a_stretch_cut_by_shots_and_a_file_in_many_clips() {
    let spark = FakeSpark::new(Vec::new());
    // A talks alone for 20 s across two cuts: three shots, a window in each.
    let turns = [turn(0.0, 20.0, 0, "A")];
    let f = two_apart(20.0);
    let w = voices::windows(
        &timed(&f),
        &[s(7.0), s(14.0)],
        range(0.0, 20.0),
        &turns,
        PORTRAIT,
    );
    assert_eq!(w.len(), 3, "{w:?}");
    let most = voices::most_usd(&spark, &turns, &[range(0.0, 20.0)]);
    let asked: f64 = (w.iter())
        .map(|w| spark.estimate_usd((w.range.end - w.range.start) as f64 / 1e6))
        .sum();
    assert!(asked <= most + 1e-12, "{asked} > {most}");
    // One file cut into many clips is priced once: still at most three windows for its voice.
    let pieces: Vec<RangeUs> = (0..10)
        .map(|i| range(f64::from(i) * 2.0, f64::from(i) * 2.0 + 2.0))
        .collect();
    let most = voices::most_usd(&spark, &turns, &pieces);
    assert!((most - PRICE * 3.0).abs() < 1e-9, "{most}");
}

#[tokio::test]
async fn cached_answers_alone_settle_voices_without_asking() {
    let setup = setup();
    let f = two_apart(20.0);
    let w = windows_of(&f, &interview(), PORTRAIT);
    // Nothing asked yet: nothing settled.
    assert!(voices::cached_voices(Some(&setup.cache), "fake-spark", &w).is_empty());
    let spark = FakeSpark::new(vec![
        (0.0, Heard::Person { person: 1 }),
        (9.0, Heard::Person { person: 2 }),
    ]);
    let matched =
        voices::match_voices(&spark, Arc::new(FakeCopy::default()), &asking(&setup), &w).await;
    assert_eq!(
        voices::cached_voices(Some(&setup.cache), "fake-spark", &w),
        matched.voices
    );
    // Another model's answers aren't this one's.
    assert!(voices::cached_voices(Some(&setup.cache), "other-model", &w).is_empty());
}

#[tokio::test]
async fn an_answer_about_other_people_in_the_same_window_is_asked_again() {
    let setup = setup();
    let f = two_apart(20.0);
    let w = windows_of(&f, &interview(), PORTRAIT);
    let spark = FakeSpark::new(vec![(0.0, Heard::Person { person: 1 })]);
    voices::match_voices(
        &spark,
        Arc::new(FakeCopy::default()),
        &asking(&setup),
        &w[..1],
    )
    .await;
    assert_eq!(spark.asked(), 1);
    // The same stretch, two people again, but standing elsewhere (another crop of the clip keeps others).
    let mut moved = w[0].clone();
    for b in &mut moved.people {
        b.left += 0.1;
        b.right += 0.1;
    }
    voices::match_voices(
        &spark,
        Arc::new(FakeCopy::default()),
        &asking(&setup),
        &[moved],
    )
    .await;
    assert_eq!(spark.asked(), 2);
}
