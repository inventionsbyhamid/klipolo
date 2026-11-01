//! How loud a recorded sound file is on each of its channels, so the clip card's Centre switch plays the side of a
//! one-sided recording that carries the sound, and never guesses one. The first that knows:
//! 1. the sound cache's peaks (`peaks::loudest`): the whole file, once its waveform is built;
//! 2. the take's manifest (`ManifestRun::channel_peaks`, the recorder's own measure of each run), for a recording
//!    Klipolo made;
//! 3. a bounded read of the file itself (`decode::loudest`): its first `READ_FRAMES`, nothing written or kept. A read
//!    that is silent on every side says nothing.
//!
//! A file none of them knows is left out: the switch then keeps the side the sound already plays from. Blocking: one
//! header and the coarsest level of its peaks (about 84 KB for an hour of stereo), or one manifest read, or decoding
//! at most two minutes of sound a block at a time.

use std::path::Path;

use klipolo_capture::manifest::read_manifest;
use klipolo_capture::session::SessionDir;
use klipolo_engine::audio::decode;
use klipolo_engine::audio::mix::RATE;
use klipolo_engine::audio::pcm::{peaks, peaks_file};
use klipolo_model::project::Project;

/// A file read now is read for its first two minutes.
const READ_FRAMES: u64 = 120 * RATE as u64;
/// A read whose every channel stays at or under -60 dBFS doesn't say which side carries the sound.
const SILENT: f32 = 0.001;

/// The loudest sample of each of `path`'s channels, 0 to 1, found as the module says; None when nothing knows.
/// `cache_dir`: the sound cache's folder (None: no cache to read).
pub fn of_file(project: &Project, cache_dir: Option<&Path>, path: &str) -> Option<Vec<f32>> {
    let cached = cache_dir
        .and_then(|dir| peaks_file(dir, path))
        .and_then(|file| peaks::loudest(&file));
    if cached.is_some() {
        return cached;
    }
    let file = Path::new(path);
    let asset = project.assets.iter().find(|a| a.path == path)?;
    let recording = project.recordings.iter().find(|r| {
        (r.sources.iter())
            .flat_map(|s| &s.runs)
            .any(|run| run.asset_id == asset.id)
    });
    recording
        .and_then(|r| take_peaks(&r.id, file))
        .or_else(|| read(file))
}

/// The take's own measure of the run file at `path` (its manifest's `channel_peaks`), when `path` is a run of the take
/// recording `id` came from (import keeps the take's id as the recording's).
fn take_peaks(id: &str, path: &Path) -> Option<Vec<f32>> {
    let (dir, key) = SessionDir::of_run(path)?;
    let manifest = read_manifest(&dir).ok().filter(|m| m.session == id)?;
    (manifest.sources.iter())
        .flat_map(|s| &s.runs)
        .find(|run| run.file == key)?
        .channel_peaks
        .clone()
}

/// A bounded read of `path` (its first `READ_FRAMES`); a read silent on every side of two says nothing.
fn read(path: &Path) -> Option<Vec<f32>> {
    decode::loudest(path, READ_FRAMES).filter(|p| p.len() < 2 || p.iter().any(|&x| x > SILENT))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use klipolo_capture::manifest::{Manifest, ManifestRun, ManifestSource, write_manifest};
    use klipolo_capture::session::{ImportTarget, SessionDir};
    use klipolo_engine::audio::mix::RATE;
    use klipolo_engine::audio::pcm::peaks::Peaks;
    use klipolo_engine::media::key::{CacheKind, stem};
    use klipolo_model::project::{
        Asset, AssetKind, Recording, RecordingKind, RecordingSource, SourceMix, SourceRole,
        SourceRun,
    };

    use super::*;

    fn sound(id: &str, path: &str) -> Asset {
        Asset {
            id: id.into(),
            kind: AssetKind::Audio,
            path: path.into(),
            name: id.into(),
            duration_us: 1_000_000,
            width: 0,
            height: 0,
            fps: 0.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
            made_with: None,
        }
    }

    fn source(id: &str, role: SourceRole, asset_id: &str) -> RecordingSource {
        RecordingSource {
            id: id.into(),
            role,
            label: format!("capture.source.{id}"),
            runs: vec![SourceRun {
                asset_id: asset_id.into(),
                offset_us: 0,
                duration_us: 1_000_000,
                drift_ppm: 0.0,
                file_in_us: 0,
            }],
            nudge_us: 0,
            mix: SourceMix::default(),
            synced: None,
        }
    }

    /// A take `r1` whose mic is the file at `mic`, and a plain sound file at `other` that no take has.
    fn project(mic: &str, other: &str) -> Project {
        let mut p = Project::new("Takes");
        p.assets = vec![sound("m1", mic), sound("other", other)];
        p.recordings = vec![Recording {
            id: "r1".into(),
            kind: RecordingKind::Screen,
            sources: vec![source("mic", SourceRole::Mic, "m1")],
            primary: "mic".into(),
            cursor: None,
            markers: vec![],
            game: None,
        }];
        p
    }

    /// One second of 16-bit stereo at 48 kHz, each frame `[left, right]`.
    fn wav(path: &Path, left: i16, right: i16) {
        let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let data = RATE * 4;
        let header: [&[u8]; 12] = [
            b"RIFF",
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
        ];
        header.iter().for_each(|part| out.write_all(part).unwrap());
        for _ in 0..RATE {
            out.write_all(&left.to_le_bytes()).unwrap();
            out.write_all(&right.to_le_bytes()).unwrap();
        }
    }

    #[test]
    fn a_sound_file_s_peaks_come_from_its_sound_cache_by_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let mic = dir.path().join("mic.caf");
        std::fs::write(&mic, b"a mic recording").unwrap();
        let mic = mic.to_string_lossy().into_owned();
        let p = project(&mic, "/nowhere/other.wav");
        let cache = dir.path().join("audio");
        std::fs::create_dir_all(&cache).unwrap();
        let file = cache.join(format!("{}.peaks", stem(&mic, &CacheKind::Pcm).unwrap()));
        let mut peaks = Peaks::create(&file, 2).unwrap();
        peaks.push(0, &[0.0, 0.5].repeat(48_000)).unwrap();
        peaks.end_run(true).unwrap();
        peaks.finish(48_000).unwrap();
        let known = of_file(&p, Some(&cache), &mic).expect("from the cache");
        assert!(
            known[0] < 0.001 && (known[1] - 0.5).abs() < 1e-3,
            "{known:?}"
        );
        assert_eq!(
            of_file(&p, Some(&cache), "/nowhere/other.wav"),
            None,
            "nothing knows it"
        );
    }

    /// With no sound cache, a take's run is known from its manifest, and any other sound file from a bounded read of
    /// the file; a file that can't be read, or another take's manifest, never answers.
    #[test]
    fn without_its_cache_a_sound_is_known_from_its_take_s_manifest_else_from_the_file() {
        let dir = tempfile::tempdir().unwrap();
        // The mic is a run of take r1, whose manifest measured [silence, voice]. Its file holds no sound: only the
        // manifest can say.
        let take = SessionDir::create(dir.path(), "r1").unwrap();
        let mic = take.path("mic/run-01.caf");
        std::fs::create_dir_all(mic.parent().unwrap()).unwrap();
        std::fs::write(&mic, b"not sound").unwrap();
        let manifest = Manifest {
            session: "r1".into(),
            kind: RecordingKind::Screen,
            duration_us: 1_000_000,
            sources: vec![ManifestSource {
                id: "mic".into(),
                role: SourceRole::Mic,
                label: "capture.source.mic".into(),
                runs: vec![ManifestRun {
                    file: "mic/run-01.caf".into(),
                    offset_us: 0,
                    duration_us: 1_000_000,
                    drift_ppm: 0.0,
                    approximate: false,
                    channel_peaks: Some(vec![0.0, 0.6]),
                }],
            }],
            markers: vec![],
            cursor: None,
            game: None,
            notes: vec![],
            recovered: false,
            target: ImportTarget::New,
        };
        write_manifest(&take, &manifest).unwrap();
        // A plain file: a second of [0.5, silence], read now.
        let other = dir.path().join("other.wav");
        wav(&other, 0x4000, 0);
        let (mic, other) = (
            mic.to_string_lossy().into_owned(),
            other.to_string_lossy().into_owned(),
        );
        let mut p = project(&mic, &other);
        assert_eq!(
            of_file(&p, None, &mic),
            Some(vec![0.0, 0.6]),
            "the take's own measure"
        );
        let read = of_file(&p, None, &other).expect("read from the file");
        assert!((read[0] - 0.5).abs() < 0.01 && read[1] < 0.001, "{read:?}");
        // Another take's manifest never answers: the file is read instead, which holds no sound, so it stays unknown.
        p.recordings[0].id = "r9".into();
        assert_eq!(of_file(&p, None, &mic), None, "never guessed");
        // A silent stereo file says nothing either.
        let silent = dir.path().join("silent.wav");
        wav(&silent, 0, 0);
        p.assets[1].path = silent.to_string_lossy().into_owned();
        assert_eq!(of_file(&p, None, &p.assets[1].path.clone()), None);
    }
}
