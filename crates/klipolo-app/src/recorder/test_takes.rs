//! Takes and project sessions for the landing tests. A take's files are real media, as a saved take leaves them: the
//! capture crate's H.264 fixture remuxed at 30 fps for each picture, a sized CAF of a tone for the mic, and a cursor
//! file; or, for a take whose worker stopped while saving, the same runs cut off before they were finished. The app's
//! own media probe reads them. Nothing captures.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use klipolo_capture::backend::{CursorCaps, PresetId};
use klipolo_capture::caf;
use klipolo_capture::fmp4::{Fmp4Writer, finalize};
use klipolo_capture::io::OsIo;
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::manifest::{
    Manifest, ManifestCursor, ManifestRun, ManifestSource, write_manifest,
};
use klipolo_capture::media::AudioFormat;
use klipolo_capture::session::{self, ImportTarget, SessionConfig, SessionDir, read_config};
use klipolo_capture::synthetic::fixture::FixtureVideo;
use klipolo_model::coded::{CodedError, Said};
use klipolo_model::cursor_style::{ClickSource, CursorCapture};
use klipolo_model::history::HistoryState;
use klipolo_model::project::{Asset, AssetKind, Project, SourceRole};
use klipolo_model::timeline::{Clip, ClipSource};

use super::config::{TakeChoices, session_config};
use super::destination::Day;
use super::env::Done;
use super::import::{Landing, land};
use super::setup::Files;
use super::test_support::{Lands, options_screen, options_yourself};
use super::types::{ArmOptions, TakeImport};
use crate::project::{
    PROJECT_EXTENSION, ProjectSessionState, create_in, snapshot_now, step, with_open,
};

/// A take's length.
pub(super) const TAKE_US: i64 = 2_000_000;
/// The fixed clock's day: a take from the start screen is "Recording, Oct 31".
pub(super) const OCT_31: Day = Day { month: 10, day: 31 };

/// The capture crate's media fixtures.
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../klipolo-capture/tests/fixtures")
}

/// A project session over temporary folders: the library new projects go into, and a folder of its own for the
/// project opened from elsewhere.
pub(super) struct Sessions {
    pub(super) state: ProjectSessionState,
    library: tempfile::TempDir,
    elsewhere: tempfile::TempDir,
}

impl Sessions {
    /// The start screen: nothing open.
    pub(super) fn start_screen() -> Self {
        Self {
            state: ProjectSessionState::default(),
            library: tempfile::tempdir().unwrap(),
            elsewhere: tempfile::tempdir().unwrap(),
        }
    }

    /// "Edit", with one clip, open (its file outside the library).
    pub(super) fn with_project() -> Self {
        let sessions = Self::start_screen();
        sessions.open_new("Edit");
        sessions
    }

    /// A new project `name` with one clip, made outside the library and opened: its id and file.
    pub(super) fn open_new(&self, name: &str) -> (String, PathBuf) {
        let mut project = Project::new(name);
        let asset = Asset {
            id: "footage".into(),
            kind: AssetKind::Video,
            path: fixtures()
                .join("gop-h264.mp4")
                .to_string_lossy()
                .into_owned(),
            name: "gop-h264.mp4".into(),
            duration_us: 1_000_000,
            width: 1280,
            height: 720,
            fps: 30.0,
            rotation: None,
            has_audio: false,
            audio_lead_us: None,
            made_with: None,
        };
        let clip = Clip::new(
            ClipSource::Asset {
                id: asset.id.clone(),
            },
            0,
            1_000_000,
        );
        project.assets.push(asset);
        project.timeline.main.push(clip);
        let id = project.id.clone();
        create_in(&self.state, self.elsewhere.path(), project).unwrap();
        let file = self.open_file().unwrap();
        (id, file)
    }

    /// The open project.
    pub(super) fn project(&self) -> Project {
        snapshot_now(&self.state)
            .expect("a project is open")
            .project
    }

    /// The open project's file.
    pub(super) fn open_file(&self) -> Option<PathBuf> {
        with_open(&self.state, |open| Ok(open.file.clone())).ok()
    }

    pub(super) fn history(&self) -> HistoryState {
        snapshot_now(&self.state)
            .expect("a project is open")
            .history
    }

    /// One Undo.
    pub(super) fn undo(&self) {
        with_open(&self.state, |open| step(open, false)).unwrap();
    }

    /// The library takes no new project (a file stands where its folder was), or takes them again.
    pub(super) fn library_refuses(&self, refuses: bool) {
        let path = self.library.path();
        if refuses {
            std::fs::remove_dir_all(path).unwrap();
            std::fs::write(path, b"").unwrap();
        } else {
            std::fs::remove_file(path).unwrap();
            std::fs::create_dir(path).unwrap();
        }
    }

    /// The library's project files, by name.
    pub(super) fn library_files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.library.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(&format!(".{PROJECT_EXTENSION}")))
            .collect();
        names.sort();
        names
    }

    /// Lands `done` as the app's sink does, on the fixed clock, new projects in the library, probed by the app's own
    /// probe.
    pub(super) fn land(
        &self,
        done: &Done,
        files: Option<&Files>,
    ) -> Result<TakeImport, CodedError> {
        let library = self.library.path().to_path_buf();
        let library = move || -> Result<PathBuf, Said> { Ok(library.clone()) };
        let mut probe = |path: &Path, _key: &str| crate::media::probe_asset(path);
        let landing = Landing {
            library: &library,
            today: OCT_31,
            files,
            probe: &mut probe,
        };
        land(&self.state, done, landing)
    }
}

/// A recorder's sink lands each finished take into `sessions`, keeping what each landing came to.
pub(super) fn landing_into(
    sessions: Arc<Sessions>,
    landed: Arc<Mutex<Vec<Result<TakeImport, CodedError>>>>,
) -> Lands {
    Box::new(move |done: &Done| {
        let result = sessions.land(done, None);
        landed.lock().unwrap().push(result.clone());
        result.map(|_| ())
    })
}

/// What a take recorded.
#[derive(Clone, Copy, Debug)]
pub(super) struct Recorded {
    /// The camera, and "Mirror my camera" for it.
    pub(super) camera: Option<bool>,
    /// The screen's picture file (false: a take with nothing to edit).
    pub(super) picture: bool,
    /// The cursor file, with the clicks as events.
    pub(super) cursor: bool,
    /// How long it ran.
    pub(super) length_us: i64,
    /// "Record yourself": the camera (`camera`, else mirrored) and the mic alone, no screen.
    pub(super) yourself: bool,
}

impl Default for Recorded {
    fn default() -> Self {
        Self {
            camera: None,
            picture: true,
            cursor: false,
            length_us: TAKE_US,
            yourself: false,
        }
    }
}

/// A take's folder in a temporary recordings folder, made as arming makes it (`session.json`), landing in `target`.
pub(super) struct TakeFixture {
    pub(super) dir: SessionDir,
    pub(super) cfg: SessionConfig,
    _root: tempfile::TempDir,
}

impl TakeFixture {
    /// Take "take-1", set up as `recorded` says, its files not written yet.
    pub(super) fn new(recorded: Recorded, target: ImportTarget) -> Self {
        let root = tempfile::tempdir().unwrap();
        let (dir, cfg) = take_in(root.path(), "take-1", recorded, target, 0);
        Self {
            dir,
            cfg,
            _root: root,
        }
    }

    /// A saved take: its files, manifest and `Finalized`.
    pub(super) fn saved(recorded: Recorded, target: ImportTarget) -> Self {
        let take = Self::new(recorded, target);
        write_saved(&take.dir, recorded);
        take
    }

    /// The take as the recorder hands it on to land (`Done`), armed in the project at `project_file`.
    pub(super) fn done(&self, recover: bool, project_file: Option<PathBuf>) -> Done {
        Done {
            id: self.cfg.id.clone(),
            dir: self.dir.clone(),
            target: self.cfg.target.clone(),
            project_file,
            recover,
            note: None,
            earlier_run: false,
        }
    }
}

/// Take `id` in the recordings folder `root`, set up at `created_ms` as `recorded` says, landing in `target`: its
/// folder and `session.json`, its files not written yet.
pub(super) fn take_in(
    root: &Path,
    id: &str,
    recorded: Recorded,
    target: ImportTarget,
    created_ms: u64,
) -> (SessionDir, SessionConfig) {
    let mut options: ArmOptions = options_screen();
    if let Some(mirror) = recorded.camera {
        options.camera = Some("camera-1".into());
        options.mirror_camera = mirror;
    }
    if recorded.yourself {
        options = options_yourself();
        options.mirror_camera = recorded.camera.unwrap_or(true);
    }
    let choices = TakeChoices {
        preset: PresetId::default(),
        camera_preset: PresetId::Camera720,
        cursor_caps: CursorCaps::none(),
        created_ms,
        os: "test".into(),
        target,
        encoders: Vec::new(),
        camera_encoders: Vec::new(),
    };
    let cfg = session_config(&options, id, &choices);
    let dir = SessionDir::create(root, &cfg.id).unwrap();
    session::write_config(&dir, &cfg).unwrap();
    (dir, cfg)
}

/// The run file of `source` (the take's first run).
fn run_key(role: SourceRole, source: &str) -> String {
    SessionDir::run_key(source, 1, role.is_video())
}

/// What a saved take holds, for the take whose `session.json` is in `dir`: each source's run, the cursor file when
/// `recorded` says, the manifest, and `Finalized` journaled.
pub(super) fn write_saved(dir: &SessionDir, recorded: Recorded) {
    let cfg = read_config(dir).unwrap();
    let mut sources = Vec::new();
    for source in &cfg.sources {
        let key = run_key(source.role, &source.id);
        match source.role {
            SourceRole::Screen if !recorded.picture => continue,
            role if role.is_video() => write_picture(&dir.path(&key), true, recorded.length_us),
            _ => write_sound(&dir.path(&key), true, recorded.length_us),
        }
        let peaks = (!source.role.is_video()).then(|| vec![0.25]);
        sources.push(ManifestSource {
            id: source.id.clone(),
            role: source.role,
            label: source.label.clone(),
            runs: vec![ManifestRun {
                file: key,
                offset_us: 0,
                duration_us: recorded.length_us,
                drift_ppm: 0.0,
                approximate: false,
                channel_peaks: peaks,
            }],
        });
    }
    let cursor = recorded.cursor.then(|| {
        std::fs::write(dir.cursor(), b"").unwrap();
        ManifestCursor {
            file: session::CURSOR.into(),
            capture: CursorCapture::Separate,
            clicks: ClickSource::Events,
        }
    });
    let manifest = Manifest {
        session: cfg.id.clone(),
        kind: cfg.kind,
        duration_us: recorded.length_us,
        sources,
        markers: Vec::new(),
        cursor,
        game: None,
        notes: Vec::new(),
        recovered: false,
        target: cfg.target.clone(),
    };
    write_manifest(dir, &manifest).unwrap();
    append(
        dir,
        &Entry::Finalized {
            duration_us: recorded.length_us,
        },
    );
}

/// What a take whose worker stopped while saving holds: each source's run as it was being written (the picture's
/// fragments without the remux's tail, the sound's CAF without its size), no manifest, no `Finalized`.
pub(super) fn write_cut_off(dir: &SessionDir) {
    let cfg = read_config(dir).unwrap();
    for source in &cfg.sources {
        let key = run_key(source.role, &source.id);
        if source.role.is_video() {
            write_picture(&dir.path(&key), false, TAKE_US);
        } else {
            write_sound(&dir.path(&key), false, TAKE_US);
        }
    }
}

/// One line at the end of the take's journal.
pub(super) fn append(dir: &SessionDir, entry: &Entry) {
    let mut journal = journal::open_append(&dir.journal()).unwrap();
    journal.write_all(&journal::line(entry)).unwrap();
}

/// The fixture's H.264 frames for `length_us` at 30 fps, fragmented as the recorder writes them; `finished`: then
/// soft-remuxed into a regular MP4, as a saved run is.
fn write_picture(path: &Path, finished: bool, length_us: i64) {
    const FPS: u32 = 30;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let video = FixtureVideo::load(&fixtures().join("gop-h264.mp4"));
    let mut mux = Fmp4Writer::at_rate(&video.info, FPS);
    let mut bytes = mux.head();
    let frames = u64::from(FPS) * length_us as u64 / 1_000_000;
    for (i, packet) in video.packets(frames, 0, FPS).iter().enumerate() {
        let pts = (i as u64 * 1_000_000 / u64::from(FPS)) as i64;
        if let Some(fragment) = mux.push(pts, packet) {
            bytes.extend(fragment.bytes);
        }
    }
    bytes.extend(mux.flush().unwrap().bytes);
    std::fs::write(path, &bytes).unwrap();
    if !finished {
        return;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let plan = mux.remux_plan().unwrap();
    finalize::write_tail(&OsIo, &file, path, &plan).unwrap();
    finalize::flip(&OsIo, &file, path, &plan).unwrap();
}

/// A mono 440 Hz tone at -12 dBFS for `length_us` as a CAF; `finished`: with its size written, as a saved run is.
fn write_sound(path: &Path, finished: bool, length_us: i64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let format = AudioFormat {
        rate: 48_000,
        channels: 1,
    };
    let frames = u64::from(format.rate) * length_us as u64 / 1_000_000;
    let samples: Vec<f32> = (0..frames)
        .map(|i| 0.25 * (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin())
        .collect();
    let mut bytes = caf::header(format);
    caf::encode_into(&samples, &mut bytes);
    std::fs::write(path, &bytes).unwrap();
    if !finished {
        return;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let scan = caf::scan(&mut file, bytes.len() as u64).unwrap();
    caf::finalize(&OsIo, &file, path, &scan, frames).unwrap();
}
