//! The rules beyond `tests/rules.rs`: the start gate's count from when the pipes started, the heal table's other
//! rows, the drop warning, the rate meter, and a mic that never sends.
use klipolo_capture::disk::{RateMeter, outlook};
use klipolo_capture::gate::{GateSample, GateState, StartGate};
use klipolo_capture::heal::{HealAction, HealPolicy, SourceKind};
use klipolo_capture::health::{MicRules, VideoProblem, VideoRules};
use klipolo_capture::media::PcmBlock;
use klipolo_capture::pipe::{PipeEvent, VideoStats};

const S: u64 = 1_000_000_000;

#[test]
fn the_gate_counts_from_when_the_pipes_started_and_asks_about_a_source_that_never_sent() {
    // The session makes the gate once every pipe has started (here 5 s in); nothing has sent anything yet.
    let all: Vec<String> = vec!["screen".into(), "mic".into(), "gameSound".into()];
    let mut g = StartGate::new(&all, 5 * S);
    assert_eq!(
        g.poll(7 * S),
        GateState::Waiting {
            missing: all.clone()
        }
    );
    assert_eq!(
        g.poll(8 * S),
        GateState::Ask {
            missing: all.clone()
        }
    );
    // A mic's exact zeros aren't real samples; game sound's silence is (silence is normal there).
    let zeros = PcmBlock {
        host_ns: 0,
        samples: vec![0.0; 480],
    };
    assert_eq!(GateSample::from_pcm(&zeros, SourceKind::Mic), None);
    let sound = GateSample::from_pcm(&zeros, SourceKind::Sound).unwrap();
    g.observe("gameSound", sound);
    g.observe("screen", GateSample::Frame);
    assert_eq!(
        g.poll(8 * S),
        GateState::Ask {
            missing: vec!["mic".into()]
        }
    );
}

#[test]
fn an_encoder_rebuilds_three_times_then_lowers_the_preset_once_then_pauses() {
    let mut h = HealPolicy::new();
    let failed = PipeEvent::EncoderFailed { code: -12_903 };
    let actions: Vec<_> = (0..5)
        .map(|i| h.decide("screen", SourceKind::Screen, &failed, i * S))
        .collect();
    assert_eq!(
        actions[..4],
        [
            HealAction::RebuildEncoder,
            HealAction::RebuildEncoder,
            HealAction::RebuildEncoder,
            HealAction::LowerPreset
        ]
    );
    assert!(matches!(&actions[4], HealAction::PauseSource(n) if n.code == "capture.health.lost"));
    // A pipe's own note is given once per take.
    let note = PipeEvent::Note("capture.health.presenterOverlay".into());
    assert!(matches!(
        h.decide("screen", SourceKind::Screen, &note, 6 * S),
        HealAction::Note(n) if n.code == "capture.health.presenterOverlay"
    ));
    assert_eq!(
        h.decide("screen", SourceKind::Screen, &note, 7 * S),
        HealAction::Ignore
    );
}

#[test]
fn a_lost_camera_a_blank_screen_and_a_quit_game_wait_then_note_once() {
    let mut h = HealPolicy::new();
    let camera =
        |h: &mut HealPolicy, e: PipeEvent, at: u64| h.decide("camera", SourceKind::Camera, &e, at);
    assert_eq!(
        camera(&mut h, PipeEvent::DeviceLost, 0),
        HealAction::WaitForDevice
    );
    assert_eq!(
        h.device_still_missing("camera", SourceKind::Camera, 0, 9 * S),
        HealAction::Ignore
    );
    let lost = h.device_still_missing("camera", SourceKind::Camera, 0, 10 * S);
    assert!(matches!(lost, HealAction::Note(n) if n.code == "capture.health.lost"));
    assert_eq!(
        h.device_still_missing("camera", SourceKind::Camera, 0, 11 * S),
        HealAction::Ignore,
        "once"
    );
    assert_eq!(
        camera(&mut h, PipeEvent::DeviceBack, 12 * S),
        HealAction::Restart
    );

    let screen =
        |h: &mut HealPolicy, e: PipeEvent, at: u64| h.decide("screen", SourceKind::Screen, &e, at);
    assert_eq!(
        screen(&mut h, PipeEvent::Blank(true), 20 * S),
        HealAction::Ignore
    );
    assert_eq!(h.still_blank("screen", 23 * S), HealAction::Ignore);
    let blank = h.still_blank("screen", 23 * S + 1);
    assert!(
        matches!(&blank, HealAction::Note(n) if n.params.iter().any(|(k, v)| *k == "reason" && format!("{v:?}").contains("blank")))
    );
    assert_eq!(h.still_blank("screen", 24 * S), HealAction::Ignore, "once");
    assert_eq!(
        screen(&mut h, PipeEvent::TargetGone, 30 * S),
        HealAction::WaitForTarget
    );
    assert_eq!(
        screen(&mut h, PipeEvent::TargetBack, 40 * S),
        HealAction::Restart
    );
}

#[test]
fn more_than_one_percent_dropped_over_ten_seconds_warns_once_and_one_drop_never_does() {
    let mut v = VideoRules::new("screen", 60);
    let mut stats = VideoStats::default();
    for s in 1..=11 {
        stats.delivered += 60;
        stats.dropped_encoder += u64::from(s == 5);
        assert_eq!(v.check(&stats, s * S, true), None, "one glitch at {s} s");
    }
    let mut problems = vec![];
    for s in 12..=30 {
        stats.delivered += 60;
        stats.dropped_pool += 1;
        stats.dropped_channel += 1;
        problems.extend(v.check(&stats, s * S, true));
    }
    let codes: Vec<_> = problems
        .iter()
        .map(|p| match p {
            VideoProblem::Dropping(n) => n.code,
            VideoProblem::Stalled => "stalled",
        })
        .collect();
    assert_eq!(codes, ["capture.health.dropping"]);
}

#[test]
fn the_rate_meter_measures_the_last_thirty_seconds() {
    let mut r = RateMeter::new();
    assert_eq!(r.bytes_per_s(), None);
    // 8 MB/s for 30 s, then 2 MB/s, reported ten times a second.
    for tenth in 0..=600u64 {
        let total = if tenth <= 300 {
            800_000 * tenth
        } else {
            240_000_000 + 200_000 * (tenth - 300)
        };
        r.add(tenth * S / 10, total);
    }
    let rate = r.bytes_per_s().unwrap();
    assert!((rate - 2_000_000.0).abs() < 1.0, "{rate}");
    let low = outlook(9_000_000_000, rate);
    assert_eq!(
        low.note().map(|n| (n.code, n.params.len())),
        Some(("capture.health.diskLow", 1))
    );
}

#[test]
fn a_mic_that_never_sends_a_block_is_still_caught() {
    // Resume (no countdown) and a fallback mic (a new `MicRules`) check a mic that may never call back: an hour of no
    // sound must not pass without a note. The rules' clock starts at the first check.
    let mut m = MicRules::new("mic", "MV7");
    assert_eq!(m.check(0), None);
    assert_eq!(m.check(9_900_000), None);
    let note = m
        .check(10_000_000)
        .expect("a silent mic is noted after 10 s");
    assert_eq!(note.code, "capture.health.micQuiet");
    assert_eq!(m.check(3_600_000_000), None, "once");
}
