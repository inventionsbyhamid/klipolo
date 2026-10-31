//! A finished take lands in a project: one step "Add recording" (`undo.record`: `[AddAssets, AddRecording,
//! InsertClip { index: None }]`) through the path every edit takes (`project::build_and_apply`), so one Undo takes it
//! back, the engine has it at once and it is saved. Its files stay in the take's folder: every asset points there.
//!
//! 1. A take the worker didn't finish (it stopped while saving) is finished from its files first (`recover::repair`).
//! 2. `recording_from` makes the recording, each run's file probed (`probe`) and a one-sided mic centred from the
//!    manifest's peaks. A take with nothing to edit stops here, with no project made, opened or changed.
//! 3. The project is opened (`destination`). The recording's id is the take's, so a take already in it isn't added
//!    again. A project made for the take goes again when the step is refused.
//! 4. The clip is the whole take, last on the main track, its camera mirrored as "Mirror my camera" was (`clip_for`).
//! 5. `Imported { project }` goes to the take's journal and its `imported` mark is written.
//!
//! Then its clicks get their auto zoom as a second step, "Add auto zoom" (`zoom`), unless the take was in the project
//! already: zooms are never added twice.
//!
//! The person is told how the take ended when nothing said it already (`Done.note`), then the recorder's notes, then the
//! import's own (`recording_from` leaves out its repeats).

use std::io::Write;
use std::path::{Path, PathBuf};

use klipolo_capture::import::{clip_for, manifest_peaks, recording_from};
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::manifest::{Manifest, read_manifest};
use klipolo_capture::recover::{self, RecoverError};
use klipolo_capture::session::{self, SessionDir, read_config};
use klipolo_model::Op;
use klipolo_model::coded::{CodedError, Said};
use klipolo_model::history::Snapshot;
use klipolo_model::project::{Asset, Recording};
use klipolo_model::timeline::ClipSource;
use klipolo_platform::durable;

use super::destination::{self, Day, Destination, Target};
use super::env::Done;
use super::setup::{self, Files};
use super::types::TakeImport;
use super::zoom::zoom_take;
use crate::project::{ProjectSessionState, build_and_apply, open_in, snapshot_now};

/// The step's label: "Add recording".
pub(super) const LABEL: &str = "undo.record";

/// A take in its project.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Landed {
    /// The project's id.
    pub(super) project: String,
    /// Where the project is saved.
    pub(super) file: PathBuf,
    pub(super) recording_id: String,
    /// Its clip on the main track.
    pub(super) clip_id: String,
    /// Added now (false: the take was in its project already).
    pub(super) added: bool,
    pub(super) notes: Vec<CodedError>,
}

/// Why a take didn't land.
#[derive(Debug, PartialEq)]
pub(super) enum ImportError {
    /// A recorder holds the take (a worker records on).
    Live,
    /// What the person is told.
    Said(Said),
}

impl From<Said> for ImportError {
    fn from(said: Said) -> Self {
        Self::Said(said)
    }
}

/// Lands the take in `dir` where `to` says (the module doc). `probe`: a run's file as an asset (the app's media
/// probe).
pub(super) fn import_take(
    sessions: &ProjectSessionState,
    dir: &SessionDir,
    to: &Destination<'_>,
    probe: &mut dyn FnMut(&Path, &str) -> Result<Asset, String>,
) -> Result<Landed, ImportError> {
    let manifest = manifest_of(dir)?;
    let (imported, notes) = recording_from(&manifest, dir, &manifest_peaks, probe)
        .map_err(|e| ImportError::Said(Said::Coded(e)))?;
    let recording = imported.recording;
    let Target {
        id: project,
        file,
        made,
    } = destination::open_for(sessions, dir, to)?;
    let mirror = mirror_of(dir);
    let (clip_id, added) = add(sessions, &project, imported.assets, &recording, mirror)
        .inspect_err(|_| {
            if made {
                destination::unmake(sessions, &file);
            }
        })?;
    imported_in(dir, &project);
    tracing::info!(
        take = recording.id,
        "recorder: the take landed in its project"
    );
    let mut all = manifest.notes;
    for note in notes {
        if !all.contains(&note) {
            all.push(note);
        }
    }
    Ok(Landed {
        project,
        file,
        recording_id: recording.id,
        clip_id,
        added,
        notes: all,
    })
}

/// The step in `project`, unless the take is in it already: its clip's id either way (empty when the person has
/// deleted that clip since), and whether the step was made now.
fn add(
    sessions: &ProjectSessionState,
    project: &str,
    assets: Vec<Asset>,
    recording: &Recording,
    mirror: bool,
) -> Result<(String, bool), Said> {
    let clip = clip_for(recording, mirror);
    let clip_id = clip.id.clone();
    let mut there = None;
    // Built under the project's lock, so nothing lands between the look and the step.
    build_and_apply(sessions, Some(project), None, LABEL, |open| {
        if open.recordings.iter().any(|r| r.id == recording.id) {
            let source = ClipSource::Recording {
                id: recording.id.clone(),
            };
            let clip = (open.timeline.main.iter().rev()).find(|c| c.source == source);
            there = Some(clip.map(|c| c.id.clone()).unwrap_or_default());
            // No ops: no step.
            return Ok(Vec::new());
        }
        Ok(vec![
            Op::AddAssets { assets },
            Op::AddRecording {
                recording: recording.clone(),
            },
            Op::InsertClip { clip, index: None },
        ])
    })
    .map_err(|e| e.said())?;
    Ok(match there {
        Some(clip_id) => {
            tracing::info!(
                take = recording.id,
                "recorder: the take is in its project already"
            );
            (clip_id, false)
        }
        None => (clip_id, true),
    })
}

/// The take's "Mirror my camera", from its `session.json`; on, the default, when that can't be read.
fn mirror_of(dir: &SessionDir) -> bool {
    read_config(dir).map_or(true, |cfg| cfg.mirror_camera)
}

/// The take's manifest: as the worker's save left it, else finished from its files now.
fn manifest_of(dir: &SessionDir) -> Result<Manifest, ImportError> {
    let end = recover::ending(dir);
    if end.finalized || end.recovered {
        match read_manifest(dir) {
            Ok(manifest) => return Ok(manifest),
            Err(error) => {
                tracing::warn!(%error, "recorder: a saved take's manifest is unreadable, so it is finished from its files");
            }
        }
    }
    match recover::repair(dir) {
        Ok(repaired) => {
            tracing::info!(
                took_ms = repaired.took_ms,
                "recorder: the take was finished from its files"
            );
            Ok(repaired.manifest)
        }
        Err(RecoverError::Live) => Err(ImportError::Live),
        Err(error) => {
            tracing::error!(%error, "recorder: the take couldn't be finished from its files");
            Err(ImportError::Said(Said::code("capture.recovered.failed")))
        }
    }
}

/// `Imported { project }` at the end of the take's journal, pushed to the disk, then its `imported` mark. Either failing
/// is logged: the take is in its project, and landing it again finds it there.
fn imported_in(dir: &SessionDir, project: &str) {
    let entry = Entry::Imported {
        project: project.to_owned(),
    };
    let journaled = journal::open_append(&dir.journal()).and_then(|mut file| {
        file.write_all(&journal::line(&entry))?;
        durable::push(&file)
    });
    if let Err(error) = journaled {
        tracing::warn!(%error, "recorder: the take's Imported line wasn't written");
    }
    if let Err(error) = session::mark_imported(dir) {
        tracing::warn!(%error, "recorder: the take's imported mark wasn't written");
    }
}

/// What the app gives a landing: where new projects go, today, the recorder's files and the media probe.
pub(super) struct Landing<'a> {
    pub(super) library: &'a dyn Fn() -> Result<PathBuf, Said>,
    pub(super) today: Day,
    /// The recorder's own files (the encoder probe's report); None without an app data folder.
    pub(super) files: Option<&'a Files>,
    pub(super) probe: &'a mut dyn FnMut(&Path, &str) -> Result<Asset, String>,
}

/// Lands `done` (`import_take`), then adds its auto zoom (`after_landing`): what the page hears as `recorder-imported`,
/// or the note it hears instead, a `capture.*` code (the take's files stay in its folder either way). A take recorded on
/// a fallback encoder lets the probe's report go, landed or not, so the next opening of the Record dialog measures
/// again.
pub(super) fn land(
    sessions: &ProjectSessionState,
    done: &Done,
    with: Landing<'_>,
) -> Result<TakeImport, CodedError> {
    let to = Destination {
        target: &done.target,
        file: done.project_file.as_deref(),
        library: with.library,
        today: with.today,
    };
    let landed = import_take(sessions, &done.dir, &to, with.probe);
    if let Some(files) = with.files {
        forget_stale_probe(files, &done.dir, &landed);
    }
    let landed = landed.map_err(|error| {
        tracing::warn!(take = done.id, ?error, "recorder: the take didn't land");
        not_landed(error)
    })?;
    after_landing(sessions, done, landed)
}

/// After a take's step: its project open first, again should the person have closed it, or opened another, since the
/// step (`snapshot_after`), so its auto zoom goes there and is never lost; then the zoom step (`zoom_take`) when the
/// take was added now; then the page's `TakeImport`, its snapshot the take's project, zooms and all. A take that
/// landed but whose zooms the project refused is still landed, with none.
pub(super) fn after_landing(
    sessions: &ProjectSessionState,
    done: &Done,
    landed: Landed,
) -> Result<TakeImport, CodedError> {
    let opened = snapshot_after(sessions, &landed)?;
    let zooms = if landed.added {
        let (project, take, clip) = (&landed.project, &landed.recording_id, &landed.clip_id);
        zoom_take(sessions, project, take, clip).unwrap_or_else(|said| {
            tracing::warn!(
                take = done.id,
                ?said,
                "recorder: the take landed without its auto zoom"
            );
            0
        })
    } else {
        0
    };
    let snapshot = if zooms > 0 {
        snapshot_after(sessions, &landed)?
    } else {
        opened
    };
    let mut notes: Vec<CodedError> = done.note.iter().cloned().collect();
    notes.extend(landed.notes);
    Ok(TakeImport {
        take: done.id.clone(),
        recording_id: landed.recording_id,
        clip_id: landed.clip_id,
        snapshot,
        notes,
        zooms,
    })
}

/// The take's project as it is now, open, for the page. Should the person have closed it, or opened another, while the
/// take landed, it opens again (the open one saved and closed first, as for a take whose project isn't open when it
/// lands), so a take that was added is never reported as not added. When it can't be opened:
/// `capture.import.addedNotOpened`, "The recording was added to your project. Open the project from the start screen to
/// see it."
pub(super) fn snapshot_after(
    sessions: &ProjectSessionState,
    landed: &Landed,
) -> Result<Snapshot, CodedError> {
    if let Ok(snapshot) = snapshot_now(sessions)
        && snapshot.project.id == landed.project
    {
        return Ok(snapshot);
    }
    tracing::warn!(
        take = landed.recording_id,
        "recorder: the take's project closed while it landed, so it opens again"
    );
    open_in(sessions, &landed.file).map_err(|said| {
        tracing::error!(
            ?said,
            take = landed.recording_id,
            "recorder: the take's project couldn't be opened again"
        );
        CodedError::new("capture.import.addedNotOpened")
    })
}

/// The probe's report goes after a take recorded on a fallback encoder: from its notes when it landed, else from its
/// manifest's when there is one.
fn forget_stale_probe(files: &Files, dir: &SessionDir, landed: &Result<Landed, ImportError>) {
    match landed {
        Ok(landed) => setup::forget_stale_probe(files, &landed.notes),
        Err(_) => {
            if let Ok(manifest) = read_manifest(dir) {
                setup::forget_stale_probe(files, &manifest.notes);
            }
        }
    }
}

/// What the person is told when a take didn't land: the recorder's own words (a take with no picture, one that couldn't
/// be finished from its files), else "The recording couldn't be added to your project. Its files are in Movies /
/// Klipolo / Recordings."
fn not_landed(error: ImportError) -> CodedError {
    match error {
        ImportError::Said(Said::Coded(said)) if said.code.starts_with("capture.") => said,
        _ => CodedError::new("capture.import.notAdded"),
    }
}
