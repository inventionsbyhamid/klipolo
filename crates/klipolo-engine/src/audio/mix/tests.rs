//! The sweep over sources in order of start mixes exactly what the walk over every source
//! did, while looking at only the sources sounding in each block.

use std::sync::Arc;

use klipolo_model::project::Loudness;

use super::{Mixdown, RATE};
use crate::audio::pcm::Pcm;
use crate::audio::plan::{AudioPlan, AudioSource, Crossfade};

/// Frames per clip: 1,500 clips make 94 s.
const CLIP: i64 = 3_000;

fn us(frames: i64) -> i64 {
    frames * 1_000_000 / i64::from(RATE)
}

fn source(path: &str, start: i64, end: i64, source_in: i64) -> AudioSource {
    AudioSource {
        path: path.into(),
        start_us: us(start),
        end_us: us(end),
        source_in_us: us(source_in),
        speed: 1.0,
        rate: 1.0,
        gain: 0.7,
        fade_in_us: 0,
        fade_out_us: 0,
        crossfade: Crossfade::default(),
        filter: None,
        centre_from: None,
        duck: None,
        seam: Default::default(),
        line: None,
    }
}

/// A long project's shape at a small scale: 1,500 recording clips, each a mic and a system source (3,000), with splits
/// that play through their cut, fades, crossfades into the next clip, and a music bed under all of it.
fn plan() -> AudioPlan {
    let mut sources = Vec::new();
    for k in 0..1_500i64 {
        let (start, end) = (k * CLIP, (k + 1) * CLIP);
        // Every fifth clip carries on from the one before (a split with nothing removed): one voice plays both.
        let source_in = if k % 5 == 0 { 0 } else { (k % 5) * CLIP };
        let mut mic = source("/mic", start, end, source_in);
        let mut system = source("/system", start, end, source_in + 7);
        if k % 7 == 3 {
            (mic.fade_in_us, mic.fade_out_us) = (us(500), us(800));
        }
        if k % 11 == 5 {
            // Crossfades into the clip after it.
            system.end_us = us(end + 1_000);
            system.fade_out_us = us(2_000);
            system.crossfade.fade_out = true;
        }
        sources.extend([mic, system]);
    }
    let mut bed = source("/system", 0, 1_500 * CLIP, 0);
    bed.gain = 0.2;
    sources.push(bed);
    AudioPlan {
        sources,
        loudness: Loudness::Off,
    }
}

fn pcm(channels: usize, seconds: usize) -> Arc<Pcm> {
    let samples = (0..seconds * RATE as usize * channels)
        .map(|n| ((n as f32 * 0.013).sin() * 0.4) + ((n % 97) as f32 / 970.0))
        .collect();
    Arc::new(Pcm::from_channels(samples, channels))
}

#[test]
fn the_sweep_mixes_exactly_what_the_walk_over_every_source_did() {
    let (mic, system) = (pcm(1, 30), pcm(2, 30));
    let lookup = |s: &AudioSource| Some(if s.path == "/mic" { &mic } else { &system }.clone());
    let plan = Arc::new(plan());
    let (mut sweep, mut walk) = (Mixdown::new(plan.clone()), Mixdown::new(plan.clone()));
    walk.walk_every_source();
    let (mut a, mut b) = (vec![0f32; 2 * 4_096], vec![0f32; 2 * 4_096]);
    // Blocks mixed, and what the walk looks at: every source of the plan, every block.
    let (mut blocks, mut every) = (0u64, 0u64);
    let mut both = |at: u64, sweep: &mut Mixdown, walk: &mut Mixdown| {
        sweep.render(&lookup, at, &mut a);
        walk.render(&lookup, at, &mut b);
        assert!(a == b, "block at frame {at} differs");
        blocks += 1;
        every += walk.plan().sources.len() as u64;
    };
    let end = (1_500 * CLIP) as u64;
    // A full pass, then jumps back and forward (seeks), then an edit and on.
    for at in (0..end).step_by(4_096) {
        both(at, &mut sweep, &mut walk);
    }
    for at in [end / 2, end / 2 + 4_096, 1_000, 5_096, end - 5_000] {
        both(at, &mut sweep, &mut walk);
    }
    let mut edited = (*plan).clone();
    edited.sources.retain(|s| s.start_us % 3 != 0);
    for s in edited.sources.iter_mut().step_by(4) {
        s.gain = 0.4;
    }
    let edited = Arc::new(edited);
    sweep.set_plan(edited.clone());
    walk.set_plan(edited);
    for at in (end / 3..end).step_by(4_096) {
        both(at, &mut sweep, &mut walk);
    }
    // The walk looked at every source in every block; the sweep at a handful.
    assert_eq!(walk.sweep.looked, every);
    let per_block = sweep.sweep.looked as f64 / blocks as f64;
    assert!(per_block < 8.0, "{per_block} sources looked at per block");
}

/// A jump can hand a voice on to the next part of a split that doesn't sound there either: it is let go on the next
/// block, as the walk let it go, instead of holding its file until the next edit.
#[test]
fn a_voice_handed_on_across_a_jump_is_let_go() {
    let mic = pcm(1, 5);
    let lookup = |_: &AudioSource| Some(mic.clone());
    // "a" plays 0–1,000; "b" carries on from it until 1,010.
    let plan = AudioPlan {
        sources: vec![
            source("/mic", 0, 1_000, 0),
            source("/mic", 1_000, 1_010, 1_000),
        ],
        loudness: Loudness::Off,
    };
    let mut mixdown = Mixdown::new(Arc::new(plan));
    let mut out = vec![0f32; 2 * 500];
    mixdown.render(&lookup, 500, &mut out);
    assert!(mixdown.voices[0].is_some());
    // A seek far past both: "a" hands its voice to "b", which isn't sounding there.
    mixdown.render(&lookup, 50_000, &mut out);
    mixdown.render(&lookup, 50_500, &mut out);
    assert!(mixdown.voices.iter().all(Option::is_none));
}

/// Volume keys set by hand: the source's gain follows its line sample by sample, the first key's level before it and
/// the last's after it, eased in between, on top of its own gain.
#[test]
fn a_line_of_volume_keys_sets_the_gain_between_keys() {
    use klipolo_model::motion::{Ease, Key};

    use crate::audio::plan::Line;

    let steady = Arc::new(Pcm::from_channels(vec![0.5; 2 * 3 * RATE as usize], 2));
    let key = |t_us: i64, v: f64, ease: Option<Ease>| Key { t_us, v, ease };
    let mut s = source("/steady", 0, 2 * i64::from(RATE), 0);
    s.gain = 0.8;
    // From 0.5 s: full, down to silence at 1.5 s, then up to half over an ease in to 1.9 s.
    s.line = Some(Line {
        origin_us: 500_000,
        keys: Arc::from(vec![
            key(0, 1.0, None),
            key(1_000_000, 0.0, None),
            key(1_400_000, 0.5, Some(Ease::EaseIn)),
        ]),
    });
    let plan = AudioPlan {
        sources: vec![s],
        loudness: Loudness::Off,
    };
    let mut out = vec![0f32; 2 * 2 * RATE as usize];
    super::mix(&plan, &|_| Some(steady.clone()), 0, 1.0, &mut out);
    let at = |seconds: f64| out[2 * (seconds * f64::from(RATE)).round() as usize];
    let near = |seconds: f64, level: f64| {
        let want = 0.5 * 0.8 * level;
        assert!(
            (f64::from(at(seconds)) - want).abs() < 1e-4,
            "{seconds} s: {} for {want}",
            at(seconds)
        );
    };
    near(0.25, 1.0);
    near(1.0, 0.5);
    near(1.25, 0.25);
    near(1.5, 0.0);
    // An ease in starts slow: under the straight line's level halfway.
    assert!(f64::from(at(1.7)) < 0.5 * 0.8 * 0.25);
    near(1.95, 0.5);
}
