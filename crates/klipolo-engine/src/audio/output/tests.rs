//! The mixer and the real callback, driven by a simulated device (no audio device is opened). The device plays only
//! the buffers a test tells it to, so what is heard never depends on the machine's load or how long a sleep lasts.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use klipolo_model::project::Loudness;

use super::mixer::Mixer;
use crate::audio::clock::AudioClock;
use crate::audio::mix::RATE;
use crate::audio::pcm::{Pcm, PcmKey, PcmStore};
use crate::audio::plan::{AudioPlan, AudioSource};
use crate::audio::stream::{Callback, Shared};

fn plan(gain: f64) -> Arc<AudioPlan> {
    Arc::new(AudioPlan {
        sources: vec![AudioSource {
            path: "/ramp".into(),
            start_us: 0,
            end_us: 60_000_000,
            source_in_us: 0,
            speed: 1.0,
            rate: 1.0,
            gain,
            fade_in_us: 0,
            fade_out_us: 0,
            crossfade: Default::default(),
            filter: None,
            centre_from: None,
            duck: None,
        }],
        loudness: Loudness::Off,
    })
}

/// What the simulated device is told to do.
enum Pull {
    /// Play this many 512-frame buffers.
    Buffers(usize),
    /// Play one buffer once the mixer asks for flush `n` (a seek or a re-plan, which waits for that buffer).
    AfterFlush(u64),
}

/// A simulated device on its own thread (as a real one calls back on its own) that plays 512-frame buffers only when
/// told. Each test says how much the device plays between the mixer's steps, so an underrun happens exactly where it
/// says, whatever the machine's load. A device paced by sleeps played less under load: a 60 ms underrun then drained
/// too little of the 250 ms queued to be heard.
struct Device {
    pulls: mpsc::Sender<Pull>,
    done: mpsc::Receiver<()>,
    shared: Arc<Shared>,
    played: Arc<Mutex<Vec<f32>>>,
}

impl Device {
    fn new(mut cb: Callback) -> Self {
        let shared = cb.shared.clone();
        let played = Arc::new(Mutex::new(Vec::new()));
        let (pulls, asked) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let (heard, flags) = (played.clone(), shared.clone());
        std::thread::spawn(move || {
            let (mut buf, mut host) = (vec![0f32; 1024], 0u64);
            let mut play = |cb: &mut Callback| {
                cb.fill(&mut buf, host);
                host += 10_666_667;
                heard.lock().unwrap().extend_from_slice(&buf);
            };
            for pull in asked {
                match pull {
                    Pull::Buffers(n) => (0..n).for_each(|_| play(&mut cb)),
                    Pull::AfterFlush(n) => {
                        while flags.flush.load(Ordering::Acquire) < n {
                            std::thread::yield_now();
                        }
                        play(&mut cb);
                    }
                }
                if finished.send(()).is_err() {
                    break;
                }
            }
        });
        Device {
            pulls,
            done,
            shared,
            played,
        }
    }

    /// Plays `n` buffers; returns once they are played.
    fn play(&self, n: usize) {
        self.pulls.send(Pull::Buffers(n)).unwrap();
        self.done.recv().unwrap();
    }

    /// Runs `step`, a seek or a re-plan, while the device plays exactly one buffer once the flush is asked for: the one
    /// that carries the flush out (the step waits for it).
    fn flushing<T>(&self, step: impl FnOnce() -> T) -> T {
        let next = self.shared.flush.load(Ordering::Acquire) + 1;
        self.pulls.send(Pull::AfterFlush(next)).unwrap();
        let out = step();
        self.done.recv().unwrap();
        out
    }

    /// Every stereo sample played so far.
    fn played(&self) -> Vec<f32> {
        self.played.lock().unwrap().clone()
    }
}

#[test]
fn sound_stays_on_the_clock_through_seeks_underruns_and_edits() {
    let store = PcmStore::new(std::env::temp_dir().join("klipolo-pcm-test-unused"));
    let ramp = (0..600_000).flat_map(|i| [i as f32 / 1e6, 0.5]).collect();
    store.insert_ready(
        PcmKey::of(&plan(1.0).sources[0]),
        Arc::new(Pcm::from_samples(ramp)),
    );
    let clock = Arc::new(AudioClock::new(RATE));
    let gain = Arc::new(AtomicU32::new(1f32.to_bits()));
    let (mut mixer, cons, shared) = Mixer::new(store, RATE, clock.clone(), gain);
    let device = Device::new(Callback::new(cons, shared, clock.clone(), 2));
    mixer.set_plan(plan(1.0));
    device
        .flushing(|| mixer.seek(1_000_000, 1, Duration::from_secs(1)))
        .unwrap();
    let (mark, from) = (mixer.mark as i64, 48_000i64);
    // The clock maps the mark to the position asked for.
    assert!(clock.timeline_us(1, u64::MAX / 2).unwrap() > 1_000_000);
    // Playing: the mixer keeps 250 ms queued; the device plays 4 buffers (about 43 ms) between its top-ups.
    for _ in 0..10 {
        mixer.top_up();
        device.play(4);
    }
    // An underrun: the device plays 40 buffers (about 430 ms) while nothing is mixed, past the 250 ms queued.
    device.play(40);
    for _ in 0..10 {
        mixer.top_up();
        device.play(4);
    }
    mixer.set_plan(plan(0.5)); // an edit during playback
    device.flushing(|| mixer.replan()).unwrap();
    for _ in 0..10 {
        mixer.top_up();
        device.play(4);
    }

    // Every frame heard after the seek is silence or exactly the timeline frame the clock says, at some gain
    // (ramps after a re-seat or a gain change): the right channel holds 0.5 × gain.
    let played = device.played();
    let (mut full, mut half, mut silent) = (0, 0, 0);
    for (k, lr) in played.chunks_exact(2).enumerate().skip(mark as usize) {
        let expected = (from + k as i64 - mark) as f32 / 1e6;
        let gain = lr[1] / 0.5;
        if lr == [0.0, 0.0] {
            silent += 1;
            continue;
        }
        assert!(
            gain > 0.0 && gain <= 1.0 + 1e-6 && (lr[0] - expected * gain).abs() < 1e-6,
            "stream frame {k} plays {lr:?}; the clock says {expected}"
        );
        if (gain - 1.0).abs() < 1e-6 {
            full += 1;
        } else if (gain - 0.5).abs() < 1e-6 {
            half += 1;
        }
    }
    assert!(
        full > 1000 && half > 1000 && silent > 1000,
        "{full} {half} {silent}"
    );
}

#[test]
fn resuming_after_a_pause_and_a_seek_plays_nothing_from_before() {
    let store = PcmStore::new(std::env::temp_dir().join("klipolo-pcm-test-unused"));
    let ramp = (0..600_000).flat_map(|i| [i as f32 / 1e6, 0.5]).collect();
    store.insert_ready(
        PcmKey::of(&plan(1.0).sources[0]),
        Arc::new(Pcm::from_samples(ramp)),
    );
    let clock = Arc::new(AudioClock::new(RATE));
    let gain = Arc::new(AtomicU32::new(1f32.to_bits()));
    let (mut mixer, cons, shared) = Mixer::new(store, RATE, clock.clone(), gain);
    let device = Device::new(Callback::new(cons, shared, clock, 2));
    mixer.set_plan(plan(1.0));
    device
        .flushing(|| mixer.seek(1_000_000, 1, Duration::from_secs(1)))
        .unwrap();
    for _ in 0..10 {
        mixer.top_up();
        device.play(1);
    }
    // The stream stops before its callback sees the pause (the worst case): the device plays nothing while the mixer
    // asks for it, and the ring still holds old sound.
    mixer.pause(Duration::from_millis(5));
    let before = device.played().len() / 2;
    // Resume at 5 s.
    device
        .flushing(|| mixer.seek(5_000_000, 2, Duration::from_secs(1)))
        .unwrap();
    let mark = mixer.mark as usize - before;
    for _ in 0..10 {
        mixer.top_up();
        device.play(4);
    }
    let after = device.played().split_off(before * 2);
    let mut heard = 0;
    for (k, lr) in after.chunks_exact(2).enumerate() {
        if lr == [0.0, 0.0] {
            continue;
        }
        let expected = (240_000 + k as i64 - mark as i64) as f32 / 1e6;
        let gain = lr[1] / 0.5;
        assert!(
            k >= mark && (lr[0] - expected * gain).abs() < 1e-6,
            "frame {k} after the resume plays {lr:?} (old sound); expected {expected} × {gain}"
        );
        heard += 1;
    }
    assert!(heard > 1000, "{heard}");
}
