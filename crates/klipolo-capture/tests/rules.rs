use klipolo_capture::disk::{DiskLevel, outlook};
use klipolo_capture::gate::{GateSample, GateState, StartGate};
use klipolo_capture::heal::{HealAction, HealPolicy, SourceKind};
use klipolo_capture::health::{MicRules, VideoProblem, VideoRules};
use klipolo_capture::media::PcmBlock;
use klipolo_capture::pipe::{PipeEvent, VideoStats};

const S: u64 = 1_000_000_000;
const US: i64 = 1_000_000;

fn feed(rules: &mut MicRules, from_s: f64, to_s: f64, level: f32) {
    let mut t = from_s;
    while t < to_s {
        rules.feed(
            &PcmBlock {
                host_ns: 0,
                samples: vec![level; 480],
            },
            (t * 1e6) as i64,
        );
        t += 0.01;
    }
}

#[test]
fn the_gate_opens_once_every_source_sends_real_samples() {
    let mut g = StartGate::new(&["screen".into(), "mic".into()], 0);
    g.observe("screen", GateSample::Frame);
    assert_eq!(
        g.poll(S),
        GateState::Waiting {
            missing: vec!["mic".into()]
        }
    );
    g.observe("mic", GateSample::Sound);
    assert_eq!(g.poll(S), GateState::Open);
}

#[test]
fn a_source_silent_after_three_seconds_asks_and_record_without_it_opens() {
    let mut g = StartGate::new(&["screen".into(), "camera".into()], 0);
    g.observe("screen", GateSample::Frame);
    assert_eq!(
        g.poll(3 * S + 1),
        GateState::Ask {
            missing: vec!["camera".into()]
        }
    );
    g.without("camera");
    assert_eq!(g.poll(3 * S + 2), GateState::Open);
}

#[test]
fn exact_zeros_for_two_seconds_warn_at_once() {
    let mut m = MicRules::new("mic", "MV7");
    feed(&mut m, 0.0, 2.1, 0.0);
    assert_eq!(
        m.check(2_100_000).map(|n| n.code),
        Some("capture.health.micZeros")
    );
    assert_eq!(m.check(2_200_000), None, "once");
}

#[test]
fn near_silence_for_ten_seconds_names_the_device() {
    let mut m = MicRules::new("mic", "LG UltraFine");
    feed(&mut m, 0.0, 10.1, 0.000_1); // −80 dBFS
    let note = m.check(10_100_000).unwrap();
    assert_eq!(note.code, "capture.health.micQuiet");
    assert!(
        note.params
            .iter()
            .any(|(k, v)| *k == "device" && format!("{v:?}").contains("LG UltraFine"))
    );
}

#[test]
fn no_speech_in_a_minute_is_one_soft_notice() {
    let mut m = MicRules::new("mic", "AirPods");
    feed(&mut m, 0.0, 60.1, 0.003); // −50 dBFS: room tone, never speech
    let codes: Vec<_> = std::iter::from_fn(|| m.check(60_100_000))
        .map(|n| n.code)
        .collect();
    assert_eq!(codes, ["capture.health.micNotHeard"]);
}

#[test]
fn speech_then_zeros_warns_again() {
    let mut m = MicRules::new("mic", "MV7");
    feed(&mut m, 0.0, 5.0, 0.3);
    assert_eq!(m.check(5 * US), None);
    // Once the person was heard, exact zeros are a gated mic's pause
    // between phrases until they last 30 s.
    feed(&mut m, 5.0, 7.1, 0.0);
    assert_eq!(m.check(7_100_000), None, "a gated mic's pause");
    feed(&mut m, 7.1, 35.1, 0.0);
    assert_eq!(
        m.check(35_100_000).map(|n| n.code),
        Some("capture.health.micZeros")
    );
}

#[test]
fn three_restarts_in_ten_minutes_then_that_source_pauses() {
    let mut h = HealPolicy::new();
    for minute in [0u64, 2, 4] {
        assert_eq!(
            h.decide(
                "screen",
                SourceKind::Screen,
                &PipeEvent::StoppedBySystem,
                minute * 60 * S
            ),
            HealAction::Restart
        );
    }
    assert!(matches!(
        h.decide(
            "screen",
            SourceKind::Screen,
            &PipeEvent::StoppedBySystem,
            5 * 60 * S
        ),
        HealAction::PauseSource(_)
    ));
    assert_eq!(
        h.decide(
            "screen",
            SourceKind::Screen,
            &PipeEvent::StoppedBySystem,
            12 * 60 * S
        ),
        HealAction::Restart,
        "the window moved on"
    );
}

#[test]
fn stop_sharing_and_a_revoked_permission_stop_and_save() {
    let mut h = HealPolicy::new();
    assert_eq!(
        h.decide("screen", SourceKind::Screen, &PipeEvent::StoppedByUser, 0),
        HealAction::StopAndSave("capture.stop.sharing")
    );
    assert_eq!(
        h.decide(
            "camera",
            SourceKind::Camera,
            &PipeEvent::PermissionRevoked,
            0
        ),
        HealAction::StopAndSave("capture.stop.permission")
    );
}

#[test]
fn a_lost_mic_falls_back_after_three_seconds_unless_it_returns() {
    let mut h = HealPolicy::new();
    assert_eq!(
        h.decide("mic", SourceKind::Mic, &PipeEvent::DeviceLost, 0),
        HealAction::WaitForDevice
    );
    assert_eq!(
        h.device_still_missing("mic", SourceKind::Mic, 0, 2 * S),
        HealAction::Ignore
    );
    assert_eq!(
        h.device_still_missing("mic", SourceKind::Mic, 0, 3 * S),
        HealAction::FallbackMic
    );
    assert_eq!(
        h.decide("mic", SourceKind::Mic, &PipeEvent::DeviceBack, 2 * S),
        HealAction::Restart
    );
}

#[test]
fn a_stall_is_two_seconds_without_any_buffer_while_awake() {
    let mut v = VideoRules::new("screen", 60);
    let mut stats = VideoStats {
        delivered: 60,
        ..VideoStats::default()
    };
    assert_eq!(v.check(&stats, S, true), None);
    assert_eq!(v.check(&stats, 2 * S, true), None);
    assert_eq!(v.check(&stats, 3 * S, true), Some(VideoProblem::Stalled));
    stats.delivered = 61;
    assert_eq!(
        v.check(&stats, 4 * S, false),
        None,
        "a sleeping display isn't a stall"
    );
}

#[test]
fn disk_outlook_warns_at_ten_gb_or_thirty_minutes_and_stops_at_two() {
    let rate = 4_000_000.0; // 32 Mb/s
    assert_eq!(outlook(50_000_000_000, rate).level, DiskLevel::Ok);
    assert_eq!(outlook(9_000_000_000, rate).level, DiskLevel::Warn);
    assert_eq!(outlook(6_000_000_000, 4_000_000.0).minutes_left, 16);
    assert_eq!(
        outlook(12_000_000_000, 8_000_000.0).level,
        DiskLevel::Warn,
        "about 20 min above the floor"
    );
    assert_eq!(outlook(1_900_000_000, rate).level, DiskLevel::Stop);
}
