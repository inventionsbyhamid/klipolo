//! Ducking: a sound layer with `duck` set (`ItemContent::Audio::duck`) is lowered by that many dB wherever someone
//! speaks (`speech_ranges`), easing down over 150 ms before each stretch of speech and back up over 400 ms after it,
//! sample by sample, the same in the preview's mix and export's. The mixdowns here are offline: a 1 kHz tone is the
//! music (synthetic PCM, or a WAV file where export reads one), every other file is silent, and the speech ranges are
//! given directly.
mod support;

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use klipolo_engine::api::ExportQuality;
use klipolo_engine::audio::loudness::{Estimate, estimate_until, measure};
use klipolo_engine::audio::mix::{ATTACK_US, Mixdown, RATE, RELEASE_US, finish};
use klipolo_engine::audio::pcm::{Pcm, PcmKey, PcmStore};
use klipolo_engine::audio::plan::{
    AudioPlan, AudioSource, JOIN_US, audio_plan, ducking_speech, ducking_speech_after,
    may_change_sound, speech_ranges,
};
use klipolo_engine::export::audio::{CEILING_DBTP, ExportAudio};
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::naming::plan_files;
use klipolo_engine::export::runner::Batch;
use klipolo_engine::registry::Registry;
use klipolo_engine::session::blank::BlankBackend;
use klipolo_engine::session::{
    Engine, EngineOptions, PreviewBackend, ProjectFeed, RenderRequest, Rendered,
};
use klipolo_model::Op;
use klipolo_model::project::{FormatKind, Loudness, Project};
use klipolo_model::speech::{Levels, Speech, Transcript, Word, WordKind};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{ItemContent, Track, TrackKind};
use support::*;
use tokio::sync::watch;

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

/// The left channel's RMS over `[from, to)` seconds, in dBFS.
fn rms_db(mix: &[f32], from: f64, to: f64) -> f64 {
    let frame = |s: f64| (s * f64::from(RATE)).round() as usize;
    let (a, b) = (frame(from), frame(to));
    let power: f64 = mix[2 * a..2 * b]
        .chunks_exact(2)
        .map(|lr| f64::from(lr[0]).powi(2))
        .sum::<f64>()
        / (b - a) as f64;
    10.0 * power.log10()
}

/// The bed's own level: the tone at 50 %.
fn own_db() -> f64 {
    20.0 * (0.5 * f64::from(AMPLITUDE) / 2f64.sqrt()).log10()
}

#[test]
fn music_is_12db_lower_during_speech() {
    let mix = render_tone(plan(&music_alone(Some(DUCK_DB)), speech()));
    let between = rms_db(&mix, 11.0, 13.0);
    assert!(
        (between - own_db()).abs() < 0.05,
        "between speech it plays at its own volume: {between:.2} vs {:.2} dB",
        own_db()
    );
    for (from, to) in [(6.0, 8.0), (16.0, 19.0)] {
        let talking = rms_db(&mix, from, to);
        println!(
            "{from}-{to} s: {:.3} dB under the bed between speech",
            talking - between
        );
        assert!(
            (talking - between - DUCK_DB).abs() < 0.5,
            "{from}-{to} s: {talking:.2} dB while speaking, {between:.2} dB between"
        );
    }
}

#[test]
fn attack_and_release_are_smooth() {
    let mix = render_tone(plan(&music_alone(Some(DUCK_DB)), speech()));
    // 1 ms windows from before the first attack to after its release: no step between two of them over 1 dB.
    let ms: Vec<f64> = (4_500..9_800)
        .map(|m| rms_db(&mix, f64::from(m) / 1e3, f64::from(m + 1) / 1e3))
        .collect();
    let (steepest, at) = ms
        .windows(2)
        .enumerate()
        .map(|(k, w)| ((w[1] - w[0]).abs(), 4_500 + k))
        .fold((0.0, 0), |a, b| if b.0 > a.0 { b } else { a });
    println!("steepest 1 ms step: {steepest:.3} dB at {at} ms");
    assert!(steepest <= 1.0, "a {steepest:.2} dB step at {at} ms");
    // And it does duck: down from 150 ms before the words, all the way when they start, back up over 400 ms after.
    let level = |s: f64| rms_db(&mix, s, s + 0.001) - own_db();
    for (s, want, what) in [
        (4.80, 0.0, "before the attack"),
        (4.925, DUCK_DB / 2.0, "half way down"),
        (5.0, DUCK_DB, "down as the words start"),
        (8.99, DUCK_DB, "down as they end"),
        (9.2, DUCK_DB / 2.0, "half way back up"),
        (9.45, 0.0, "back up"),
    ] {
        let got = level(s);
        println!("{what} ({s} s): {got:.3} dB");
        assert!(
            (got - want).abs() < 0.25,
            "{what} ({s} s): {got:.2} dB, want {want:.2}"
        );
    }
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

#[test]
fn export_mix_matches_preview_mix() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("music.wav");
    write_tone(&wav, 30);
    let mut p = music_alone(Some(DUCK_DB));
    p.assets.iter_mut().find(|a| a.id == "music").unwrap().path = wav.display().to_string();
    let registry = Registry::builtin();
    let cache = dir.path().join("cache");
    let (export, levels) = ExportAudio::prepare(
        &p,
        &registry,
        ExportQuality::High,
        Some(&cache),
        None,
        speech(),
    )
    .unwrap();
    assert_eq!(levels.gain_db, 0.0, "loudness off: unity");
    // The same plan on both paths.
    let preview = audio_plan(&p, &registry, speech());
    assert_eq!(**export.plan(), preview);
    assert!(preview.sources.iter().any(|s| s.duck.is_some()));
    // And the same sound: the preview's mixdown against export's stream (its limiter idle at these levels).
    let store = PcmStore::new(cache);
    let heard = render(
        preview,
        &|s: &AudioSource| store.build(&PcmKey::of(s)).ok(),
        HEARD_S,
    );
    let mut stream = export.stream(CEILING_DBTP);
    let mut exported = vec![0f32; heard.len()];
    for block in exported.chunks_mut(2 * 1_024) {
        stream.next(block);
    }
    let worst = heard
        .iter()
        .zip(&exported)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    println!("largest sample difference, export vs preview: {worst:e}");
    assert!(worst < 1e-3, "export differs from the preview by {worst}");
    let (talking, between) = (rms_db(&exported, 6.0, 8.0), rms_db(&exported, 11.0, 13.0));
    assert!(
        (talking - between - DUCK_DB).abs() < 0.5,
        "export: {talking:.2} vs {between:.2} dB"
    );
}

/// The preview levels from an estimate of the mix's loudness and export from its exact measure (`audio::loudness`):
/// both hear the music go down under speech. 8 dB here, inside R128's relative gate (12 dB down is gated out of both).
#[test]
fn the_preview_levels_a_ducked_mix_as_export_measures_it() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("music.wav");
    write_tone(&wav, 30);
    let registry = Registry::builtin();
    let store = PcmStore::new(dir.path().join("cache"));
    // (the preview's estimate, the export's exact LUFS) of the bed ducking by `duck`.
    let levels = |duck: Option<f64>| {
        let mut p = music_alone(duck);
        p.assets.iter_mut().find(|a| a.id == "music").unwrap().path = wav.display().to_string();
        let plan = audio_plan(&p, &registry, speech());
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
        (estimate, measure(&plan, &pcm).unwrap())
    };
    let (plain_estimate, plain) = levels(None);
    let (estimate, exact) = levels(Some(-8.0));
    println!(
        "plain {plain:.3}/{plain_estimate:.3}, ducking 8 dB {exact:.3}/{estimate:.3} LUFS (export/preview)"
    );
    assert!(
        (plain_estimate - plain).abs() < 0.1,
        "{plain_estimate} vs {plain}"
    );
    assert!(
        (estimate - exact).abs() < 0.1,
        "preview {estimate} vs export {exact}"
    );
    assert!(
        plain - exact > 1.0,
        "ducking lowers the mix: {exact} vs {plain}"
    );
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
            // 200 ms later: the music stays down between them.
            (1.6, 2.0, WordKind::Word),
            // A pause word is a gap, not speech: a second without words lets the music back up.
            (2.0, 3.0, WordKind::Pause),
            (3.0, 3.5, WordKind::Filler),
            // 549 ms later: under the attack and release together (550 ms), so it stays down between sentences.
            (4.049, 4.4, WordKind::Word),
            // 550 ms later: the music lifts between them.
            (4.95, 5.2, WordKind::Word),
        ],
        60.0,
        true,
    );
    assert_eq!(JOIN_US, ATTACK_US + RELEASE_US);
    assert_eq!(
        &*speech_ranges(&p, &heard_in(&[("mic", words)], &[])),
        [range(1.0, 2.0), range(3.0, 4.4), range(4.95, 5.2)]
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

/// A dry run (a drag the engine previews) uses the committed speech again when it can't move it, and works it out anew
/// when it can, or when the committed project didn't duck.
#[test]
fn a_dry_run_reuses_the_speech_it_cannot_move() {
    let words = transcript(&[(1.0, 2.0, WordKind::Word)], 60.0, true);
    let said = heard_in(&[("mic", words)], &[]);
    let committed = with_music(Some(DUCK_DB));
    let known = ducking_speech(&committed, &said);
    assert_eq!(&*known, [range(1.0, 2.0)]);
    // Moving the music: the same ranges, not walked again.
    let mut moved = committed.clone();
    moved.timeline.tracks[0].items[0].start_us = secs(1.0);
    let again = ducking_speech_after(&moved, &said, &committed, &known);
    assert!(Arc::ptr_eq(&again, &known));
    // Cutting the first second of the take: the words move with it.
    let mut cut = committed.clone();
    cut.timeline.main[0].source_in_us = secs(1.0);
    let anew = ducking_speech_after(&cut, &said, &committed, &known);
    assert_eq!(&*anew, [range(0.0, 1.0)]);
    // Turning ducking on in the dry run of a project that didn't duck: worked out, not the committed (empty) ones.
    let plain = with_music(None);
    let none = ducking_speech(&plain, &said);
    assert!(none.is_empty());
    let on = ducking_speech_after(&committed, &said, &plain, &none);
    assert_eq!(&*on, [range(1.0, 2.0)]);
    // And nothing to work out once nothing ducks.
    assert!(ducking_speech_after(&plain, &said, &committed, &known).is_empty());
}

/// Fixing a pause into words makes it speech, so for a project that ducks a word fix changes the sound (a dry run
/// plans it again); for one that doesn't, a word fix is the captions' alone.
#[test]
fn word_fixes_move_speech_when_something_ducks() {
    let words = transcript(
        &[
            (1.0, 2.0, WordKind::Word),
            (2.0, 3.0, WordKind::Pause),
            (3.0, 4.0, WordKind::Word),
        ],
        60.0,
        true,
    );
    let said = heard_in(&[("mic", words)], &[]);
    let fix = Op::FixWords {
        asset_id: "mic".into(),
        start_us: secs(2.0),
        end_us: secs(3.0),
        text: Some("and so".into()),
    };
    let p = with_music(Some(DUCK_DB));
    let fixed = klipolo_model::apply_ops(&p, std::slice::from_ref(&fix)).unwrap();
    assert_eq!(
        &*speech_ranges(&p, &said),
        [range(1.0, 2.0), range(3.0, 4.0)]
    );
    assert_eq!(&*speech_ranges(&fixed, &said), [range(1.0, 4.0)]);
    assert!(may_change_sound(&p, &fix), "a project that ducks");
    assert!(
        !may_change_sound(&with_music(None), &fix),
        "one that doesn't"
    );
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

/// A preview that draws nothing and keeps every sound plan the engine gets ready to play (`prepare_audio`).
struct Prepared {
    frames: BlankBackend,
    plans: Arc<Mutex<Vec<Arc<AudioPlan>>>>,
}

impl PreviewBackend for Prepared {
    async fn render(&mut self, req: RenderRequest) -> Result<Rendered, String> {
        self.frames.render(req).await
    }
    fn reset(&mut self) {}
    fn prepare_audio(&mut self, plan: Arc<AudioPlan>, _at_us: Us) {
        self.plans.lock().unwrap().push(plan);
    }
}

/// The preview's side: the project feed sends the same revision again when a transcript finishes, with new speech; a
/// project that ducks has its sound planned again, now going down under the words, and the same words sent again keep
/// the plan it has.
#[tokio::test]
async fn the_preview_ducks_once_the_words_arrive() {
    let plans = Arc::new(Mutex::new(Vec::new()));
    let backend = Prepared {
        frames: BlankBackend::new(),
        plans: plans.clone(),
    };
    let (feed, rx) = watch::channel(None);
    let options = EngineOptions {
        allowed_origins: vec!["tauri://localhost".into()],
        max_preview_pixels: 921_600,
        socket_playback_pixels: 921_600,
        socket_rgba_only: false,
        adapter: "Test".into(),
        gpu_class: klipolo_engine::platform::gpu::GpuClass::Integrated,
    };
    let _engine = Engine::start(options, rx, backend).await.unwrap();
    let project = Arc::new(with_music(Some(DUCK_DB)));
    let send = |speech: &Arc<Speech>| {
        feed.send_replace(Some(ProjectFeed {
            project_id: "p".into(),
            revision: 0,
            opened: 1,
            project: project.clone(),
            speech: speech.clone(),
            text_language: "en".into(),
            registry: Registry::builtin(),
        }));
    };
    let bed = |plan: &Arc<AudioPlan>| {
        let bed = plan.sources.iter().find(|s| s.path.ends_with("music"));
        bed.unwrap().duck.clone()
    };
    // Nothing heard yet: the bed plays at its volume.
    send(&Arc::default());
    until(&plans, |p| !p.is_empty()).await;
    assert!(plans.lock().unwrap().iter().all(|p| bed(p).is_none()));
    // The transcript finishes: the same revision, new speech. The bed goes down under the words.
    let words = transcript(&[(1.0, 2.0, WordKind::Word)], 60.0, true);
    let said = Arc::new(heard_in(&[("mic", words)], &[]));
    send(&said);
    until(&plans, |p| p.last().is_some_and(|p| bed(p).is_some())).await;
    let ducked = plans.lock().unwrap().last().unwrap().clone();
    let duck = bed(&ducked).unwrap();
    assert_eq!((duck.db, &*duck.speech), (-12.0, &[range(1.0, 2.0)][..]));
    // The same words again (a new copy): the plan it has is kept.
    let n = plans.lock().unwrap().len();
    send(&Arc::new((*said).clone()));
    until(&plans, |p| p.len() > n).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        plans.lock().unwrap()[n..]
            .iter()
            .all(|p| Arc::ptr_eq(p, &ducked))
    );
}

/// Waits (up to 2 s) until the plans the preview prepared satisfy `ok`.
async fn until(plans: &Mutex<Vec<Arc<AudioPlan>>>, ok: impl Fn(&[Arc<AudioPlan>]) -> bool) {
    for _ in 0..200 {
        if ok(&plans.lock().unwrap()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the sound plans never got there");
}

/// A 16-bit stereo WAV of the tone, `seconds` long.
fn write_tone(path: &Path, seconds: usize) {
    let frames = seconds * RATE as usize;
    let data = (frames * 4) as u32;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for part in [
        &b"RIFF"[..],
        &(36 + data).to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &2u16.to_le_bytes(),
        &RATE.to_le_bytes(),
        &(RATE * 4).to_le_bytes(),
        &4u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ] {
        out.write_all(part).unwrap();
    }
    for n in 0..frames {
        let x = ((tone_at(n) * 32767.0).round() as i16).to_le_bytes();
        out.write_all(&x).unwrap();
        out.write_all(&x).unwrap();
    }
}
