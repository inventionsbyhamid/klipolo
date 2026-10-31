//! The Record dialog's data and what a take is planned with, from the backend's answers without capture or prompts
//! (`klipolo_capture::preflight`), all off the page's thread:
//! - **The encoder probe** (generated frames only) runs once per macOS version and Klipolo version, as
//!   `klipolo-app --record-worker --probe <file>` (5 s at most), and is kept at
//!   `<app data>/recorder/probe-<os>-<klipolo>.json`.
//! - **The recordings folder** is measured once per opening of the dialog (`check_folder`: 1 MB written and fully
//!   flushed, and whether it can lock a file); one that can't lock is refused up front (`refuse_folder`).
//! - **Screen Recording still refused after Reopen Klipolo** (`screen_reopen`) is said, so the step explains.

use std::path::{Path, PathBuf};

use klipolo_capture::backend::{Backend, PresetId, ProbeReport};
use klipolo_capture::preflight::{Fitted, RecorderSetup, check_folder, setup_with};
use klipolo_capture::session::{ImportTarget, SessionConfig};
use klipolo_model::coded::CodedError;
use klipolo_platform::durable::free_space;

use super::config::{TakeChoices, session_config};
use super::screen_reopen::refused_after_reopen;
use super::types::{ArmOptions, WhatToRecord};
use super::words;

/// The recorder's own files, in `<app data>/recorder`.
#[derive(Clone, Debug)]
pub struct Files {
    pub dir: PathBuf,
}

impl Files {
    /// The probe's report for this macOS version and this Klipolo.
    pub(super) fn probe(&self) -> PathBuf {
        let os = klipolo_platform::caps::current().os.version;
        let os: String = (os.chars())
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        (self.dir).join(format!("probe-{os}-{}.json", env!("CARGO_PKG_VERSION")))
    }
}

/// The probe's report: kept, else measured now by `run` (which writes it to the path it is given), else None (the
/// dialog then offers no quality and the backend picks its encoder).
fn probe_report(files: &Files, run: &dyn Fn(&Path)) -> Option<ProbeReport> {
    let path = files.probe();
    if let Some(report) = read_json(&path) {
        return Some(report);
    }
    if let Err(error) = std::fs::create_dir_all(&files.dir) {
        tracing::warn!(%error, "recorder: the recorder's folder can't be made");
        return None;
    }
    run(&path);
    read_json(&path)
}

/// The Record dialog's data. `probe`: runs the encoder probe when none is kept (None: only a kept report is read).
/// `root`: the recordings folder, measured (None skips that). `own_take`: a take is the recorder's.
pub(super) fn recorder_setup(
    backend: &dyn Backend,
    files: &Files,
    probe: Option<&dyn Fn(&Path)>,
    root: Option<&Path>,
    own_take: bool,
) -> RecorderSetup {
    let report = match probe {
        Some(run) => probe_report(files, run),
        None => read_json(&files.probe()),
    };
    let folder = root.map(check_folder);
    let free = root.and_then(free_space);
    let write_ms = folder.and_then(|f| f.write_ms);
    let battery = klipolo_platform::power::battery();
    let mut setup = setup_with(
        backend,
        report.as_ref(),
        free,
        write_ms,
        battery,
        own_take,
        None,
    );
    setup.own_take = own_take;
    // macOS has a page for each of Screen Recording, the camera and the microphone (`permissions::privacy_url`).
    setup.privacy_settings = cfg!(target_os = "macos");
    setup.screen_refused_after_reopen = refused_after_reopen(files, setup.permissions.screen);
    if folder.is_some_and(|f| !f.locks) {
        setup.refuse_folder();
    }
    setup
}

/// Plans take `id` from `setup` (the one the dialog showed): its config, at the quality the setup offered first for
/// the take's display and camera, landing in `target`.
pub(super) fn plan(
    setup: &RecorderSetup,
    options: &ArmOptions,
    id: &str,
    target: ImportTarget,
    created_ms: u64,
) -> Result<SessionConfig, CodedError> {
    check_ids(options)?;
    let display = match &options.what {
        WhatToRecord::Screen { display } | WhatToRecord::App { display, .. } => display.as_str(),
    };
    let camera_on = options.camera.is_some();
    let fit = (setup.displays.iter())
        .find(|d| d.display == display)
        .or(setup.displays.first())
        .map(|d| match &d.with_camera {
            Some(with) if camera_on => with.clone(),
            _ => d.no_camera.clone(),
        })
        .unwrap_or_else(|| Fitted {
            choices: setup.choices.clone(),
            offers: setup.offers.clone(),
        });
    // The quality marked best for a whole screen or an app's windows, which the list offers first; a camera stepped
    // down beside it is that offer's own.
    let offer = (fit.offers.iter())
        .find(|o| o.best_native)
        .or(fit.offers.first());
    let preset = offer.map_or_else(PresetId::default, |o| o.id);
    let camera_preset = (offer.and_then(|o| o.camera.as_ref()))
        .or(fit.choices.camera.as_ref())
        .map_or(PresetId::Camera720, |c| c.preset);
    let os = klipolo_platform::caps::current().os;
    let choices = TakeChoices {
        preset,
        camera_preset,
        cursor_caps: setup.caps.cursor,
        created_ms,
        os: format!("{:?} {}", os.family, os.version),
        target,
    };
    Ok(session_config(options, id, &choices))
}

/// An id the OS gave: bounded, without control characters.
fn is_os_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 1_024 && !id.chars().any(char::is_control)
}

/// The page's device, display and app ids are names the OS gave (`is_os_id`).
fn check_ids(o: &ArmOptions) -> Result<(), CodedError> {
    let what: [Option<&String>; 2] = match &o.what {
        WhatToRecord::Screen { display } => [Some(display), None],
        WhatToRecord::App { app_id, display } => [Some(app_id), Some(display)],
    };
    let mut ids = (o.camera.iter().chain(&o.mic)).chain(what.into_iter().flatten());
    if ids.all(|id| is_os_id(id)) {
        Ok(())
    } else {
        Err(words::no_device())
    }
}

/// A file of the recorder's, or None when it is missing or unreadable (logged: the recorder then uses its defaults).
pub(super) fn read_json<T: for<'de> serde::Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes)
        .inspect_err(|error| tracing::warn!(%error, file = ?path.file_name(), "recorder: a file of its own is unreadable"))
        .ok()
}
