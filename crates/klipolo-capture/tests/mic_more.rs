//! The microphone beyond `mic.rs`: the host-time re-base keeps a device's own spacing, the
//! callback's real-time hand-off (no allocation, lock or I/O on the device's thread), the mic's one or two
//! channels (the CAF header has no `chan` chunk), `pick_mic`'s fallbacks, and a lost mic that comes
//! back.

use std::cell::Cell;
use std::sync::mpsc::{TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use klipolo_capture::backend::{DeviceInfo, Transport};
use klipolo_capture::hosttime::{Rebase, capture_host_ns};
use klipolo_capture::media::{AudioFormat, PcmBlock};
use klipolo_capture::mic::{Flow, handoff, i16_to_f32, i32_to_f32, pick_mic};
use klipolo_capture::pipe::{AudioOut, AudioStats, PipeEvent};

const MS: u64 = 1_000_000;

#[test]
fn a_device_clock_far_from_the_host_is_rebased_once_and_keeps_its_own_spacing() {
    let now = 50_000 * MS;
    let lat = Duration::from_millis(10);
    let dev = |ms: u64| Duration::from_secs(3_600) + Duration::from_millis(ms);
    let mut clock = Rebase::default();
    assert_eq!(
        clock.stamp(dev(0), dev(0) + lat, now),
        now - 10 * MS,
        "anchored at now − latency"
    );
    // However late the callbacks come, the blocks keep the device's 10 ms spacing: no jitter, no false gaps.
    for (block, late_ms) in [(1, 3), (2, 0), (3, 25), (100, 40)] {
        let at = clock.stamp(
            dev(10 * block),
            dev(10 * block) + lat,
            now + (10 * block + late_ms) * MS,
        );
        assert_eq!(at, now - 10 * MS + 10 * block * MS);
    }
    // The device's clock jumps (a reset): the next block is anchored again, never 5 s off.
    let reset = Duration::from_secs(7);
    let later = now + 2_000 * MS;
    assert_eq!(clock.stamp(reset, reset + lat, later), later - 10 * MS);
    // A clock that doesn't advance can't space blocks: each one is now − latency.
    assert_eq!(
        clock.stamp(reset, reset + lat, later + 10 * MS),
        later,
        "stuck"
    );

    // A latency that can't be real (the capture instant garbage, the callback on the host clock) counts as none.
    assert_eq!(
        capture_host_ns(Duration::ZERO, Duration::from_nanos(now), now),
        now
    );
    assert_eq!(
        capture_host_ns(dev(10), dev(0), now),
        now,
        "callback before capture"
    );
    // A device on the host clock is used as it is, also through the re-base.
    let mut host = Rebase::default();
    assert_eq!(
        host.stamp(
            Duration::from_nanos(now - 8 * MS),
            Duration::from_nanos(now),
            now
        ),
        now - 8 * MS
    );
}

fn out(
    blocks: usize,
    events: usize,
) -> (
    AudioOut,
    std::sync::mpsc::Receiver<PcmBlock>,
    std::sync::mpsc::Receiver<PipeEvent>,
) {
    let (b, rb) = sync_channel(blocks);
    let (e, re) = sync_channel(events);
    (
        AudioOut {
            blocks: b,
            events: e,
            stopping: Default::default(),
        },
        rb,
        re,
    )
}

#[test]
fn blocks_cross_the_handoff_whole_on_the_host_clock_in_one_or_two_channels() {
    let (out, rx, _events) = out(8, 4);
    // An eight-input interface: the mic records its first two inputs, as a two-input one would.
    let (mut feed, mut pump, shared) = handoff(48_000, 8);
    assert_eq!(
        pump.format(),
        AudioFormat {
            rate: 48_000,
            channels: 2
        }
    );
    let data: Vec<i16> = (0..480i16)
        .flat_map(|f| [f * 10, -f * 10, 7, 7, 7, 7, 7, 7])
        .collect();
    let at = Duration::from_secs(1);
    assert!(feed.push(
        &data[..],
        i16_to_f32,
        at,
        at + Duration::from_millis(10),
        1_010 * MS
    ));
    assert_eq!(pump.tick(&out), Flow::Open);
    let block = rx.try_recv().unwrap();
    assert_eq!(block.host_ns, 1_000 * MS);
    assert_eq!(block.whole_frames(2), Ok(480));
    assert_eq!(
        &block.samples[2..4],
        &[i16_to_f32(10), i16_to_f32(-10)],
        "frame 1, inputs 1 and 2"
    );
    assert_eq!(
        shared.stats(),
        AudioStats {
            callbacks: 1,
            frames: 480,
            dropped_channel: 0
        }
    );

    // A mono mic stays mono; 32-bit samples convert too.
    let (mut feed, mut pump, _) = handoff(44_100, 1);
    assert_eq!(pump.format().channels, 1);
    assert!(feed.push(&[i32::MIN, 1 << 30][..], i32_to_f32, at, at, 1_000 * MS));
    pump.tick(&out);
    assert_eq!(rx.try_recv().unwrap().samples, [-1.0, 0.5]);
}

#[test]
fn a_full_session_holds_sound_back_and_only_a_full_ring_drops_whole_blocks() {
    let (out, rx, events) = out(1, 1);
    let (mut feed, mut pump, shared) = handoff(48_000, 1);
    // The rings hold 0.5 s: 50 blocks of 10 ms. The pump hasn't run, so the 51st and 52nd are dropped whole.
    let block = vec![0.25f32; 480];
    let push = |feed: &mut klipolo_capture::mic::MicFeed, n: u64| {
        let at = Duration::from_millis(1_000 + 10 * n);
        feed.push(&block[..], |s| s, at, at, (1_000 + 10 * n) * MS)
    };
    let sent: Vec<bool> = (0..52).map(|n| push(&mut feed, n)).collect();
    assert!(sent[..50].iter().all(|&s| s) && !sent[50] && !sent[51]);
    assert_eq!(shared.stats().dropped_channel, 2);

    // The session takes one block a tick (its channel holds one): the rest wait in order, none lost.
    for n in 0..50u64 {
        assert_eq!(pump.tick(&out), Flow::Open);
        let got = rx.try_recv().unwrap();
        assert_eq!(got.host_ns, (1_000 + 10 * n) * MS);
        assert_eq!(got.samples.len(), 480);
    }
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    assert!(push(&mut feed, 52), "room again");

    // The device goes away: one DeviceLost, sent once the event channel has room.
    out.events.try_send(PipeEvent::Stalled).unwrap();
    shared.device_lost();
    shared.device_lost();
    pump.tick(&out);
    assert_eq!(events.try_recv(), Ok(PipeEvent::Stalled));
    pump.tick(&out);
    pump.tick(&out);
    assert_eq!(events.try_recv(), Ok(PipeEvent::DeviceLost));
    assert_eq!(events.try_recv(), Err(TryRecvError::Empty));

    assert_eq!(rx.try_recv().unwrap().host_ns, 1_520 * MS);

    // Stopping sends what the device gave before it stopped; a session that's gone closes the pump.
    assert!(push(&mut feed, 53));
    drop(feed);
    pump.finish(&out);
    assert_eq!(rx.try_recv().unwrap().host_ns, 1_530 * MS);
    drop(rx);
    let (mut feed, mut pump, _) = handoff(48_000, 1);
    assert!(push(&mut feed, 0));
    assert_eq!(pump.tick(&out), Flow::Closed);
}

fn mic(id: &str, t: Transport) -> DeviceInfo {
    DeviceInfo {
        id: format!("uid-{id}"),
        name: id.into(),
        transport: t,
        is_virtual: false,
        in_use: false,
        suspended: false,
    }
}

#[test]
fn a_missing_choice_falls_back_and_a_lone_headset_keeps_its_mic() {
    let output = mic("AirPods Pro", Transport::Bluetooth);
    // Windows names a headset's mic and its output apart; any Bluetooth mic counts while the sound goes to Bluetooth.
    // The mic's id comes back, as `AudioSpec::Mic` carries it, and the note names it.
    let mics = [
        mic("Headset Microphone (AirPods Pro)", Transport::Bluetooth),
        mic("MV7", Transport::Usb),
    ];
    let (picked, note) = pick_mic(&mics, Some(&output), Some("uid-gone"));
    assert_eq!(
        picked.as_deref(),
        Some("uid-MV7"),
        "the chosen mic is unplugged"
    );
    let note = note.unwrap();
    assert_eq!(
        note.params.get("fallback"),
        Some(&klipolo_model::coded::ErrorParam::Text("MV7".into()))
    );
    // A Mac mini with only the headset: its mic is better than none, and there is nothing to say.
    let (picked, note) = pick_mic(&mics[..1], Some(&output), None);
    assert_eq!(
        picked.as_deref(),
        Some("uid-Headset Microphone (AirPods Pro)")
    );
    assert!(note.is_none());
    assert_eq!(pick_mic(&[], Some(&output), None), (None, None));
}

#[test]
fn a_chosen_mic_that_isnt_connected_is_never_replaced_silently() {
    // No headset involved: the default is picked, and preflight says the chosen one is missing.
    let mics = [
        mic("MacBook Pro Microphone", Transport::BuiltIn),
        mic("Yeti", Transport::Usb),
    ];
    let (picked, note) = pick_mic(&mics, None, Some("uid-MV7"));
    assert_eq!(picked.as_deref(), Some("uid-MacBook Pro Microphone"));
    let note = note.expect("a note that the chosen mic is missing");
    assert_eq!(note.code, "capture.preflight.micMissing");
    assert_eq!(
        klipolo_capture::english::english(&note),
        "The microphone you chose isn't connected, so we picked MacBook Pro Microphone."
    );
    // The connected choice, or no choice, says nothing.
    assert_eq!(pick_mic(&mics, None, Some("uid-Yeti")).1, None);
    assert_eq!(pick_mic(&mics, None, None).1, None);
}

#[test]
fn a_lost_mic_is_looked_for_once_a_second_and_its_return_is_sent_once() {
    let (out, _blocks, events) = out(4, 4);
    let (_feed, mut pump, shared) = handoff(48_000, 1);
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let looks = Cell::new(0);
    let counter = &looks;
    let listed = |answer: bool| {
        move || {
            counter.set(counter.get() + 1);
            answer
        }
    };
    // Before any loss there is nothing to look for, and a stream that stopped for another reason isn't looked for.
    assert_eq!(pump.watch(&out, at(0), listed(true)), Flow::Open);
    assert_eq!(looks.get(), 0);

    shared.device_lost();
    pump.tick(&out);
    assert_eq!(events.try_recv(), Ok(PipeEvent::DeviceLost));
    pump.watch(&out, at(0), listed(false));
    pump.watch(&out, at(500), listed(false));
    assert_eq!(looks.get(), 0, "the first look is a second after the loss");
    pump.watch(&out, at(1_000), listed(false));
    pump.watch(&out, at(1_500), listed(false));
    assert_eq!(looks.get(), 1, "once a second");
    assert_eq!(events.try_recv(), Err(TryRecvError::Empty));

    // It's back: DeviceBack, once, even if the event channel is full at first.
    for _ in 0..4 {
        out.events.try_send(PipeEvent::Stalled).unwrap();
    }
    pump.watch(&out, at(2_000), listed(true));
    assert_eq!(looks.get(), 2);
    for _ in 0..4 {
        assert_eq!(events.try_recv(), Ok(PipeEvent::Stalled));
    }
    pump.watch(&out, at(2_100), listed(true));
    pump.watch(&out, at(3_000), listed(true));
    pump.watch(&out, at(9_000), listed(true));
    assert_eq!(looks.get(), 2, "no more looks once it was seen");
    assert_eq!(events.try_recv(), Ok(PipeEvent::DeviceBack));
    assert_eq!(events.try_recv(), Err(TryRecvError::Empty));

    // A stream that stopped (not lost) is restarted by the session, not looked for.
    let (_feed, mut pump, shared) = handoff(48_000, 1);
    shared.stream_stopped();
    pump.tick(&out);
    pump.watch(&out, at(5_000), listed(true));
    pump.watch(&out, at(9_000), listed(true));
    assert_eq!(events.try_recv(), Ok(PipeEvent::StoppedBySystem));
    assert_eq!(events.try_recv(), Err(TryRecvError::Empty));
}
