mod support;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use klipolo_engine::api::ExportQuality;
use klipolo_engine::audio::loudness::{
    Estimate, PREVIEW_QUALITY, estimate_until, gain_db, level_db, measure, target_lufs,
};
use klipolo_engine::audio::mix::{Mixdown, RATE, mix};
use klipolo_engine::audio::pcm::{Pcm, PcmKey, PcmStore};
use klipolo_engine::audio::plan::{
    AudioPlan, AudioSource, Crossfade, audio_plan, may_change_sound,
};
use klipolo_engine::export::audio::ExportAudio;
use klipolo_engine::registry::Registry;
use klipolo_model::contrib::builtin;
use klipolo_model::project::{Loudness, SourceRole};
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, Track, TrackKind};
use support::*;

/// Counts the live heap bytes of each thread, so a test can bound what one build allocates.
struct Counting;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn track(delta: isize) {
    let _ = LIVE.try_with(|live| {
        let now = live.get() + delta;
        live.set(now);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
    });
}

// SAFETY: forwards to the system allocator; the counters are const-initialised thread-locals (no allocation).
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            track(layout.size() as isize);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        track(-(layout.size() as isize));
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, size) };
        if !q.is_null() {
            track(size as isize - layout.size() as isize);
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn source(gain: f64, fade_in_us: i64, speed: f64, source_in_us: i64) -> AudioSource {
    AudioSource {
        path: "/tone".into(),
        start_us: 0,
        end_us: secs(4.0),
        source_in_us,
        speed,
        rate: 1.0,
        gain,
        fade_in_us,
        fade_out_us: 0,
        crossfade: Crossfade::default(),
        filter: None,
        centre_from: None,
    }
}

/// Stereo PCM whose left channel holds the frame index ÷ 1e6 and right a constant 0.5.
fn ramp(frames: usize) -> Arc<Pcm> {
    Arc::new(Pcm::from_samples(
        (0..frames).flat_map(|i| [i as f32 / 1e6, 0.5]).collect(),
    ))
}

fn render(plan: &AudioPlan, pcm: Arc<Pcm>, at_s: f64) -> [f32; 2] {
    let mut out = [0f32; 2];
    mix(
        plan,
        &|_| Some(pcm.clone()),
        (at_s * f64::from(RATE)) as u64,
        1.0,
        &mut out,
    );
    out
}

#[test]
fn audio_plan_follows_the_main_track() {
    let mut p = project();
    p.timeline.main[0].volume = 1.5;
    p.timeline.main[0].fade_out_us = secs(1.0);
    let plan = audio_plan(&p, &Registry::builtin());
    let paths: Vec<&str> = plan.sources.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["/media/mic", "/media/broll"]);
    let mic = &plan.sources[0];
    assert_eq!(
        (
            mic.start_us,
            mic.end_us,
            mic.source_in_us,
            mic.gain,
            mic.fade_out_us
        ),
        (0, secs(20.0), 0, 1.5, secs(1.0))
    );
    assert_eq!(
        (plan.sources[1].start_us, plan.sources[1].end_us),
        (secs(20.0), secs(30.0))
    );
    p.timeline.main[1].muted = true;
    assert_eq!(audio_plan(&p, &Registry::builtin()).sources.len(), 1);
}

fn set_dissolve(clip_id: &str, seconds: f64) -> klipolo_model::ops::Op {
    klipolo_model::ops::Op::SetTransition {
        clip_id: clip_id.into(),
        transition: Some(klipolo_model::timeline::Transition {
            effect: builtin::TRANSITION_DISSOLVE.into(),
            duration_us: secs(seconds),
            params: Default::default(),
        }),
    }
}

#[test]
fn a_transition_crossfades_the_two_clips_sound_over_its_window() {
    // Same project as the plan tests: 1 s transition at the cut at 20 s ("b" plays broll 2–10 s, which has sound).
    let mut p = project();
    p.timeline.main[1].source_in_us = secs(2.0);
    let p = klipolo_model::apply_ops(&p, &[set_dissolve("a", 1.0)]).unwrap();
    let plan = audio_plan(&p, &Registry::builtin());
    let mic = plan
        .sources
        .iter()
        .find(|s| s.path.ends_with("mic"))
        .unwrap();
    assert_eq!((mic.end_us, mic.fade_out_us), (secs(20.5), secs(1.0)));
    let broll = plan
        .sources
        .iter()
        .find(|s| s.path.ends_with("broll"))
        .unwrap();
    assert_eq!(
        (broll.start_us, broll.source_in_us, broll.fade_in_us),
        (secs(19.5), secs(1.5), secs(1.0))
    );
    // Equal power on the crossfading edges only; the clips' other ends keep their fades to silence.
    let only = |fade_in, fade_out| Crossfade { fade_in, fade_out };
    assert_eq!(
        (mic.crossfade, broll.crossfade),
        (only(false, true), only(true, false))
    );
}

#[test]
fn a_crossfade_keeps_the_power_of_two_sounds_level() {
    // Over a crossfade (here 1–3 s) the outgoing gain falls as cos and the incoming one rises as sin, so
    // their squares sum to 1 and unrelated sounds don't dip in the middle. A fade to silence stays linear.
    let pair = |crossfade: bool| {
        let outgoing = AudioSource {
            end_us: secs(3.0),
            fade_out_us: secs(2.0),
            crossfade: Crossfade {
                fade_out: crossfade,
                ..Crossfade::default()
            },
            ..source(1.0, 0, 1.0, 0)
        };
        let incoming = AudioSource {
            start_us: secs(1.0),
            end_us: secs(5.0),
            crossfade: Crossfade {
                fade_in: crossfade,
                ..Crossfade::default()
            },
            ..source(1.0, secs(2.0), 1.0, 0)
        };
        (outgoing, incoming)
    };
    let pcm = ramp(6 * RATE as usize);
    // The right channel is a constant 0.5.
    let gain = |s: &AudioSource, at: f64| {
        let plan = AudioPlan {
            sources: vec![s.clone()],
            loudness: Loudness::Off,
        };
        render(&plan, pcm.clone(), at)[1] / 0.5
    };
    let (outgoing, incoming) = pair(true);
    for at in [1.5, 2.0, 2.5] {
        let angle = ((at - 1.0) / 2.0) as f32 * std::f32::consts::FRAC_PI_2;
        let (a, b) = (gain(&outgoing, at), gain(&incoming, at));
        assert!(
            (a - angle.cos()).abs() < 1e-3 && (b - angle.sin()).abs() < 1e-3,
            "{at} s: {a} {b}"
        );
        assert!(
            (a * a + b * b - 1.0).abs() < 2e-3,
            "{at} s: power {}",
            a * a + b * b
        );
    }
    let (fade, _) = pair(false);
    assert!(
        (gain(&fade, 2.0) - 0.5).abs() < 1e-3,
        "half way down a linear fade"
    );
}

fn audio_track(muted: bool, items: Vec<Item>) -> Track {
    let mut t = Track::new(TrackKind::Audio, "Music");
    t.muted = muted;
    t.items = items;
    t
}

#[test]
fn audio_items_and_video_layers_join_the_mix() {
    let mut p = project();
    let music = Item {
        id: "m".into(),
        start_us: secs(2.0),
        duration_us: secs(10.0),
        attach: None,
        layout: klipolo_model::layout::Layout::new(
            klipolo_model::layout::Anchor::Center,
            bx(0.0, 0.0, 1.0, 1.0),
        ),
        content: ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: secs(5.0),
            volume: 0.5,
            muted: false,
            fade_in_us: secs(1.0),
            fade_out_us: secs(2.0),
            effect: Some(builtin::audio_effect("warm")),
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        effects: Vec::new(),
    };
    let clip = Item {
        id: "v".into(),
        start_us: secs(1.0),
        duration_us: secs(3.0),
        attach: None,
        layout: klipolo_model::layout::Layout::new(
            klipolo_model::layout::Anchor::Center,
            bx(0.5, 0.5, 0.2, 0.2),
        ),
        content: ItemContent::Video {
            asset_id: "broll".into(),
            source_in_us: 0,
            speed: 2.0,
            volume: 0.8,
            muted: false,
            mirror: false,
            tie: None,
        },
        effects: Vec::new(),
    };
    p.timeline.tracks = vec![
        audio_track(false, vec![music]),
        overlay_track("o", vec![clip]),
    ];
    let plan = audio_plan(&p, &Registry::builtin());
    let m = plan
        .sources
        .iter()
        .find(|s| s.path == "/media/music")
        .unwrap();
    assert_eq!(
        (
            m.start_us,
            m.end_us,
            m.source_in_us,
            m.gain,
            m.filter.clone()
        ),
        (
            secs(2.0),
            secs(12.0),
            secs(5.0),
            0.5,
            Some("lowshelf=f=200:g=3,lowpass=f=9000".into())
        ),
        "the effect plays through its filter chain"
    );
    let v = plan
        .sources
        .iter()
        .find(|s| s.path == "/media/broll" && s.start_us == secs(1.0))
        .unwrap();
    assert_eq!((v.speed, v.gain), (2.0, 0.8));
    p.timeline.tracks[0].muted = true;
    p.timeline.tracks[1].hidden = true;
    assert_eq!(
        audio_plan(&p, &Registry::builtin()).sources.len(),
        2,
        "only the main track is left"
    );
}

/// A sound layer's own mute: of two sounds sharing a track, the muted one adds nothing and the
/// other still plays; unmuting gives back the plan it had.
#[test]
fn a_muted_sound_layer_is_silent_beside_one_that_plays() {
    let sound = |id: &str, at: f64| Item {
        id: id.into(),
        start_us: secs(at),
        duration_us: secs(2.0),
        attach: None,
        layout: klipolo_model::layout::Layout::new(
            klipolo_model::layout::Anchor::Center,
            bx(0.0, 0.0, 1.0, 1.0),
        ),
        content: ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: 0,
            volume: 0.8,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        effects: Vec::new(),
    };
    let mut p = project();
    p.timeline.tracks = vec![audio_track(false, vec![sound("a", 1.0), sound("b", 5.0)])];
    let layers = |p: &klipolo_model::project::Project| -> Vec<(i64, f64)> {
        audio_plan(p, &Registry::builtin())
            .sources
            .iter()
            .filter(|s| s.path == "/media/music")
            .map(|s| (s.start_us, s.gain))
            .collect()
    };
    let before = audio_plan(&p, &Registry::builtin());
    assert_eq!(layers(&p), [(secs(1.0), 0.8), (secs(5.0), 0.8)]);
    let mute = |p: &mut klipolo_model::project::Project, on: bool| {
        if let ItemContent::Audio { muted, .. } = &mut p.timeline.tracks[0].items[0].content {
            *muted = on;
        }
    };
    mute(&mut p, true);
    assert_eq!(layers(&p), [(secs(5.0), 0.8)], "only the muted sound goes");
    mute(&mut p, false);
    assert_eq!(
        audio_plan(&p, &Registry::builtin()),
        before,
        "unmuting restores the plan"
    );
}

#[test]
fn effects_and_loudness_targets() {
    assert_eq!(
        Registry::builtin().audio_filter(&builtin::audio_effect("telephone")),
        Some("highpass=f=300,lowpass=f=3400")
    );
    assert_eq!(target_lufs(Loudness::Youtube), Some(-14.0));
    assert_eq!(target_lufs(Loudness::Podcast), Some(-16.0));
    assert_eq!(target_lufs(Loudness::Off), None);
    assert_eq!(gain_db(-20.0, -14.0), 6.0);
    assert_eq!(gain_db(-40.0, -14.0), 12.0, "clamped");
}

#[test]
fn mixer_applies_gain_and_fades() {
    let plan = AudioPlan {
        sources: vec![source(1.0, secs(1.0), 1.0, 0)],
        loudness: Loudness::Off,
    };
    let pcm = ramp(5 * RATE as usize);
    assert!(
        (render(&plan, pcm.clone(), 0.5)[1] - 0.25).abs() < 1e-3,
        "half-way through the fade-in"
    );
    assert!((render(&plan, pcm.clone(), 2.0)[1] - 0.5).abs() < 1e-6);
    assert_eq!(render(&plan, pcm, 4.5), [0.0, 0.0], "after the source ends");
}

#[test]
fn speed_changes_play_live_from_the_right_source_time() {
    // At 2× from source 4 s, timeline 1 s plays source 6 s: the stretch runs in the mixer, so no cached copy per
    // speed exists. The left channel's ramp names the source frame (±10 ms of source; measured 2–5 ms).
    let plan = Arc::new(AudioPlan {
        sources: vec![source(1.0, 0, 2.0, secs(4.0))],
        loudness: Loudness::Off,
    });
    let pcm = ramp(12 * RATE as usize);
    let mut mixdown = Mixdown::new(plan.clone());
    let mut block = vec![0f32; 2 * 4_800];
    let lookup = |_: &AudioSource| Some(pcm.clone());
    for k in 0..20 {
        mixdown.render(&lookup, k * 4_800, &mut block);
    }
    // Streaming from 0: timeline 2 s → source 8 s.
    let at_2s = block[block.len() - 2];
    assert!((at_2s - (8.0 * 48_000.0) / 1e6).abs() < 0.000_48, "{at_2s}");
    // A jump (seek) re-seats with pre-roll: timeline 1 s → source 6 s.
    mixdown.render(&lookup, 48_000, &mut block[..2 * 1_024]);
    let at_1s = block[2 * 1_000];
    assert!(
        (at_1s - (6.0 * 48_000.0 + 2_000.0) / 1e6).abs() < 0.000_48,
        "{at_1s}"
    );
    assert!(
        (block[2 * 1_000 + 1] - 0.5).abs() < 0.01,
        "pitch-kept level"
    );
}

#[test]
fn hard_cuts_are_ramped() {
    // A loud constant source that starts and stops on a cut never jumps straight to full level.
    let plan = AudioPlan {
        sources: vec![AudioSource {
            start_us: secs(1.0),
            ..source(1.0, 0, 1.0, 0)
        }],
        loudness: Loudness::Off,
    };
    let pcm = Arc::new(Pcm::from_samples(vec![0.5; 10 * RATE as usize]));
    let mut out = vec![0f32; 2 * 400];
    mix(&plan, &|_| Some(pcm.clone()), 47_900, 1.0, &mut out);
    let first = out[2 * 101];
    assert!(first > 0.0 && first < 0.01, "{first}");
    assert!((out[2 * 399] - 0.5).abs() < 1e-6);
}

/// RMS (dB) of the left channel over timeline frames `[from, from + frames)`, streamed in blocks from 0.
fn rms_db(plan: AudioPlan, pcm: Arc<Pcm>, from: usize, frames: usize) -> f64 {
    let mut mixdown = Mixdown::new(Arc::new(plan));
    let mut block = vec![0f32; 2 * 1_024];
    let (mut sum, mut count) = (0f64, 0usize);
    let lookup = |_: &AudioSource| Some(pcm.clone());
    for k in 0..(from + frames) / 1_024 {
        mixdown.render(&lookup, (k * 1_024) as u64, &mut block);
        if k * 1_024 >= from {
            sum += block
                .iter()
                .step_by(2)
                .map(|x| f64::from(*x).powi(2))
                .sum::<f64>();
            count += 1_024;
        }
    }
    10.0 * (sum / count as f64).log10()
}

#[test]
fn mono_keeps_its_level_through_speed_changes_and_effects() {
    // A quiet 440 Hz tone: below the Voice compressor's threshold and away from its EQ.
    let tone: Vec<f32> = (0..20 * RATE as usize)
        .map(|n| 0.05 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / RATE as f32).sin())
        .collect();
    let pcm = Arc::new(Pcm::from_channels(tone, 1));
    let voice = Registry::builtin().audio_filter(&builtin::audio_effect("voice"));
    let plan = |speed: f64, filter: Option<&'static str>| AudioPlan {
        sources: vec![AudioSource {
            end_us: secs(10.0),
            speed,
            filter: filter.map(Arc::from),
            ..source(1.0, 0, 1.0, 0)
        }],
        loudness: Loudness::Off,
    };
    let dry = rms_db(plan(1.0, None), pcm.clone(), 48_000, 96_000);
    for (speed, filter) in [(1.5, None), (1.0, voice)] {
        let wet = rms_db(plan(speed, filter), pcm.clone(), 48_000, 96_000);
        assert!(
            (wet - dry).abs() < 0.2,
            "{speed}× {filter:?}: {wet:.2} dB vs {dry:.2} dB"
        );
    }
}

#[test]
fn a_split_with_nothing_removed_plays_through_the_cut() {
    let pcm = Arc::new(Pcm::from_channels(vec![0.5; 10 * RATE as usize], 1));
    for speed in [1.0, 1.5] {
        let half = AudioSource {
            end_us: secs(1.0),
            speed,
            ..source(1.0, 0, 1.0, 0)
        };
        let rest = AudioSource {
            start_us: secs(1.0),
            end_us: secs(2.0),
            source_in_us: secs(speed),
            ..half.clone()
        };
        let mut mixdown = Mixdown::new(Arc::new(AudioPlan {
            sources: vec![half, rest],
            loudness: Loudness::Off,
        }));
        let mut block = vec![0f32; 2 * 1_000];
        let lookup = |_: &AudioSource| Some(pcm.clone());
        let mut lowest = f32::MAX;
        for k in 0..90 {
            mixdown.render(&lookup, k * 1_000, &mut block);
            if (40..60).contains(&k) {
                lowest = block.iter().copied().fold(lowest, f32::min);
            }
        }
        assert!(lowest > 0.49, "{speed}×: dips to {lowest} at the cut");
    }
}

/// A mono 997 Hz sine at half scale, played on both speakers, measures −6.02 LUFS (EBU Tech 3341: a stereo sine at
/// −23 dBFS is −23 LUFS), so YouTube's −14 target turns it down 8 dB.
#[test]
fn loudness_of_the_mix_reaches_the_target() {
    let tone: Vec<f32> = (0..10 * RATE as usize)
        .map(|n| 0.5 * (2.0 * std::f32::consts::PI * 997.0 * n as f32 / RATE as f32).sin())
        .collect();
    let pcm = Arc::new(Pcm::from_channels(tone, 1));
    let plan = AudioPlan {
        sources: vec![AudioSource {
            end_us: secs(10.0),
            ..source(1.0, 0, 1.0, 0)
        }],
        loudness: Loudness::Youtube,
    };
    let lufs = measure(&plan, &|_| Some(pcm.clone())).unwrap();
    assert!((lufs + 6.02).abs() < 0.3, "{lufs}");
    assert!((gain_db(lufs, -14.0) + 7.98).abs() < 0.3);
    assert_eq!(measure(&plan, &|_| None), None, "silence");
}

/// 16-bit stereo WAV: left 0.5 × sin(440 Hz), right 0.25 × sin(220 Hz).
fn write_wav(path: &Path, seconds: usize) {
    let frames = seconds * RATE as usize;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let data = (frames * 4) as u32;
    out.write_all(b"RIFF").unwrap();
    out.write_all(&(36 + data).to_le_bytes()).unwrap();
    out.write_all(b"WAVEfmt ").unwrap();
    out.write_all(&16u32.to_le_bytes()).unwrap();
    out.write_all(&1u16.to_le_bytes()).unwrap();
    out.write_all(&2u16.to_le_bytes()).unwrap();
    out.write_all(&RATE.to_le_bytes()).unwrap();
    out.write_all(&(RATE * 4).to_le_bytes()).unwrap();
    out.write_all(&4u16.to_le_bytes()).unwrap();
    out.write_all(&16u16.to_le_bytes()).unwrap();
    out.write_all(b"data").unwrap();
    out.write_all(&data.to_le_bytes()).unwrap();
    for (l, r) in (0..frames).map(expected) {
        out.write_all(&((l * 32767.0).round() as i16).to_le_bytes())
            .unwrap();
        out.write_all(&((r * 32767.0).round() as i16).to_le_bytes())
            .unwrap();
    }
}

fn expected(n: usize) -> (f32, f32) {
    let t = n as f64 / f64::from(RATE);
    (
        (0.5 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as f32,
        (0.25 * (2.0 * std::f64::consts::PI * 220.0 * t).sin()) as f32,
    )
}

fn assert_matches(pcm: &Pcm, frames: &[usize]) {
    let mut lr = [0f32; 2];
    for &n in frames {
        pcm.read(n as i64, &mut lr);
        let (l, r) = expected(n);
        assert!(
            (lr[0] - l).abs() < 0.003 && (lr[1] - r).abs() < 0.003,
            "frame {n}: {lr:?} != ({l}, {r})"
        );
    }
}

#[test]
fn pcm_cache_streams_a_long_file_in_bounded_memory() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("long.wav");
    write_wav(&wav, 120);
    let store = PcmStore::new(dir.path().join("cache"));
    let key = PcmKey {
        path: wav.display().to_string(),
    };
    let start = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(start));
    let pcm = store.build(&key).unwrap();
    let peak = PEAK.with(Cell::get) - start;
    // Decoding the whole file into one Vec would hold 46 MB for these 2 minutes, and 1.38 GB for an hour.
    assert!(peak < 2 << 20, "the build held {peak} bytes");
    assert_eq!((pcm.channels(), pcm.is_complete()), (2, true));
    assert_eq!(pcm.frames(), 120 * RATE as usize);
    assert_matches(&pcm, &[0, 1, 47_999, 3_000_000, 5_759_999]);
    assert!(pcm.loudness().is_some_and(|l| (-20.0..-5.0).contains(&l)));
    let cached: Vec<String> = std::fs::read_dir(dir.path().join("cache"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        cached.len(),
        3,
        "one .pcm, its .peaks and .energy: {cached:?}"
    );
    // 10 windows a second, 4 bytes a channel: 288,000 bytes for an hour of stereo.
    let energy = cached.iter().find(|n| n.ends_with(".energy")).unwrap();
    let len = std::fs::metadata(dir.path().join("cache").join(energy))
        .unwrap()
        .len();
    assert_eq!(len, 32 + 120 * 10 * 2 * 4);
    // 1.5 KB of level-0 peaks per second of stereo, plus a third more for the coarser levels.
    let peaks = cached.iter().find(|n| n.ends_with(".peaks")).unwrap();
    let len = std::fs::metadata(dir.path().join("cache").join(peaks))
        .unwrap()
        .len();
    assert!((230_000..250_000).contains(&len), "{len}");

    // From the playhead first (70 s in), then the start: both halves line up at the join.
    let other = PcmStore::new(dir.path().join("cache2"));
    let plan = AudioPlan {
        sources: vec![AudioSource {
            path: key.path.clone(),
            end_us: secs(120.0),
            ..source(1.0, 0, 1.0, 0)
        }],
        loudness: Loudness::Off,
    };
    other.prepare(&plan, secs(70.0));
    let pcm = other.wait_complete(&key, &AtomicBool::new(false)).unwrap();
    let join = (70 * RATE as usize) / 49_152 * 49_152;
    assert_matches(&pcm, &[10, join - 1, join, join + 1, 5_000_000]);
}

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn pcm_cache_keeps_mono_mono() {
    let dir = tempfile::tempdir().unwrap();
    let store = PcmStore::new(dir.path().into());
    let mut s = source(1.0, 0, 1.0, 0);
    s.path = support::fixture("tone-440.m4a").display().to_string();
    let pcm = store.build(&PcmKey::of(&s)).unwrap();
    assert_eq!(pcm.channels(), 1);
    assert!(
        (pcm.frames() as i64 - 4 * 48_000).abs() < 2048,
        "{}",
        pcm.frames()
    );
}

/// Peak footprint (dirty + compressed memory, what macOS charges the process) since it started.
#[cfg(target_os = "macos")]
fn peak_footprint() -> u64 {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: our own pid and a correctly sized buffer for RUSAGE_INFO_V4.
    let ok = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as i32,
            libc::RUSAGE_INFO_V4,
            (&raw mut info).cast(),
        )
    };
    assert_eq!(ok, 0);
    info.ri_lifetime_max_phys_footprint
}

/// The cache build for an hour of 48 kHz stereo AAC (`KLIPOLO_LONG_AUDIO`, e.g. made with ffmpeg's anoisesrc + aac):
/// peak footprint, Rust heap, wall time, time until a seek target 45 minutes in plays, and the cache's size.
/// `KLIPOLO_LONG_AUDIO=… cargo test -p klipolo-engine --test audio one_hour -- --ignored --nocapture`
#[cfg(target_os = "macos")]
#[test]
#[ignore = "needs a 1-hour audio file in KLIPOLO_LONG_AUDIO"]
fn one_hour_source_builds_in_bounded_memory() {
    let path = std::env::var("KLIPOLO_LONG_AUDIO").expect("KLIPOLO_LONG_AUDIO");
    let dir = tempfile::tempdir().unwrap();
    let key = PcmKey { path: path.clone() };
    let before = peak_footprint();
    let start = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(start));
    let started = std::time::Instant::now();
    let pcm = PcmStore::new(dir.path().join("a")).build(&key).unwrap();
    let wall = started.elapsed();
    let heap = PEAK.with(Cell::get) - start;
    let after = peak_footprint();
    let bytes: u64 = std::fs::read_dir(dir.path().join("a"))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    println!(
        "1 h build: {:.1} s, {} frames, peak footprint {:.1} MB → {:.1} MB, Rust heap peak {:.2} MB, cache {:.0} MB, {:?} LUFS",
        wall.as_secs_f64(),
        pcm.frames(),
        before as f64 / 1e6,
        after as f64 / 1e6,
        heap as f64 / 1e6,
        bytes as f64 / 1e6,
        pcm.loudness(),
    );
    drop(pcm);
    // From the playhead: Play at 45:00 has sound long before the whole hour is decoded.
    let store = PcmStore::new(dir.path().join("b"));
    let plan = AudioPlan {
        sources: vec![AudioSource {
            path,
            end_us: secs(3_600.0),
            ..source(1.0, 0, 1.0, 0)
        }],
        loudness: Loudness::Off,
    };
    let at = 45 * 60 * u64::from(RATE);
    let started = std::time::Instant::now();
    store.prepare(&plan, secs(45.0 * 60.0));
    let first = loop {
        if store
            .ready(&key)
            .is_some_and(|p| p.is_ready(at + u64::from(RATE)))
        {
            break started.elapsed();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let pcm = store.wait_complete(&key, &AtomicBool::new(false)).unwrap();
    println!(
        "Play at 45:00: sound after {:.0} ms; whole hour after {:.1} s; peak footprint now {:.1} MB",
        first.as_secs_f64() * 1e3,
        started.elapsed().as_secs_f64(),
        peak_footprint() as f64 / 1e6
    );
    assert!(pcm.is_complete());
}

/// Mixer cost per 1024-frame block (21.3 ms of sound): 8 straight sources, then 8 with 2 of them stretched live, then 8
/// recorded at a drifting device's rate (each read resampled).
/// `cargo test -p klipolo-engine --test audio mixer_block_cost -- --ignored --nocapture` (a debug build: an upper bound)
#[test]
#[ignore = "timing"]
fn mixer_block_cost() {
    let mono = Arc::new(Pcm::from_channels(vec![0.1; 120 * RATE as usize], 1));
    // Two channels, read as recorded or centred from the first.
    let stereo = Arc::new(Pcm::from_channels(vec![0.1; 2 * 120 * RATE as usize], 2));
    for (file, stretched, drifting, centred) in [
        ("mono", 0, 0, false),
        ("mono", 2, 0, false),
        ("mono", 0, 8, false),
        ("stereo", 0, 0, false),
        ("stereo", 0, 0, true),
        ("stereo", 0, 8, true),
    ] {
        let pcm = if file == "mono" { &mono } else { &stereo };
        let lookup = |_: &AudioSource| Some(pcm.clone());
        let sources = (0..8)
            .map(|k| AudioSource {
                start_us: 0,
                end_us: secs(100.0),
                speed: if k < stretched { 1.5 } else { 1.0 },
                rate: if k < drifting { 1.0001 } else { 1.0 },
                centre_from: centred.then_some(0),
                ..source(0.5, 0, 1.0, 0)
            })
            .collect();
        let mut mixdown = Mixdown::new(Arc::new(AudioPlan {
            sources,
            loudness: Loudness::Off,
        }));
        let mut block = vec![0f32; 2 * 1_024];
        let blocks = 2_000u64;
        let started = std::time::Instant::now();
        for k in 0..blocks {
            mixdown.render(&lookup, k * 1_024, &mut block);
        }
        let per_block = started.elapsed().as_secs_f64() * 1e6 / blocks as f64;
        println!(
            "8 {file} sources, {stretched} stretched, {drifting} drifting, centred {centred}: {per_block:.0} µs per block \
             ({:.2}% of one core)",
            per_block / 21_333.0 * 100.0
        );
    }
}

/// While any track is soloed only soloed tracks are heard, beside the main track, which always plays; mute
/// wins over solo; un-soloing gives back exactly the plan from before. Export plans its sound with `audio_plan` too.
#[test]
fn solo_picks_the_sound_and_mute_still_wins() {
    let layer = |id: &str, content| Item {
        id: id.into(),
        start_us: secs(1.0),
        duration_us: secs(3.0),
        attach: None,
        layout: klipolo_model::layout::Layout::new(
            klipolo_model::layout::Anchor::Center,
            bx(0.0, 0.0, 1.0, 1.0),
        ),
        content,
        effects: Vec::new(),
    };
    let sound = |asset: &str| ItemContent::Audio {
        asset_id: asset.into(),
        source_in_us: 0,
        volume: 1.0,
        muted: false,
        fade_in_us: 0,
        fade_out_us: 0,
        effect: None,
        drift_ppm: 0.0,
        centre_from: None,
        tie: None,
        duck: None,
    };
    let game = ItemContent::Video {
        asset_id: "broll".into(),
        source_in_us: 0,
        speed: 1.0,
        volume: 1.0,
        muted: false,
        mirror: false,
        tie: None,
    };
    let mut p = project();
    p.timeline.tracks = vec![
        audio_track(false, vec![layer("song", sound("music"))]),
        audio_track(false, vec![layer("voice", sound("mic"))]),
        overlay_track("o", vec![layer("game", game)]),
    ];
    let heard = |p: &klipolo_model::project::Project| {
        let plan = audio_plan(p, &Registry::builtin());
        // The main track's two sources (clip a's mic, clip b's own sound) start at 0 and 20 s; the layers at 1 s.
        assert_eq!(
            plan.sources
                .iter()
                .filter(|s| s.start_us != secs(1.0))
                .count(),
            2
        );
        let mut layers: Vec<String> = plan
            .sources
            .iter()
            .filter(|s| s.start_us == secs(1.0))
            .map(|s| s.path.clone())
            .collect();
        layers.sort();
        layers
    };
    let before = audio_plan(&p, &Registry::builtin());
    assert_eq!(heard(&p), ["/media/broll", "/media/mic", "/media/music"]);

    let mut soloed = p.clone();
    soloed.timeline.tracks[0].solo = true;
    assert_eq!(
        heard(&soloed),
        ["/media/music"],
        "only the soloed track, beside main"
    );
    soloed.timeline.tracks[2].solo = true;
    assert_eq!(
        heard(&soloed),
        ["/media/broll", "/media/music"],
        "a soloed overlay's video sound"
    );
    soloed.timeline.tracks[0].muted = true;
    assert_eq!(heard(&soloed), ["/media/broll"], "mute wins over solo");

    soloed.timeline.tracks[0].muted = false;
    soloed.timeline.tracks[0].solo = false;
    soloed.timeline.tracks[2].solo = false;
    assert_eq!(soloed, p);
    assert_eq!(
        audio_plan(&soloed, &Registry::builtin()),
        before,
        "un-solo restores the plan exactly"
    );
}

/// A WAV of `seconds` at 48 kHz, 16-bit, `channels` (1 or 2), sample `n` of channel `c` from `f(n, c)`.
fn write_signal(path: &Path, channels: u16, seconds: usize, f: impl Fn(usize, usize) -> f32) {
    let frames = seconds * RATE as usize;
    let data = (frames * 2 * usize::from(channels)) as u32;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for part in [
        &b"RIFF"[..],
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &channels.to_le_bytes(),
        &RATE.to_le_bytes(),
        &(RATE * 2 * u32::from(channels)).to_le_bytes(),
        &(2 * channels).to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ] {
        out.write_all(part).unwrap();
    }
    for n in 0..frames {
        for c in 0..usize::from(channels) {
            let x = (f(n, c).clamp(-1.0, 1.0) * 32767.0).round() as i16;
            out.write_all(&x.to_le_bytes()).unwrap();
        }
    }
}

/// Deterministic white noise in [-1, 1) for sample `n` of a stream `seed`.
fn noise(seed: u64, n: usize) -> f32 {
    let mut x = (n as u64 ^ seed.rotate_left(17)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// The preview levels from an estimate of the mix's loudness, export from its exact
/// measurement, and both make the same gain decision within 0.5 dB. The project has what moves the
/// estimate: a recording's mic (speech-like bursts and pauses) and system sound, fades, an equal-power crossfade, a
/// split that plays through its cut, a music bed with the "voice" effect (rendered alone through it), and an overlay
/// video's sound at 1.5×.
#[test]
fn the_preview_and_export_make_the_same_gain_decision() {
    let dir = tempfile::tempdir().unwrap();
    let file = |name: &str| dir.path().join(name).display().to_string();
    let rate = RATE as f32;
    // Syllables at 4 Hz in 1.6 s phrases with 0.4 s pauses.
    write_signal(Path::new(&file("mic.wav")), 1, 40, |n, _| {
        let t = n as f32 / rate;
        let speaking = (t * 0.5).fract() < 0.8;
        let syllable = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * 4.0 * t).sin();
        if speaking {
            0.35 * syllable * noise(1, n)
        } else {
            0.0
        }
    });
    write_signal(Path::new(&file("system.wav")), 2, 40, |n, c| {
        let t = n as f32 / rate;
        0.12 * (1.0 + 0.5 * (2.0 * std::f32::consts::PI * 0.3 * t).sin()) * noise(2 + c as u64, n)
    });
    write_signal(Path::new(&file("music.wav")), 2, 40, |n, c| {
        0.25 * noise(4 + c as u64, n) + 0.1 * noise(6, n)
    });
    write_signal(Path::new(&file("broll.wav")), 2, 40, |n, c| {
        0.2 * noise(8 + c as u64, n)
    });

    let mut p = project();
    lengthen(&mut p, 40.0);
    for asset in &mut p.assets {
        match asset.id.as_str() {
            "mic" | "music" | "broll" => asset.path = file(&format!("{}.wav", asset.id)),
            _ => {}
        }
    }
    let mut system = p.assets[2].clone();
    (system.id, system.path) = ("system".into(), file("system.wav"));
    p.assets.push(system);
    p.recordings[0]
        .sources
        .push(rec_source("system", SourceRole::SystemAudio, 0, 40.0));
    let clip = |id: &str, source: ClipSource, from: f64, to: f64| {
        let mut c = Clip::new(source, secs(from), secs(to));
        c.id = id.into();
        c
    };
    let rec = || ClipSource::Recording { id: "rec".into() };
    let mut a = clip("a", rec(), 0.0, 12.0);
    a.fade_in_us = secs(0.5);
    let mut b = clip("b", rec(), 15.0, 27.0);
    b.volume = 0.8;
    let mut c = clip("c", rec(), 27.0, 31.0);
    c.volume = 0.8;
    let mut d = clip("d", ClipSource::Asset { id: "broll".into() }, 0.0, 6.0);
    d.volume = 0.7;
    p.timeline.main = vec![a, b, c, d];
    let music = Item {
        id: "m".into(),
        start_us: secs(5.0),
        duration_us: secs(25.0),
        attach: None,
        layout: klipolo_model::layout::Layout::new(
            klipolo_model::layout::Anchor::Center,
            bx(0.0, 0.0, 1.0, 1.0),
        ),
        content: ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: secs(2.0),
            volume: 0.3,
            muted: false,
            fade_in_us: secs(1.0),
            fade_out_us: secs(3.0),
            effect: Some(builtin::audio_effect("voice")),
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        effects: Vec::new(),
    };
    let video = item(
        "v",
        8.0,
        6.0,
        bx(0.5, 0.5, 0.3, 0.3),
        ItemContent::Video {
            asset_id: "broll".into(),
            source_in_us: secs(10.0),
            speed: 1.5,
            volume: 0.7,
            muted: false,
            mirror: false,
            tie: None,
        },
    );
    p.timeline.tracks = vec![
        audio_track(false, vec![music]),
        overlay_track("o", vec![video]),
    ];
    p.audio.loudness = Loudness::Youtube;
    let p = klipolo_model::apply_ops(&p, &[set_dissolve("a", 1.0)]).unwrap();

    // Export: decodes every file, then measures the rendered mix exactly.
    let registry = Registry::builtin();
    let cache = dir.path().join("cache");
    let (_, export) =
        ExportAudio::prepare(&p, &registry, ExportQuality::High, Some(&cache), None).unwrap();
    // The preview: the same files' energies (written by export's decode), the effect's source rendered alone.
    let plan = audio_plan(&p, &registry);
    assert!(
        plan.sources
            .iter()
            .any(|s| s.filter.is_some() && s.speed == 1.0)
    );
    assert!(plan.sources.iter().any(|s| s.crossfade.fade_out));
    let store = PcmStore::new(cache);
    let pcm = |s: &AudioSource| store.build(&PcmKey::of(s)).ok();
    let files: Vec<_> = plan
        .sources
        .iter()
        .map(|s| {
            let key = PcmKey::of(s);
            let energies = store.energies(&key, &store.build(&key).unwrap());
            (s.path.clone(), energies.unwrap())
        })
        .collect();
    let energies = |s: &AudioSource| files.iter().find(|f| f.0 == s.path).map(|f| &f.1);
    let effected = |s: &AudioSource, p: &Arc<Pcm>, c: &AtomicBool| store.effected(s, p, c);
    let started = std::time::Instant::now();
    let Estimate::Lufs(Some(estimate)) =
        estimate_until(&plan, &pcm, &energies, &effected, &AtomicBool::new(false))
    else {
        panic!("no estimate");
    };
    let took = started.elapsed();
    let exact = export.lufs.unwrap();
    println!("estimate {estimate:.3} LUFS, exact {exact:.3} LUFS, estimated in {took:?}");
    assert!((estimate - exact).abs() < 0.5, "{estimate} vs {exact}");
    // The same decision: toward −14 LUFS for a new project's target in both.
    let preview = level_db(Some(estimate), p.audio.loudness, PREVIEW_QUALITY);
    assert!(
        (preview - export.gain_db).abs() < 0.5,
        "{preview} vs {}",
        export.gain_db
    );
    assert!(
        export.gain_db.abs() > 1.0,
        "the mix is levelled: {}",
        export.gain_db
    );
    // Off is untouched everywhere and Master never levels, whatever was measured.
    assert_eq!(
        level_db(Some(estimate), Loudness::Off, PREVIEW_QUALITY),
        0.0
    );
    assert_eq!(
        level_db(Some(exact), Loudness::Youtube, ExportQuality::Master),
        0.0
    );
    // A file without energies: the preview measures exactly instead.
    assert_eq!(
        estimate_until(
            &plan,
            &pcm,
            &|_: &AudioSource| None,
            &effected,
            &AtomicBool::new(false)
        ),
        Estimate::Missing
    );
}

/// The estimate's worst real case, dissolves over continuous footage. A recording split with
/// nothing removed and a dissolve at the cut plays the same samples on both sides of the crossfade, so the mix there
/// is louder than the sum of the two powers (up to +3 dB at the middle). The preview must still make export's gain
/// decision within 0.5 dB. Summing powers alone was +0.27, +0.50 and +0.98 dB off at these three densities; with the
/// estimate's cross term for such sources (`add_same_sound`) all three are within 0.01 dB.
#[test]
fn dissolves_over_continuous_footage_keep_the_preview_within_half_a_db() {
    let dir = tempfile::tempdir().unwrap();
    let file = |name: &str| dir.path().join(name).display().to_string();
    let rate = RATE as f32;
    write_signal(Path::new(&file("mic.wav")), 1, 80, |n, _| {
        let t = n as f32 / rate;
        let syllable = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * 4.0 * t).sin();
        if (t * 0.5).fract() < 0.8 {
            0.35 * syllable * noise(21, n)
        } else {
            0.0
        }
    });
    write_signal(Path::new(&file("system.wav")), 2, 80, |n, c| {
        0.1 * noise(22 + c as u64, n)
    });
    let store = PcmStore::new(dir.path().join("cache"));
    let pcm = |s: &AudioSource| store.build(&PcmKey::of(s)).ok();
    let kept: Vec<(String, klipolo_engine::audio::pcm::Energies)> = ["mic.wav", "system.wav"]
        .iter()
        .map(|name| {
            let key = PcmKey { path: file(name) };
            let decoded = store.build(&key).unwrap();
            (file(name), store.energies(&key, &decoded).unwrap())
        })
        .collect();
    let energies = |s: &AudioSource| kept.iter().find(|k| k.0 == s.path).map(|k| &k.1);
    let effected = |s: &AudioSource, p: &Arc<Pcm>, c: &AtomicBool| store.effected(s, p, c);
    // A 70 s recording cut into contiguous clips, with a 1 s dissolve at every `every`-th cut.
    let gap = |clip_secs: f64, every: usize| {
        let mut p = project();
        for asset in &mut p.assets {
            asset.duration_us = secs(80.0);
            if asset.id == "mic" {
                asset.path = file("mic.wav");
            }
        }
        let mut system = p.assets[2].clone();
        (system.id, system.path) = ("system".into(), file("system.wav"));
        p.assets.push(system);
        p.recordings[0]
            .sources
            .push(rec_source("system", SourceRole::SystemAudio, 0, 80.0));
        lengthen(&mut p, 80.0);
        let count = (70.0 / clip_secs) as usize;
        p.timeline.main = (0..count)
            .map(|k| {
                let mut c = Clip::new(
                    ClipSource::Recording { id: "rec".into() },
                    secs(5.0 + k as f64 * clip_secs),
                    secs(5.0 + (k + 1) as f64 * clip_secs),
                );
                c.id = format!("c{k}");
                c
            })
            .collect();
        p.audio.loudness = Loudness::Youtube;
        let dissolves: Vec<_> = (0..count - 1)
            .filter(|k| k % every == 0)
            .map(|k| set_dissolve(&format!("c{k}"), 1.0))
            .collect();
        let p = klipolo_model::apply_ops(&p, &dissolves).unwrap();
        let plan = audio_plan(&p, &Registry::builtin());
        let Estimate::Lufs(Some(estimate)) =
            estimate_until(&plan, &pcm, &energies, &effected, &AtomicBool::new(false))
        else {
            panic!("no estimate");
        };
        let exact = measure(&plan, &pcm).unwrap();
        let share = dissolves.len() as f64 / (count as f64 * clip_secs);
        let decision = level_db(Some(estimate), Loudness::Youtube, PREVIEW_QUALITY)
            - level_db(Some(exact), Loudness::Youtube, PREVIEW_QUALITY);
        println!(
            "{:.0} % of the time in a dissolve ({} of {count} cuts): estimate {estimate:.3}, exact {exact:.3} LUFS, \
             gain decisions {decision:+.3} dB apart",
            100.0 * share,
            dissolves.len()
        );
        decision
    };
    for (clip_secs, every) in [(4.8, 2), (4.8, 1), (2.4, 1)] {
        let decision = gap(clip_secs, every);
        assert!(decision.abs() < 0.5, "{clip_secs} s clips: {decision}");
    }
}

/// An effected source's windows through its effect are rendered once and kept beside the
/// caches, so a sound-changing edit elsewhere, or to its own volume, doesn't render a 10-minute voiceover through
/// FFmpeg again; a trim of it does. The estimate stays within the I2 tolerance of the exact measure.
#[test]
fn an_edit_elsewhere_doesnt_render_an_effected_voiceover_again() {
    let dir = tempfile::tempdir().unwrap();
    let file = |name: &str| dir.path().join(name).display().to_string();
    let rate = RATE as f32;
    // Ten minutes of speech-like bursts, and a music bed.
    write_signal(Path::new(&file("voiceover.wav")), 1, 600, |n, _| {
        let t = n as f32 / rate;
        if (t * 0.5).fract() < 0.8 {
            0.3 * noise(11, n)
        } else {
            0.0
        }
    });
    write_signal(Path::new(&file("music.wav")), 2, 40, |n, c| {
        0.2 * noise(12 + c as u64, n)
    });
    let voice = Registry::builtin().audio_filter(&builtin::audio_effect("voice"));
    assert!(voice.is_some());
    let plan = |music_gain: f64, voice_gain: f64, voice_secs: f64| AudioPlan {
        sources: vec![
            AudioSource {
                path: file("music.wav"),
                end_us: secs(40.0),
                ..source(music_gain, 0, 1.0, 0)
            },
            AudioSource {
                path: file("voiceover.wav"),
                start_us: secs(5.0),
                end_us: secs(5.0 + voice_secs),
                fade_out_us: secs(2.0),
                filter: voice.map(Arc::from),
                ..source(voice_gain, secs(1.0), 1.0, 0)
            },
        ],
        loudness: Loudness::Youtube,
    };
    let store = PcmStore::new(dir.path().join("cache"));
    let pcm = |s: &AudioSource| store.build(&PcmKey::of(s)).ok();
    let music = store
        .build(&PcmKey {
            path: file("music.wav"),
        })
        .unwrap();
    let music_energies = store
        .energies(
            &PcmKey {
                path: file("music.wav"),
            },
            &music,
        )
        .unwrap();
    let energies = |s: &AudioSource| (s.filter.is_none()).then_some(&music_energies);
    let effected = |s: &AudioSource, p: &Arc<Pcm>, c: &AtomicBool| store.effected(s, p, c);
    let estimate = |plan: &AudioPlan| {
        let at = std::time::Instant::now();
        match estimate_until(plan, &pcm, &energies, &effected, &AtomicBool::new(false)) {
            Estimate::Lufs(Some(lufs)) => (lufs, at.elapsed()),
            other => panic!("{other:?}"),
        }
    };
    let (_, first) = estimate(&plan(0.5, 0.8, 600.0));
    assert_eq!(store.effect_renders(), 1);
    // A sound-changing edit elsewhere: the music's volume.
    let (edited, again) = estimate(&plan(0.3, 0.8, 600.0));
    assert_eq!(
        store.effect_renders(),
        1,
        "an edit elsewhere renders nothing"
    );
    // Its own volume and fades apply to the kept windows.
    estimate(&plan(0.3, 0.5, 600.0));
    assert_eq!(store.effect_renders(), 1, "nor does its own volume");
    // A trim changes what its effect hears: rendered once more, then kept too.
    estimate(&plan(0.3, 0.5, 590.0));
    estimate(&plan(0.6, 0.5, 590.0));
    assert_eq!(store.effect_renders(), 2, "a trim renders once");
    let exact = measure(&plan(0.3, 0.8, 600.0), &pcm).unwrap();
    println!(
        "10-minute effected voiceover: first estimate {first:?} (renders it), after an edit elsewhere {again:?}; \
         estimate {edited:.3} LUFS, exact {exact:.3} LUFS"
    );
    assert!((edited - exact).abs() < 0.5, "{edited} vs {exact}");
}

/// The ops a dry run (or an edit) can skip planning the sound for, as a table by op kind. Each
/// "can't" is checked against the plan itself: applied to a project with titles and images beside a sounding video
/// layer, a music bed and a zoom, the sound plan comes out exactly the same.
#[test]
fn ops_that_cant_change_the_sound_skip_the_replan() {
    use klipolo_model::ids::Id;
    use klipolo_model::layout::{Anchor, Layout};
    use klipolo_model::ops::{
        ClipPatch, Edge, EffectPatch, EffectTarget, ItemPatch, Op, TrackPatch, TrackTarget,
    };
    use klipolo_model::params::ParamValue;
    use klipolo_model::project::{Background, BackgroundKind};
    use klipolo_model::timeline::{EffectInstance, Look, ZoomTarget};

    let mut p = project();
    let video = ItemContent::Video {
        asset_id: "broll".into(),
        source_in_us: 0,
        speed: 1.0,
        volume: 0.8,
        muted: false,
        mirror: false,
        tie: None,
    };
    let mut image = item(
        "img",
        10.0,
        2.0,
        bx(0.1, 0.1, 0.2, 0.2),
        ItemContent::Image {
            asset_id: "logo".into(),
        },
    );
    let mut blur = EffectInstance::new(builtin::EFFECT_BLUR);
    blur.id = "fx".into();
    image.effects = vec![blur];
    let music = Item {
        id: "m".into(),
        start_us: 0,
        duration_us: secs(10.0),
        attach: None,
        layout: Layout::new(Anchor::Center, bx(0.0, 0.0, 1.0, 1.0)),
        content: ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: 0,
            volume: 0.5,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        effects: Vec::new(),
    };
    let mut audio = audio_track(false, vec![music]);
    audio.id = "bed".into();
    p.timeline.tracks = vec![
        overlay_track(
            "o",
            vec![
                item("t", 1.0, 2.0, bx(0.1, 0.1, 0.5, 0.2), text("Hello")),
                item("v", 5.0, 4.0, bx(0.5, 0.5, 0.3, 0.3), video.clone()),
                image,
            ],
        ),
        audio,
        zoom_track(vec![item(
            "z",
            2.0,
            3.0,
            bx(0.0, 0.0, 1.0, 1.0),
            ItemContent::Zoom {
                target: ZoomTarget::Auto { using: vec![] },
                scale: 2.0,
            },
        )]),
    ];
    let p = klipolo_model::apply_ops(&p, &[]).unwrap();
    let id = |s: &str| -> Id { s.into() };
    let mut settings = p.settings.clone();
    settings.fps = 60;
    let moved_title = Op::MoveItem {
        id: id("t"),
        track_id: None,
        start_us: secs(6.0),
    };
    let restyled = ItemPatch {
        content: Some(text("Hello there")),
        duration_us: Some(secs(5.0)),
        ..ItemPatch::default()
    };
    let param = EffectPatch {
        params: Some([("radius".to_string(), ParamValue::Number(12.0))].into()),
        ..EffectPatch::default()
    };
    let moved_layout = ItemPatch {
        layout: Some(Layout::new(Anchor::Center, bx(0.2, 0.2, 0.3, 0.3))),
        ..ItemPatch::default()
    };
    let cant = [
        ("rename", Op::Rename { name: "New".into() }),
        ("settings", Op::SetSettings { settings }),
        (
            "background",
            Op::SetBackground {
                background: Background {
                    kind: BackgroundKind::Solid,
                    color: "#ff0000".into(),
                    color_end: None,
                    asset_id: None,
                    padding: 0.1,
                },
            },
        ),
        (
            "new track",
            Op::AddTrack {
                id: None,
                kind: TrackKind::Overlay,
                index: Some(0),
            },
        ),
        (
            "track name and lock",
            Op::UpdateTrack {
                id: id("o"),
                patch: TrackPatch {
                    name: Some("Titles".into()),
                    locked: Some(true),
                    ..TrackPatch::default()
                },
            },
        ),
        (
            "clip look",
            Op::UpdateClip {
                id: id("a"),
                patch: ClipPatch {
                    look: Some(Look::default()),
                    ..ClipPatch::default()
                },
            },
        ),
        (
            "clip effect",
            Op::AddEffect {
                target: EffectTarget::Clip { id: id("a") },
                effect: EffectInstance::new(builtin::EFFECT_VIGNETTE),
                index: None,
            },
        ),
        (
            "effect param on a picture",
            Op::UpdateEffect {
                target: EffectTarget::Item { id: id("img") },
                id: id("fx"),
                patch: param,
            },
        ),
        (
            "effect off a picture",
            Op::RemoveEffect {
                target: EffectTarget::Item { id: id("img") },
                id: id("fx"),
            },
        ),
        (
            "add a title over the video",
            Op::AddItem {
                track: TrackTarget::Track { id: id("o") },
                item: item("t2", 6.0, 2.0, bx(0.1, 0.1, 0.5, 0.2), text("Hi")),
                attach: false,
            },
        ),
        // Over the video on its own track: the title goes above it; the video stays where it sounds.
        ("move a title", moved_title),
        (
            "style a title",
            Op::UpdateItem {
                id: id("t"),
                patch: restyled,
            },
        ),
        (
            "trim a title",
            Op::TrimItem {
                id: id("t"),
                edge: Edge::End,
                at_us: secs(7.0),
            },
        ),
        (
            "split a title",
            Op::SplitItem {
                id: id("t"),
                at_us: secs(2.0),
            },
        ),
        (
            "attach a title",
            Op::SetAttach {
                id: id("t"),
                attached: true,
            },
        ),
        ("remove an image", Op::RemoveItem { id: id("img") }),
        (
            "move a zoom",
            Op::MoveItem {
                id: id("z"),
                track_id: None,
                start_us: secs(12.0),
            },
        ),
        (
            "move the video's box",
            Op::UpdateItem {
                id: id("v"),
                patch: moved_layout,
            },
        ),
    ];
    let registry = Registry::builtin();
    let before = audio_plan(&p, &registry);
    for (what, op) in &cant {
        assert!(!may_change_sound(&p, op), "{what}");
        let after = klipolo_model::apply_ops(&p, std::slice::from_ref(op))
            .unwrap_or_else(|e| panic!("{what}: {e:?}"));
        assert_eq!(audio_plan(&after, &registry), before, "{what}");
    }
    let mut picture = item("img2", 20.0, 2.0, bx(0.1, 0.1, 0.2, 0.2), video);
    picture.id = "img2".into();
    let may = [
        (
            "loudness",
            Op::SetAudio {
                audio: Default::default(),
            },
        ),
        (
            "mute a track",
            Op::UpdateTrack {
                id: id("bed"),
                patch: TrackPatch {
                    muted: Some(true),
                    ..TrackPatch::default()
                },
            },
        ),
        (
            "solo a track",
            Op::UpdateTrack {
                id: id("o"),
                patch: TrackPatch {
                    solo: Some(true),
                    ..TrackPatch::default()
                },
            },
        ),
        (
            "hide a track",
            Op::UpdateTrack {
                id: id("o"),
                patch: TrackPatch {
                    hidden: Some(true),
                    ..TrackPatch::default()
                },
            },
        ),
        (
            "clip volume",
            Op::UpdateClip {
                id: id("a"),
                patch: ClipPatch {
                    volume: Some(0.5),
                    ..ClipPatch::default()
                },
            },
        ),
        (
            "add a video layer",
            Op::AddItem {
                track: TrackTarget::Auto,
                item: picture,
                attach: false,
            },
        ),
        (
            "an image becomes a video",
            Op::UpdateItem {
                id: id("img"),
                patch: ItemPatch {
                    content: Some(ItemContent::Video {
                        asset_id: "broll".into(),
                        source_in_us: 0,
                        speed: 1.0,
                        volume: 1.0,
                        muted: false,
                        mirror: false,
                        tie: None,
                    }),
                    ..ItemPatch::default()
                },
            },
        ),
        (
            "move the video",
            Op::MoveItem {
                id: id("v"),
                track_id: None,
                start_us: secs(15.0),
            },
        ),
        (
            "trim the music",
            Op::TrimItem {
                id: id("m"),
                edge: Edge::End,
                at_us: secs(4.0),
            },
        ),
        (
            "a layer this project doesn't have",
            Op::MoveItem {
                id: id("new"),
                track_id: None,
                start_us: 0,
            },
        ),
        ("split the main track", Op::Split { at_us: secs(3.0) }),
        ("a transition", set_dissolve("a", 1.0)),
        (
            "detach the camera",
            Op::DetachSource {
                source_id: "camera".into(),
                clip_id: id("a"),
            },
        ),
        ("remove a track", Op::RemoveTrack { id: id("o") }),
        (
            "replace the music's effects (a sound layer's are audio effects)",
            Op::UpdateItem {
                id: id("m"),
                patch: ItemPatch {
                    effects: Some(Vec::new()),
                    ..ItemPatch::default()
                },
            },
        ),
    ];
    for (what, op) in &may {
        assert!(may_change_sound(&p, op), "{what}");
    }
}

/// A one-sided mic played centred is levelled as it plays. The preview's estimate (its live channel's
/// energies, on both sides) and the export's exact measure agree, both read it as the same voice in a one-channel file,
/// and both put it 3 dB above the same file played as recorded (one side of two).
#[test]
fn a_one_sided_mic_played_centred_is_measured_as_it_plays() {
    let dir = tempfile::tempdir().unwrap();
    let file = |name: &str| dir.path().join(name);
    let rate = RATE as f32;
    // Speech-like: noise in 1.6 s phrases with 0.4 s pauses, on input 1 only.
    let voice = |n: usize| {
        if (n as f32 / rate * 0.5).fract() < 0.8 {
            0.3 * noise(1, n)
        } else {
            0.0
        }
    };
    write_signal(&file("one-sided.wav"), 2, 20, |n, c| {
        if c == 0 { voice(n) } else { 0.0 }
    });
    write_signal(&file("mono.wav"), 1, 20, |n, _| voice(n));
    let registry = Registry::builtin();
    // (the preview's estimate, the export's exact LUFS) of a take whose mic is `wav`, centred from `centre_from`.
    let levels = |wav: &str, centre_from: Option<u8>| {
        let mut p =
            support::recording::recording_project(&[("mic", SourceRole::Mic, 0, 20.0, 0.0)]);
        let mic = p.assets.iter_mut().find(|a| a.id == "mic").unwrap();
        mic.path = file(wav).display().to_string();
        p.recordings[0].sources[1].mix.centre_from = centre_from;
        let cache = dir.path().join("cache");
        let (_, export) =
            ExportAudio::prepare(&p, &registry, ExportQuality::High, Some(&cache), None).unwrap();
        let plan = audio_plan(&p, &registry);
        let store = PcmStore::new(cache);
        let pcm = |s: &AudioSource| store.build(&PcmKey::of(s)).ok();
        let key = PcmKey::of(&plan.sources[0]);
        let energies = store.energies(&key, &store.build(&key).unwrap()).unwrap();
        let effected = |s: &AudioSource, p: &Arc<Pcm>, c: &AtomicBool| store.effected(s, p, c);
        let Estimate::Lufs(Some(estimate)) = estimate_until(
            &plan,
            &pcm,
            &|_: &AudioSource| Some(&energies),
            &effected,
            &AtomicBool::new(false),
        ) else {
            panic!("no estimate");
        };
        (estimate, export.lufs.unwrap())
    };
    let (mono_estimate, mono) = levels("mono.wav", None);
    let (estimate, exact) = levels("one-sided.wav", Some(0));
    let (recorded_estimate, recorded) = levels("one-sided.wav", None);
    println!(
        "mono {mono:.3}/{mono_estimate:.3}, centred {exact:.3}/{estimate:.3}, as recorded {recorded:.3} LUFS"
    );
    assert!((exact - mono).abs() < 0.01, "export: {exact} vs {mono}");
    assert!(
        (estimate - mono_estimate).abs() < 0.01,
        "preview: {estimate} vs {mono_estimate}"
    );
    assert!(
        (estimate - exact).abs() < 0.1,
        "preview {estimate} vs export {exact}"
    );
    assert!(
        (exact - recorded - 3.01).abs() < 0.05,
        "{exact} vs {recorded}"
    );
    assert!(
        (estimate - recorded_estimate - 3.01).abs() < 0.05,
        "{estimate} vs {recorded_estimate}"
    );
}
