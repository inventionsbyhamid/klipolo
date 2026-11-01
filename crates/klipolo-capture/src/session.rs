//! A take's folder: `<recordings root>/<id>.klipolorec/`, made once and never reused.
//!
//! - `session.json`: the fixed config, written atomically before capture (`write_config`).
//! - `session.lock`: held by the live recorder for the whole take (`SessionLock`); recovery starts only once it can
//!   take it.
//! - `journal.jsonl`: the append-only source of truth (`journal`).
//! - `timing.jsonl`: the audio runs' anchors, once a second (`journal::anchor_line`).
//! - `manifest.json`: derived from the journal and the files, never the only record (`manifest`).
//! - `cursor.jsonl` (`klipolo_cursor::format`), its shapes in `cursors/`, and `diagnostics.log`.
//! - `imported`: an empty marker once the app has imported the take (`mark_imported`).
//! - One file per run of each source: `screen/run-01.mp4`, `mic/run-02.caf` (`SessionDir::run_key`).

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use klipolo_model::project::{FormatKind, GameInfo, RecordingKind, SourceRole};
use klipolo_platform::durable::{Durability, fsync_dir, write_atomic};
use klipolo_platform::process::lock_exclusive;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::backend::{EncoderPick, PresetId, Target, VideoSpec};
use crate::media::SourceId;
use crate::profile::Corner;
use crate::shell::KeyBindings;

/// A take's folder name ends in `.klipolorec`.
pub const EXTENSION: &str = "klipolorec";

/// The cursor file's key (`klipolo_cursor::format`).
pub const CURSOR: &str = "cursor.jsonl";
/// The folder of the cursor's shapes, `cursors/<id>.png` (`cursor::ShapeStore`).
pub const CURSORS_DIR: &str = "cursors";

/// A path relative to the session folder, with `/` between its parts on every OS: "mic/run-01.caf".
pub type FileKey = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionDir {
    pub root: PathBuf,
}

impl SessionDir {
    /// Makes `<recordings_root>/<id>.klipolorec` (and the root if needed). An existing take is never reused:
    /// `AlreadyExists`. `id` must be a plain name (`is_plain_name`): `InvalidInput`. The new folder's entry is made
    /// durable (Linux syncs the root).
    pub fn create(recordings_root: &Path, id: &str) -> io::Result<Self> {
        if !is_plain_name(id) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("a take's id must be a plain name: {id:?}"),
            ));
        }
        std::fs::create_dir_all(recordings_root)?;
        let root = recordings_root.join(format!("{id}.{EXTENSION}"));
        std::fs::create_dir(&root)?;
        fsync_dir(recordings_root)?;
        Ok(Self { root })
    }

    /// An existing take's folder.
    pub fn at(path: &Path) -> Self {
        Self {
            root: path.to_path_buf(),
        }
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("session.json")
    }

    pub fn lock(&self) -> PathBuf {
        self.root.join("session.lock")
    }

    pub fn journal(&self) -> PathBuf {
        self.root.join("journal.jsonl")
    }

    pub fn timing(&self) -> PathBuf {
        self.root.join("timing.jsonl")
    }

    pub fn manifest(&self) -> PathBuf {
        self.root.join("manifest.json")
    }

    pub fn cursor(&self) -> PathBuf {
        self.root.join(CURSOR)
    }

    pub fn diagnostics(&self) -> PathBuf {
        self.root.join("diagnostics.log")
    }

    /// The `imported` marker (`mark_imported`): Home's scan skips the take by it alone.
    pub fn imported_mark(&self) -> PathBuf {
        self.root.join("imported")
    }

    /// Run `run` of `source`: "screen/run-01.mp4" for video, "mic/run-02.caf" for sound.
    pub fn run_key(source: &str, run: u32, video: bool) -> FileKey {
        let ext = if video { "mp4" } else { "caf" };
        format!("{source}/run-{run:02}.{ext}")
    }

    /// The take a run file is in, and the file's key: `<id>.klipolorec/mic/run-01.caf` gives the take's folder and
    /// "mic/run-01.caf" (`run_key`'s layout). None for any other path. Reads nothing.
    pub fn of_run(path: &Path) -> Option<(Self, FileKey)> {
        let file = path.file_name()?.to_str()?;
        let folder = path.parent()?;
        let source = folder.file_name()?.to_str()?;
        let root = folder.parent()?;
        (root.extension()? == EXTENSION && is_plain_name(source) && is_plain_name(file))
            .then(|| (Self::at(root), format!("{source}/{file}")))
    }

    /// The file `key` names, with this OS's separators. For keys this recorder made; a key read back from the journal
    /// goes through `key_path`.
    pub fn path(&self, key: &str) -> PathBuf {
        key.split('/')
            .fold(self.root.clone(), |path, part| path.join(part))
    }

    /// `path`, for a key that may not be ours (the journal's, in recovery): InvalidInput unless every part is a plain
    /// name, so a key can never reach outside the take's folder (`..`, an absolute path, or a Windows `C:` that would
    /// replace the root on join).
    pub fn key_path(&self, key: &str) -> io::Result<PathBuf> {
        if !key.split('/').all(is_plain_name) {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("a file key must be plain names joined by '/': {key:?}"),
            ));
        }
        Ok(self.path(key))
    }
}

/// A name that stays inside the folder it is joined to: not empty, not `.` or `..`, and without `/`, `\`, `:` (a
/// Windows drive or stream) or NUL.
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
}

/// The take as the person set it up, fixed before capture. Recovery and the resumed recorder read it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConfig {
    pub id: String,
    /// When the take was set up, ms since 1970.
    pub created_ms: u64,
    pub kind: RecordingKind,
    pub sources: Vec<SourceConfig>,
    pub countdown_s: u32,
    /// The game's app id (`GameInfo::app_id`): pause while it isn't in front.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_pause: Option<String>,
    pub target: ImportTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<GameInfo>,
    pub app_version: String,
    pub os: String,
    /// How long the person expects to record (the disk outlook).
    pub expected_minutes: u32,
    /// `capture.*` code → the UI's wording in the person's language, for the native menus and HUD.
    pub words: BTreeMap<String, String>,
    pub keys: KeyBindings,
    /// Where the HUD shows: a game's profile's corner (`GameProfile::hud_corner`), else top right.
    #[serde(default)]
    pub hud_corner: Corner,
    /// "Show my camera while recording": a small round live picture of the take's camera on the recorded
    /// screen, never in the video. Only a whole-screen, window or app take with a camera has it (the app's rule).
    #[serde(default)]
    pub camera_bubble: bool,
    /// "Mirror my camera": the bubble shows the camera as in a mirror, and the take's camera layer is
    /// mirrored when it is imported. The recorded file never is. A take from before it reads as on, the default.
    #[serde(default = "mirrored")]
    pub mirror_camera: bool,
}

fn mirrored() -> bool {
    true
}

impl SessionConfig {
    /// A source's label (its `capture.source.*` code, or the person's own name), else its id.
    pub fn label_of<'a>(&'a self, source: &'a str) -> &'a str {
        (self.sources.iter())
            .find(|s| s.id == source)
            .map_or(source, |s| s.label.as_str())
    }

    /// The display the take's screen, app or game is recorded on, by its id (the HUD goes there). None for a window
    /// (it can be on any display), the system's picker or a take without screen video.
    pub fn captured_display(&self) -> Option<&str> {
        self.sources.iter().find_map(|s| match &s.spec {
            SourceSpec::Screen(v) => match &v.target {
                Target::Display { id } | Target::DisplayWithoutKlipolo { id } => Some(id.as_str()),
                Target::App { display, .. } => Some(display.as_str()),
                Target::Window { .. } | Target::Camera { .. } | Target::Portal { .. } => None,
            },
            _ => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceConfig {
    /// Also the source's folder name.
    pub id: SourceId,
    pub role: SourceRole,
    /// A `capture.source.*` code, or the person's own name (as `RecordingSource::label`).
    pub label: String,
    pub spec: SourceSpec,
}

/// What a source captures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SourceSpec {
    /// A display, an app's or a game's windows, or a window (`VideoSpec::target`).
    Screen(VideoSpec),
    Camera {
        device: String,
        preset: PresetId,
        /// The encoders to record with, tried in order (`probe::camera_encoders_for`, as `VideoSpec::encoders` for a
        /// screen). Empty: the backend's own pick.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        encoders: Vec<EncoderPick>,
    },
    Mic {
        device: String,
    },
    /// The whole computer's sound, from its own pipe.
    System,
    /// The sound that comes with the video source `of` (`VideoPipes::sound`): the game's or the app's.
    AppSound {
        of: SourceId,
    },
}

/// Where the take goes after Stop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ImportTarget {
    /// A take started from Home: a new project in these formats.
    NewProject {
        name: String,
        formats: Vec<FormatKind>,
    },
    /// Appended to this project's main track.
    Project { id: String },
    /// A take started with no project open: a new project, which the app names and makes only when the take lands, so
    /// a cancelled take leaves nothing behind.
    New,
}

/// Writes `session.json` atomically and fully to the disk: a crash leaves no config or the whole one.
pub fn write_config(dir: &SessionDir, cfg: &SessionConfig) -> io::Result<()> {
    write_atomic(
        &dir.config(),
        &serde_json::to_vec_pretty(cfg)?,
        Durability::Full,
    )
}

/// Marks the take imported (an empty `imported` file, after the journal's `Imported` line), so Home's scan skips it
/// without reading its config, its lock or its journal.
pub fn mark_imported(dir: &SessionDir) -> io::Result<()> {
    write_atomic(&dir.imported_mark(), b"", Durability::Ordered)
}

/// `session.json`; NotFound without one, InvalidData when it isn't a config.
pub fn read_config(dir: &SessionDir) -> io::Result<SessionConfig> {
    serde_json::from_slice(&std::fs::read(dir.config())?)
        .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

/// The recorder's hold on its take, released when this drops or its process ends however it ends.
///
/// - **One writer.** A second recorder, or recovery, can't take it while the live one holds it.
/// - **A crashed recorder's lock is never stale.** The OS lock (`flock` on macOS and Linux, `LockFileEx` on Windows,
///   through `klipolo_platform::process::lock_exclusive`) lives with the process, not on the disk. After a crash, a
///   `kill -9` or a power cut, `session.lock` is a plain file, and a later `try_acquire` takes it over. On macOS and
///   Linux that is at once; Windows releases a killed process's locks "at a time that depends on available system
///   resources", usually at once but not always, so the retry below covers a resume after a crash too.
/// - **Never delete `session.lock`**, held or not: a second opener would lock a new file of the same name while the
///   first still holds the old one.
/// - **Per open file.** Two opens exclude each other even in one process. A forked child holds a copy until it execs
///   (std opens with close-on-exec), so a holder that spawns keeps it held for that instant.
/// - **Retry before calling it live.** Recovery's scan takes the lock only to check, so a recorder that starts at that
///   instant sees None, and so may a resume just after a crash on Windows (above). Callers that must own the take
///   retry for about 500 ms before they report it live.
#[derive(Debug)]
pub struct SessionLock(
    #[expect(dead_code, reason = "held for its lock, which closing it releases")] File,
);

impl SessionLock {
    /// None: another process (or another open in this one) holds it.
    pub fn try_acquire(dir: &SessionDir) -> io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.lock())?;
        match lock_exclusive(&file) {
            Ok(()) => Ok(Some(Self(file))),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}
