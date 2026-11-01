//! The screen encoder thread's schedule (`platform::common::still`), on every OS: no
//! wake-ups while paused beyond a nudge a second, re-feeds on the picture's own clock, and a quiet stream that answers
//! its nudges never counted as a stall by the session's own stall rule, unless it sends nothing for a whole minute.

use klipolo_capture::health::{ANSWERS_ONLY_NS, STALL_NS, VideoProblem, VideoRules};
use klipolo_capture::pipe::VideoStats;
use klipolo_capture::platform::common::sck::REFEED_NS;
use klipolo_capture::platform::common::still::{
    NUDGE_AFTER_NS, REFEED_MARGIN_NS, Stamp, StillSchedule,
};

const S: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;

#[test]
fn paused_the_thread_wakes_at_most_once_a_second_and_a_resume_refeeds_a_keyframe_at_once() {
    let mut s = StillSchedule::new(true);
    let t0 = 10 * S;
    assert_eq!(s.frame(t0), Stamp { at: t0, key: false });
    s.paused(true, t0 + 100 * MS);
    // A minute paused (a game out of focus, a locked screen), the stream sending nothing: the thread wakes only to
    // nudge it, once a second, and has nothing else to do.
    let (alive, mut now, mut wakes) = (t0, t0 + 100 * MS, 0);
    while now < t0 + 60 * S {
        let next = s.next(now, alive).expect("a nudge to wait for");
        assert!(
            next >= now + 250 * MS,
            "woke {} ms after",
            (next - now) / MS
        );
        now = next;
        wakes += 1;
        let due = s.due(now, alive);
        assert!(
            due.refeed.is_none() && !due.beat,
            "nothing re-fed or sampled while paused"
        );
        assert!(due.nudge);
    }
    assert!(wakes <= 60, "{wakes} wakes in a minute");
    // Resumed: the last frame again at once, at the resume, as a keyframe.
    let resumed = now + 3 * MS;
    s.paused(false, resumed);
    let due = s.due(resumed, alive);
    assert_eq!(
        due.refeed,
        Some(Stamp {
            at: resumed,
            key: true
        })
    );
    assert!(due.beat);
    assert_eq!(
        s.next(resumed, alive),
        Some(resumed + 250 * MS),
        "the cursor's heartbeat"
    );
}

#[test]
fn refeeds_keep_the_picture_s_own_clock() {
    let mut s = StillSchedule::new(false);
    let t = 10 * S;
    s.frame(t);
    // The thread wakes a little late: the re-feed is stamped half a second after the last frame, not now.
    let due = s.due(t + 550 * MS, t);
    assert_eq!(due.refeed.map(|r| r.at), Some(t + REFEED_NS));
    // A real change displayed just after that, delivered after the re-feed, keeps its own time.
    assert_eq!(s.frame(t + 520 * MS).at, t + 520 * MS);
    // A thread late by more than a re-feed stamps it now, once (no burst of catch-ups).
    let late = t + 520 * MS + 1_200 * MS;
    assert_eq!(s.due(late, t).refeed.map(|r| r.at), Some(late));
    assert!(s.due(late, t).refeed.is_none());
    // A frame displayed 10 ms before the next re-feed's stamp and delivered 20 ms after it comes first and keeps its
    // time: the re-feed waits out the delivery latency.
    let stamp = late + REFEED_NS;
    assert_eq!(s.next(stamp, stamp), Some(stamp + REFEED_MARGIN_NS));
    assert!(s.due(stamp + 20 * MS, stamp).refeed.is_none());
    assert_eq!(s.frame(stamp - 10 * MS).at, stamp - 10 * MS);
    let late = stamp - 10 * MS;
    // A short pause: the next re-feed never lands before the resume.
    let last = late;
    s.paused(true, last + 300 * MS);
    s.paused(false, last + 600 * MS);
    let due = s.due(last + 600 * MS, last);
    assert_eq!(
        due.refeed.map(|r| (r.at, r.key)),
        Some((last + 600 * MS, true))
    );
}

/// A screen stream that sent one buffer, then nothing for `seconds`, checked by the stall rule once a second; when
/// `answering`, each nudge is answered 10 ms later (`VideoStats::answered`). The seconds at which it stalled.
fn stalls_of_a_quiet_stream(answering: bool, seconds: u64) -> Vec<u64> {
    let mut s = StillSchedule::new(false);
    let mut rules = VideoRules::new("screen", 60);
    let mut stats = VideoStats {
        delivered: 1,
        ..VideoStats::default()
    };
    s.frame(0);
    let (mut alive, mut answer_at, mut stalls) = (1, None, vec![]);
    for step in 1..=seconds * 100 {
        let now = step * 10 * MS;
        if answer_at.is_some_and(|at| now >= at) {
            (stats.answered, alive, answer_at) = (stats.answered + 1, now, None);
        }
        if s.due(now, alive).nudge && answering {
            answer_at = Some(now + 10 * MS);
        }
        if now.is_multiple_of(S) && rules.check(&stats, now, true) == Some(VideoProblem::Stalled) {
            stalls.push(now / S);
        }
    }
    stalls
}

#[test]
fn a_quiet_stream_that_answers_its_nudges_is_never_a_stall() {
    const {
        assert!(
            NUDGE_AFTER_NS < STALL_NS,
            "a quiet stream is nudged before it would stall"
        )
    };
    assert_eq!(stalls_of_a_quiet_stream(true, 20), Vec::<u64>::new());
    // One that doesn't answer stalls as before, after 2 s of nothing.
    assert_eq!(stalls_of_a_quiet_stream(false, 20).first(), Some(&3));
}

#[test]
fn a_stream_that_only_answers_for_a_minute_is_a_stall() {
    // Answers without a single buffer for a minute: a healthy still stream sends idle buffers,
    // so it is restarted, once a minute while it lasts, and never sooner.
    assert_eq!(ANSWERS_ONLY_NS, 60 * S);
    assert_eq!(stalls_of_a_quiet_stream(true, 130), [61, 121]);
}

#[test]
fn a_camera_s_schedule_never_refeeds() {
    // A camera's file holds only the device's frames: no re-feed however long it is quiet.
    let mut s = StillSchedule::new(false).without_refeeds();
    s.frame(10 * S);
    assert_eq!(s.next(10 * S, 0), None, "nothing to wait for");
    assert!(s.due(20 * S, 0).refeed.is_none());
}
