mod support;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use klipolo_engine::audio::mix::{Mixdown, RATE, read_position_after};
use klipolo_engine::audio::pcm::Pcm;
use klipolo_engine::audio::plan::{AudioPlan, AudioSource, audio_plan};
use klipolo_engine::media::proxy::decide::{ProxyPolicy, needs_proxy};
use klipolo_engine::platform::gpu::GpuClass;
use klipolo_engine::registry::Registry;
use klipolo_model::Op;
use klipolo_model::contrib::builtin;
use klipolo_model::ops::SourcePatch;
use klipolo_model::project::*;
use klipolo_model::timeline::EffectInstance;
use support::recording as common;

/// Counts this thread's allocations, so a test can check that steady playback allocates nothing.
struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: forwards to the system allocator; the counter is a const-initialised thread-local (no allocation).
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.realloc(p, layout, size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

#[test]
fn a_drifting_mic_plays_at_its_rate_and_stays_in_sync_after_an_hour() {
    let p = common::recording_project(&[("mic", SourceRole::Mic, 0, 3_600.0, 100.0)]);
    let sources: Vec<AudioSource> = audio_plan(&p, &Registry::builtin(), Arc::default()).sources;
    assert_eq!(sources.len(), 1);
    assert!((sources[0].rate - 1.0001).abs() < 1e-9);
    // After an hour of output at 48 kHz, the read position is an hour of the device's (fast) clock.
    let read = read_position_after(&sources[0], 3_600 * 48_000);
    let expected = 3_600.0 * 48_000.0 * 1.0001;
    assert!((read - expected).abs() / 48.0 < 1.0, "within a millisecond");
}

#[test]
fn a_gap_between_runs_is_silent_and_each_run_starts_at_its_time() {
    let p =
        common::recording_project_runs("mic", SourceRole::Mic, &[(0, 10.0), (12_000_000, 18.0)]);
    let spans: Vec<(i64, i64, i64)> = audio_plan(&p, &Registry::builtin(), Arc::default())
        .sources
        .iter()
        .map(|s| (s.start_us, s.end_us, s.source_in_us))
        .collect();
    assert_eq!(spans, [(0, 10_000_000, 0), (12_000_000, 30_000_000, 0)]);
}

#[test]
fn a_clip_override_mutes_one_source_only_there() {
    let mut p = common::recording_project(&[
        ("mic", SourceRole::Mic, 0, 60.0, 0.0),
        ("game-sound", SourceRole::GameAudio, 0, 60.0, 0.0),
    ]);
    common::split_at(&mut p, 30.0);
    p.timeline.main[1].source_mix.insert(
        "game-sound".into(),
        SourceMix {
            muted: true,
            ..SourceMix::default()
        },
    );
    let paths: Vec<String> = audio_plan(&p, &Registry::builtin(), Arc::default())
        .sources
        .iter()
        .map(|s| s.path.clone())
        .collect();
    assert_eq!(
        paths.iter().filter(|p| p.contains("game-sound")).count(),
        1,
        "heard in the first clip only"
    );
    assert_eq!(paths.iter().filter(|p| p.contains("mic")).count(), 2);
}

#[test]
fn a_klipolo_recording_needs_no_editing_copy() {
    let facts = common::facts(1920, 1080, 60, "hevc", 1_000_000);
    assert_eq!(
        needs_proxy(
            &facts,
            &ProxyPolicy::with_encoder(GpuClass::Integrated, true)
        ),
        None
    );
}

/// Mono PCM whose every frame holds its own index ÷ 1e6, so a sample read names the source frame it came from.
fn frame_numbers(seconds: usize) -> Arc<Pcm> {
    let frames = seconds * RATE as usize;
    Arc::new(Pcm::from_channels(
        (0..frames).map(|i| i as f32 / 1e6).collect(),
        1,
    ))
}

/// The source frame the mix played at each of `out`'s frames (gain 1, past any ramp).
fn heard(out: &[f32]) -> impl Iterator<Item = f64> + '_ {
    out.chunks_exact(2).map(|lr| f64::from(lr[0]) * 1e6)
}

/// A take whose mic runs 1,000 ppm fast (the most validation allows), 70 s of its own clock.
fn fast_mic() -> Project {
    common::recording_project(&[("mic", SourceRole::Mic, 0, 70.0, 1_000.0)])
}

#[test]
fn the_mixer_reads_a_drifting_run_where_its_rate_puts_it() {
    let plan = Arc::new(audio_plan(
        &fast_mic(),
        &Registry::builtin(),
        Arc::default(),
    ));
    let mic = plan.sources[0].clone();
    let pcm = frame_numbers(70);
    let lookup = |_: &AudioSource| Some(pcm.clone());
    let mut mixdown = Mixdown::new(plan);
    let mut block = vec![0f32; 2 * 4_800];
    // Streaming 50 s: the last frame played is 50 s of the mic's own clock plus its 1,000 ppm (50 ms, 2,400 frames).
    for k in 0..500 {
        mixdown.render(&lookup, k * 4_800, &mut block);
    }
    let last = heard(&block).last().unwrap();
    let want = read_position_after(&mic, 50 * 48_000 - 1);
    assert!((last - want).abs() < 2.0, "{last} vs {want}");
    // A seek re-seats there: timeline 30 s plays 30.03 s of the file (past the 3 ms ramp in).
    mixdown.render(&lookup, 30 * 48_000, &mut block);
    for (k, at) in heard(&block).enumerate().skip(200) {
        let want = read_position_after(&mic, 30 * 48_000 + k as u64);
        assert!((at - want).abs() < 2.0, "frame {k}: {at} vs {want}");
    }
}

#[test]
fn a_split_of_a_drifting_run_plays_straight_through_the_cut() {
    let mut p = fast_mic();
    common::split_at(&mut p, 30.0);
    let plan = Arc::new(audio_plan(&p, &Registry::builtin(), Arc::default()));
    assert_eq!(plan.sources.len(), 2);
    let first = plan.sources[0].clone();
    let pcm = frame_numbers(70);
    let lookup = |_: &AudioSource| Some(pcm.clone());
    let mut mixdown = Mixdown::new(plan);
    let cut = 30 * 48_000;
    let mut block = vec![0f32; 2 * 960];
    // Seated a little before the cut (ramped in there), then one voice plays across it: no ramp out and in (the sound
    // would dip to silence at the cut), no jump.
    mixdown.render(&lookup, cut - 5 * 960, &mut block);
    for from in (cut - 4 * 960..cut + 4 * 960).step_by(960) {
        mixdown.render(&lookup, from, &mut block);
        for (k, at) in heard(&block).enumerate() {
            let want = read_position_after(&first, from + k as u64);
            assert!(
                (at - want).abs() < 2.0,
                "frame {}: {at} vs {want}",
                from + k as u64
            );
        }
    }
}

#[test]
fn a_drifting_voice_renders_without_allocating() {
    let pcm = frame_numbers(20);
    let lookup = |_: &AudioSource| Some(pcm.clone());
    let plan = AudioPlan {
        sources: vec![
            audio_plan(&fast_mic(), &Registry::builtin(), Arc::default())
                .sources
                .remove(0),
        ],
        loudness: Loudness::Off,
    };
    let mut mixdown = Mixdown::new(Arc::new(plan));
    let mut block = vec![0f32; 2 * 1_024];
    mixdown.render(&lookup, 0, &mut block);
    let before = ALLOCS.with(Cell::get);
    for k in 1..400 {
        mixdown.render(&lookup, k * 1_024, &mut block);
    }
    assert_eq!(
        ALLOCS.with(Cell::get) - before,
        0,
        "allocations while playing"
    );
}

#[test]
fn a_source_s_effects_play_in_its_clip_and_go_with_it_when_separated() {
    let mut p = common::recording_project(&[("mic", SourceRole::Mic, 0, 60.0, 0.0)]);
    let fx = |id: &str, word: &str, enabled: bool| EffectInstance {
        id: id.into(),
        effect: builtin::audio_effect(word),
        enabled,
        params: Default::default(),
    };
    p.recordings[0].sources[1].mix.effects = vec![
        fx("fx1", "voice", true),
        fx("fx2", "warm", false),
        fx("fx3", "telephone", true),
    ];
    let registry = Registry::builtin();
    let filters = |p: &Project| -> Vec<Option<String>> {
        audio_plan(p, &registry, Arc::default())
            .sources
            .iter()
            .map(|s| s.filter.as_deref().map(str::to_owned))
            .collect()
    };
    let chain = |word| registry.audio_filter(&builtin::audio_effect(word)).unwrap();
    // The ones that are on, in order.
    let heard = format!("{},{}", chain("voice"), chain("telephone"));
    assert_eq!(filters(&p), [Some(heard.clone())]);
    let separate = Op::DetachSource {
        clip_id: "take".into(),
        source_id: "mic".into(),
    };
    let p = klipolo_model::apply_ops(&p, &[separate]).unwrap();
    assert_eq!(
        filters(&p),
        [Some(heard)],
        "its layer plays them as the clip did"
    );
}

#[test]
fn a_sync_change_moves_a_separated_sound_as_it_moves_the_clip_s_own() {
    // The fast mic, split at 30 s, then nudged 200 ms later: separated in both clips first, its layers play exactly what
    // the clips' own sound plays after the same nudge (start, end, file time and rate), across the cut too.
    let mut p = fast_mic();
    common::split_at(&mut p, 30.0);
    let nudge = Op::UpdateRecordingSource {
        recording_id: "rec".into(),
        source_id: "mic".into(),
        patch: SourcePatch {
            nudge_us: Some(200_000),
            ..SourcePatch::default()
        },
    };
    let mut separate: Vec<Op> = p
        .timeline
        .main
        .iter()
        .map(|c| Op::DetachSource {
            clip_id: c.id.clone(),
            source_id: "mic".into(),
        })
        .collect();
    separate.push(nudge.clone());
    let spans = |p: &Project| {
        let mut spans: Vec<(i64, i64, i64, u64)> =
            audio_plan(p, &Registry::builtin(), Arc::default())
                .sources
                .iter()
                .map(|s| (s.start_us, s.end_us, s.source_in_us, s.rate.to_bits()))
                .collect();
        spans.sort_unstable();
        spans
    };
    let heard = klipolo_model::apply_ops(&p, &[nudge]).unwrap();
    let separated = klipolo_model::apply_ops(&p, &separate).unwrap();
    assert_eq!(spans(&heard).len(), 2);
    assert_eq!(spans(&separated), spans(&heard));
}

/// A 440 Hz tone of `seconds` on the left only of a stereo PCM (a mic on input 1 of a two-input interface), and the
/// same tone as a one-channel file.
fn one_sided(seconds: usize) -> (Arc<Pcm>, Arc<Pcm>) {
    let tone: Vec<f32> = (0..seconds * RATE as usize)
        .map(|n| 0.25 * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / RATE as f32).sin())
        .collect();
    let stereo = tone.iter().flat_map(|&x| [x, 0.0]).collect();
    (
        Arc::new(Pcm::from_channels(stereo, 2)),
        Arc::new(Pcm::from_channels(tone, 1)),
    )
}

/// A mic recorded [voice, silence] and played centred from its live channel sounds
/// exactly as the same voice in a one-channel file does, the same on both sides: at 1×, drifting, and through an
/// effect. Steady playback of it allocates nothing. Played as recorded, it is heard from the left only.
#[test]
fn a_one_sided_mic_played_centred_sounds_as_a_mono_one() {
    const BLOCKS: u64 = 50;
    let (stereo, mono) = one_sided(12);
    let registry = Registry::builtin();
    // The mix of `plan` with every file read from `pcm`: its samples, and the allocations after the first block.
    let play = |plan: &AudioPlan, pcm: &Arc<Pcm>| -> (Vec<f32>, u64) {
        let lookup = |_: &AudioSource| Some(pcm.clone());
        let mut mixdown = Mixdown::new(Arc::new(plan.clone()));
        let mut block = vec![0f32; 2 * 1_024];
        let mut out = Vec::with_capacity(BLOCKS as usize * block.len());
        mixdown.render(&lookup, 0, &mut block);
        out.extend_from_slice(&block);
        let before = ALLOCS.with(Cell::get);
        for k in 1..BLOCKS {
            mixdown.render(&lookup, k * 1_024, &mut block);
            out.extend_from_slice(&block);
        }
        (out, ALLOCS.with(Cell::get) - before)
    };
    for (ppm, effect) in [(0.0, false), (1_000.0, false), (0.0, true)] {
        let case = format!("{ppm} ppm, effect {effect}");
        let mut p = common::recording_project(&[("mic", SourceRole::Mic, 0, 10.0, ppm)]);
        if effect {
            p.recordings[0].sources[1].mix.effects =
                vec![EffectInstance::new(builtin::audio_effect("voice"))];
        }
        let as_recorded = audio_plan(&p, &registry, Arc::default());
        p.recordings[0].sources[1].mix.centre_from = Some(0);
        let centred = audio_plan(&p, &registry, Arc::default());

        let (reference, _) = play(&as_recorded, &mono);
        let (heard, allocations) = play(&centred, &stereo);
        assert!(
            reference.chunks_exact(2).all(|lr| lr[0] == lr[1]),
            "{case}: a mono file plays the same on both sides"
        );
        assert!(reference.iter().any(|x| x.abs() > 0.1), "{case}: it sounds");
        assert!(
            heard == reference,
            "{case}: centred, as the mono file plays"
        );
        if !effect {
            assert_eq!(allocations, 0, "{case}: allocations while playing");
        }

        let (one_side, _) = play(&as_recorded, &stereo);
        assert!(
            one_side.chunks_exact(2).all(|lr| lr[1] == 0.0),
            "{case}: as recorded, nothing on the right"
        );
        // Through an effect the stereo pair is filtered with its silent side, so only a dry read matches exactly.
        assert!(
            effect
                || one_side
                    .chunks_exact(2)
                    .zip(reference.chunks_exact(2))
                    .all(|(a, b)| a[0] == b[0]),
            "{case}: and the voice on the left"
        );
    }
}

#[test]
fn the_second_run_of_a_paused_file_plays_from_its_file_time() {
    // A recorder paused at 20 s of its own time while the take's clock ran on 7 s: two runs of its one file,
    // the second playing the file from its 20 s at the clock's 27 s. Inside the skipped stretch (20–27 s on the clock)
    // nothing of the mic plays; just after it, the mixer reads the file at the second run's file time.
    let mut p = common::recording_project(&[("mic", SourceRole::Mic, 0, 50.0, 0.0)]);
    let run = |offset_us, file_in_us, duration_us| SourceRun {
        asset_id: "mic".into(),
        offset_us,
        duration_us,
        drift_ppm: 0.0,
        file_in_us,
    };
    p.recordings[0].sources[1].runs = vec![
        run(0, 0, 20_000_000),
        run(27_000_000, 20_000_000, 30_000_000),
    ];
    klipolo_model::validate(&p).expect("two runs of one file, apart in it");
    let plan = Arc::new(audio_plan(&p, &Registry::builtin(), Arc::default()));
    let spans: Vec<(i64, i64, i64)> = plan
        .sources
        .iter()
        .map(|s| (s.start_us, s.end_us, s.source_in_us))
        .collect();
    assert_eq!(
        spans,
        [(0, 20_000_000, 0), (27_000_000, 57_000_000, 20_000_000)]
    );
    let pcm = frame_numbers(50);
    let lookup = |_: &AudioSource| Some(pcm.clone());
    let mut mixdown = Mixdown::new(plan);
    let mut block = vec![0f32; 2 * 4_800];
    mixdown.render(&lookup, 23 * 48_000, &mut block);
    assert!(
        block.iter().all(|&x| x == 0.0),
        "nothing plays between the runs"
    );
    // 27.1 s on the clock is the file's 20.1 s (past the 3 ms ramp in).
    let after = 27 * 48_000 + 4_800;
    mixdown.render(&lookup, after, &mut block);
    for (k, at) in heard(&block).enumerate().skip(200) {
        let want = (20 * 48_000 + 4_800 + k) as f64;
        assert!((at - want).abs() < 2.0, "frame {k}: {at} vs {want}");
    }
}
