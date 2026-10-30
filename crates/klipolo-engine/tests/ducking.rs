//! Ducking: a sound layer with `duck` set (`ItemContent::Audio::duck`) is lowered by that many dB wherever someone
//! speaks (`speech_ranges`), easing down over 150 ms before each stretch of speech and back up over 400 ms after it,
//! sample by sample, the same in the preview's mix and export's. The mixdowns here are offline: a 1 kHz tone is the
//! music (synthetic PCM, or a WAV file where export reads one), every other file is silent, and the speech ranges are
//! given directly.
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_engine::api::ExportQuality;
use klipolo_engine::audio::mix::{Mixdown, RATE, finish};
use klipolo_engine::audio::pcm::Pcm;
use klipolo_engine::audio::plan::{AudioPlan, AudioSource, audio_plan, speech_ranges};
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::naming::plan_files;
use klipolo_engine::export::runner::Batch;
use klipolo_engine::registry::Registry;
use klipolo_model::project::{FormatKind, Loudness, Project};
use klipolo_model::speech::{Levels, Speech, Transcript, Word, WordKind};
use klipolo_model::time::RangeUs;
use klipolo_model::timeline::{ItemContent, Track, TrackKind};
use support::*;

const DUCK_DB: f64 = -12.0;
/// The tone's peak. At the bed's 50 % it plays at 0.125, far under the soft clip's knee and the limiter's ceiling.
const AMPLITUDE: f32 = 0.25;
/// 48 samples a period at 48 kHz: each 1 ms window holds one whole period, so its RMS is steady.
const TONE_HZ: f32 = 1_000.0;
/// What the mixes here cover: both stretches of speech and the gap after them.
const HEARD_S: f64 = 22.0;

fn range(from: f64, to: f64) -> RangeUs {
    RangeUs {
        start: secs(from),
        end: secs(to),
    }
}

/// Someone speaks from 5 to 9 s and from 15 to 20 s.
fn speech() -> Arc<[RangeUs]> {
    Arc::from(vec![range(5.0, 9.0), range(15.0, 20.0)])
}

fn tone_at(n: usize) -> f32 {
    AMPLITUDE * (2.0 * std::f32::consts::PI * TONE_HZ * n as f32 / RATE as f32).sin()
}

/// The tone on both sides, 30 s.
fn tone() -> Arc<Pcm> {
    let frames = 30 * RATE as usize;
    Arc::new(Pcm::from_samples(
        (0..frames)
            .flat_map(|n| {
                let x = tone_at(n);
                [x, x]
            })
            .collect(),
    ))
}

/// The engine fixture (a 30 s main track: the recording "rec" then the "broll" file) with a music bed over all of it at
/// 50 %, ducking by `duck`. Loudness is off, so the export's gain is unity.
fn with_music(duck: Option<f64>) -> Project {
    let mut p = project();
    let mut track = Track::new(TrackKind::Audio, "Music");
    track.id = "music-track".into();
    track.items.push(item(
        "bed",
        0.0,
        30.0,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Audio {
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
            duck,
        },
    ));
    p.timeline.tracks.push(track);
    p.audio.loudness = Loudness::Off;
    klipolo_model::validate(&p).unwrap();
    p
}

/// `with_music` with every clip muted: the music is all there is to hear, and export has no other file to read.
fn music_alone(duck: Option<f64>) -> Project {
    let mut p = with_music(duck);
    for clip in &mut p.timeline.main {
        clip.muted = true;
    }
    p
}

fn plan(p: &Project, speech: Arc<[RangeUs]>) -> AudioPlan {
    audio_plan(p, &Registry::builtin(), speech)
}

/// The mix from timeline frame 0 for `seconds`, as playback renders it: one `Mixdown`, block after block, each finished
/// at unity (the soft clip, linear at these levels).
fn render(
    plan: AudioPlan,
    pcm: &dyn Fn(&AudioSource) -> Option<Arc<Pcm>>,
    seconds: f64,
) -> Vec<f32> {
    const BLOCK: usize = 1_024;
    let frames = (seconds * f64::from(RATE)) as usize;
    let mut mixdown = Mixdown::new(Arc::new(plan));
    let mut out = vec![0f32; 2 * frames];
    for (k, block) in out.chunks_mut(2 * BLOCK).enumerate() {
        mixdown.render(pcm, (k * BLOCK) as u64, block);
        finish(block, 1.0, 1.0);
    }
    out
}

/// The mix with the tone as the music and every other file silent.
fn render_tone(plan: AudioPlan) -> Vec<f32> {
    let tone = tone();
    render(
        plan,
        &|s: &AudioSource| s.path.ends_with("music").then(|| tone.clone()),
        HEARD_S,
    )
}

#[test]
fn no_speech_data_means_no_ducking() {
    // Neither a transcript nor levels for any file: nobody is known to speak.
    let p = with_music(Some(DUCK_DB));
    let none = speech_ranges(&p, &Speech::default());
    assert!(none.is_empty(), "{none:?}");
    let ducked = plan(&music_alone(Some(DUCK_DB)), none);
    assert!(ducked.sources.iter().all(|s| s.duck.is_none()));
    // The bed plays exactly as one that doesn't duck.
    let plain = plan(&music_alone(None), speech());
    assert!(plain.sources.iter().all(|s| s.duck.is_none()));
    assert_eq!(render_tone(ducked), render_tone(plain));
}

/// The mic's transcript (the recording's voice, clip "a" plays its first 20 s at 0 s): `words` as (from, to, kind),
/// reaching `covered` seconds into the file.
fn transcript(words: &[(f64, f64, WordKind)], covered: f64, complete: bool) -> Arc<Transcript> {
    Arc::new(Transcript {
        key: "mic".into(),
        language: "en".into(),
        model: "test".into(),
        covered_us: secs(covered),
        complete,
        words: words
            .iter()
            .map(|&(from, to, kind)| Word {
                text: if kind == WordKind::Pause { "" } else { "word" }.into(),
                start_us: secs(from),
                end_us: secs(to),
                confidence: 0.9,
                kind,
            })
            .collect(),
    })
}

fn heard_in(transcripts: &[(&str, Arc<Transcript>)], levels: &[(&str, Arc<Levels>)]) -> Speech {
    Speech {
        transcripts: transcripts
            .iter()
            .map(|(id, t)| (id.to_string(), t.clone()))
            .collect(),
        levels: levels
            .iter()
            .map(|(id, l)| (id.to_string(), l.clone()))
            .collect(),
        topics: HashMap::new(),
    }
}

#[test]
fn speech_is_the_words_with_short_gaps_joined() {
    let p = with_music(Some(DUCK_DB));
    let words = transcript(
        &[
            (1.0, 1.4, WordKind::Word),
            // 200 ms later: under 300 ms, so the music stays down between them.
            (1.6, 2.0, WordKind::Word),
            // A pause word is a gap, not speech.
            (2.0, 3.0, WordKind::Pause),
            (3.0, 3.5, WordKind::Filler),
            // 300 ms later: the music lifts between them.
            (3.8, 4.2, WordKind::Word),
        ],
        60.0,
        true,
    );
    assert_eq!(
        &*speech_ranges(&p, &heard_in(&[("mic", words)], &[])),
        [range(1.0, 2.0), range(3.0, 3.5), range(3.8, 4.2)]
    );
}

/// The mic's levels (a dBFS value per 100 ms, 60 s): loud from 1 to 4 s, 10 to 12 s and 30 to 40 s, quiet elsewhere.
fn mic_levels() -> Arc<Levels> {
    let loud =
        |s: f64| (1.0..4.0).contains(&s) || (10.0..12.0).contains(&s) || (30.0..40.0).contains(&s);
    Arc::new(Levels {
        db: (0..600)
            .map(|w| if loud(w as f64 / 10.0) { -20.0 } else { -70.0 })
            .collect(),
    })
}

#[test]
fn levels_stand_in_until_the_words_arrive() {
    let p = with_music(Some(DUCK_DB));
    // Levels only: what isn't a pause, each pause keeping 150 ms of breath at both ends, except where the file starts.
    let levels = heard_in(&[], &[("mic", mic_levels())]);
    assert_eq!(
        &*speech_ranges(&p, &levels),
        [range(0.85, 4.15), range(9.85, 12.15)]
    );
    // A transcript still being made: its words as far as it reaches (5 s), the levels after that.
    let partial = transcript(&[(1.0, 2.0, WordKind::Word)], 5.0, false);
    assert_eq!(
        &*speech_ranges(&p, &heard_in(&[("mic", partial)], &[("mic", mic_levels())])),
        [range(1.0, 2.0), range(9.85, 12.15)]
    );
    // A finished transcript wins over the levels everywhere.
    let done = transcript(&[(1.0, 2.0, WordKind::Word)], 60.0, true);
    assert_eq!(
        &*speech_ranges(&p, &heard_in(&[("mic", done)], &[("mic", mic_levels())])),
        [range(1.0, 2.0)]
    );
    // A file that is quiet throughout has nobody speaking in it.
    let quiet = Arc::new(Levels {
        db: vec![-70.0; 600].into(),
    });
    assert!(speech_ranges(&p, &heard_in(&[], &[("mic", quiet)])).is_empty());
    // Nor does a clip whose sound is off.
    let mut muted = p.clone();
    muted.timeline.main[0].muted = true;
    assert!(speech_ranges(&muted, &levels).is_empty());
}

/// An export job made with speech freezes the ranges a ducking layer goes down under, reads them back when resumed,
/// and goes stale when the speech they came from changes; a job whose project doesn't duck ignores speech.
#[test]
fn the_export_job_freezes_the_speech_it_ducks_under() {
    let dir = tempfile::tempdir().unwrap();
    let batch = |p: Project, speech: &Speech| Batch {
        files: plan_files(
            &p,
            &[FormatKind::Landscape],
            ExportQuality::High,
            dir.path(),
        ),
        project: p,
        fps: 30,
        quality: ExportQuality::High,
        pcm_dir: None,
        speech: Arc::new(speech.clone()),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor: false,
    };
    let words = transcript(&[(1.0, 2.0, WordKind::Word)], 60.0, true);
    let said = heard_in(&[("mic", words)], &[]);
    let p = with_music(Some(DUCK_DB));
    let job = ExportJob::create(&batch(p.clone(), &said), 30).unwrap();
    assert_eq!(job.speech_ranges, [range(1.0, 2.0)]);
    let opened = ExportJob::open(&job.dir).unwrap();
    assert_eq!(
        opened.speech_ranges, job.speech_ranges,
        "read back as frozen"
    );
    assert!(job.is_current(&p, &said));
    assert!(
        !job.is_current(&p, &Speech::default()),
        "other speech: start over"
    );

    let plain = with_music(None);
    let job = ExportJob::create(&batch(plain.clone(), &said), 30).unwrap();
    assert!(job.speech_ranges.is_empty(), "nothing ducks");
    assert!(job.is_current(&plain, &Speech::default()));
}
