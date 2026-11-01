use klipolo_capture::backend::{DeviceInfo, Transport};
use klipolo_capture::hosttime::capture_host_ns;
use klipolo_capture::mic::{MAX_INPUT_LATENCY_NS, i16_to_f32, latency_ns, pick_mic};
use std::time::Duration;

fn dev(id: &str, t: Transport) -> DeviceInfo {
    DeviceInfo {
        id: id.into(),
        name: id.into(),
        transport: t,
        is_virtual: false,
        in_use: false,
        suspended: false,
    }
}

#[test]
fn a_bluetooth_headset_output_picks_the_built_in_mic() {
    let mics = [
        dev("AirPods", Transport::Bluetooth),
        dev("MacBook Microphone", Transport::BuiltIn),
    ];
    let (picked, note) = pick_mic(&mics, Some(&dev("AirPods", Transport::Bluetooth)), None);
    assert_eq!(picked.as_deref(), Some("MacBook Microphone"));
    assert_eq!(
        note.map(|n| n.code),
        Some("capture.preflight.bluetoothMic".into())
    );
}

#[test]
fn a_mic_the_person_chose_is_kept() {
    let mics = [
        dev("AirPods", Transport::Bluetooth),
        dev("MacBook Microphone", Transport::BuiltIn),
    ];
    let (picked, _) = pick_mic(
        &mics,
        Some(&dev("AirPods", Transport::Bluetooth)),
        Some("AirPods"),
    );
    assert_eq!(picked.as_deref(), Some("AirPods"));
}

#[test]
fn a_device_clock_far_from_the_host_falls_back_to_now_minus_latency() {
    let now = 50_000_000_000u64;
    // A capture instant on the host clock: used as is.
    assert_eq!(
        capture_host_ns(
            Duration::from_nanos(now - 8_000_000),
            Duration::from_nanos(now),
            now
        ),
        now - 8_000_000
    );
    // A device clock an hour away: now minus the 8 ms the callback reports.
    assert_eq!(
        capture_host_ns(
            Duration::from_secs(3_600),
            Duration::from_secs(3_600) + Duration::from_millis(8),
            now
        ),
        now - 8_000_000
    );
}

#[test]
fn integer_samples_become_floats() {
    assert_eq!(i16_to_f32(0), 0.0);
    assert!((i16_to_f32(i16::MAX) - 1.0).abs() < 1e-4 && (i16_to_f32(i16::MIN) + 1.0).abs() < 1e-4);
}

/// A mic's input latency from Core Audio's frames: the device's latency, its safety offset and its
/// stream's, over the rate; capped at 0.5 s, and none at a rate that isn't known.
#[test]
fn the_mic_s_input_latency_is_its_frames_over_its_rate_capped() {
    // This Intel Mac's built-in mic reports about 33 ms at 48 kHz.
    assert_eq!(latency_ns(1_000, 200, 400, 48_000.0), 33_333_333);
    assert_eq!(
        latency_ns(48_000, 0, 0, 48_000.0),
        MAX_INPUT_LATENCY_NS,
        "1 s is capped"
    );
    assert_eq!(latency_ns(512, 0, 0, 0.0), 0);
    assert_eq!(latency_ns(512, 0, 0, f64::NAN), 0);
}
