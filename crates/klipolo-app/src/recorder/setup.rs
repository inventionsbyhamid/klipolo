//! The Record dialog's data and what a take is planned with, from the backend's answers without capture or prompts
//! (`klipolo_capture::preflight`), all off the page's thread:
//! - **The encoder probe** (generated frames only) runs once per macOS version and Klipolo version, as
//!   `klipolo-app --record-worker --probe <file>` (5 s at most), and is kept at
//!   `<app data>/recorder/probe-<os>-<klipolo>.json`. One probe at a time (two at once would each measure half the
//!   engine), never while a take is the recorder's (it would measure beside the take's sessions), and a report in which
//!   every encoder refused is never used, kept or found (an earlier build may have kept one), so the next opening
//!   measures again. A report with no encoder at all (a computer without a hardware one) is kept. A probe that hung or
//!   crashed isn't waited for again until Klipolo opens again, and leaves a `.failed` mark for diagnosis.
//! - **The recordings folder** is measured once per opening of the dialog (`check_folder`: 1 MB written and fully
//!   flushed, and whether it can lock a file), never while a take records there; one that can't lock is refused up
//!   front (`refuse_folder`).
//! - **Screen Recording still refused after Reopen Klipolo** (`screen_reopen`) is said, so the step explains.
//! - **A take's plan** (`plan`): the quality marked best for the display and camera, the encoders the probe chose for
//!   it with their fallbacks, and the camera only where that quality has room for it. "Record yourself" (the camera
//!   alone) records the camera at its own best (`probe::camera_alone`), with no screen.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use klipolo_capture::backend::{Backend, PresetId, ProbeReport};
use klipolo_capture::encoder_pick::FALLBACK_NOTE;
use klipolo_capture::preflight::{Fitted, RecorderSetup, check_folder, setup_with};
use klipolo_capture::probe::{
    CameraChoice, ScreenChoice, camera_alone, camera_encoders_for, encoders_for,
};
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
        self.dir.join(format!("{}.json", probe_name()))
    }

    /// The mark of a probe that hung or crashed, for this macOS version and this Klipolo: for diagnosis only.
    pub(super) fn probe_failed(&self) -> PathBuf {
        self.dir.join(format!("{}.failed", probe_name()))
    }
}

/// `probe-<os>-<klipolo>`.
fn probe_name() -> String {
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
    format!("probe-{os}-{}", env!("CARGO_PKG_VERSION"))
}

/// What the Record dialog was shown: the setup, and the probe's report it was fitted with (the take's encoders come
/// from it).
#[derive(Clone, Debug)]
pub(super) struct Shown {
    pub(super) setup: RecorderSetup,
    pub(super) report: Option<ProbeReport>,
}

/// How a setup gets the probe's report.
pub(super) enum Probing<'a> {
    /// Measured now when none is kept, by `run` (which writes it to the path it is given), one probe at a time, unless
    /// a probe gave no report since Klipolo opened (`failed`).
    Run {
        run: &'a dyn Fn(&Path),
        one_at_a_time: &'a Mutex<()>,
        failed: &'a AtomicBool,
    },
    /// Only a kept report: a take is the recorder's, or a take is armed without the dialog.
    Kept,
}

/// The probe's report: kept, else measured now (`Probing::Run`), else None (the dialog then offers no quality and the
/// backend picks its encoder).
fn probe_report(files: &Files, probing: Probing<'_>) -> Option<ProbeReport> {
    if let Some(report) = kept(files) {
        return Some(report);
    }
    let Probing::Run {
        run,
        one_at_a_time,
        failed,
    } = probing
    else {
        return None;
    };
    let _one = one_at_a_time.lock().unwrap_or_else(PoisonError::into_inner);
    // Another setup may have measured while this one waited.
    if let Some(report) = kept(files) {
        return Some(report);
    }
    if failed.load(Ordering::SeqCst) {
        return None;
    }
    if let Err(error) = std::fs::create_dir_all(&files.dir) {
        tracing::warn!(%error, "recorder: the recorder's folder can't be made");
        return None;
    }
    run(&files.probe());
    match read_json::<ProbeReport>(&files.probe()) {
        Some(report) if every_encoder_refused(&report) => {
            tracing::warn!(
                "recorder: every encoder refused the probe, so it measures again next time"
            );
            forget(&files.probe());
            None
        }
        Some(report) => {
            forget(&files.probe_failed());
            Some(report)
        }
        None => {
            tracing::warn!(
                "recorder: the encoder probe gave no report, so it isn't run again until Klipolo opens again"
            );
            failed.store(true, Ordering::SeqCst);
            if let Err(error) = std::fs::write(files.probe_failed(), b"") {
                tracing::warn!(%error, "recorder: the probe's failure wasn't marked");
            }
            None
        }
    }
}

/// The kept report, unless every encoder refused it (an earlier build kept such reports): that one goes, so the probe
/// measures again.
fn kept(files: &Files) -> Option<ProbeReport> {
    let report = read_json::<ProbeReport>(&files.probe())?;
    if every_encoder_refused(&report) {
        tracing::warn!(
            "recorder: a kept probe report in which every encoder refused is measured again"
        );
        forget(&files.probe());
        return None;
    }
    Some(report)
}

/// A take recorded on another encoder than the one the probe chose (`capture.note.encoderFallback` among its notes):
/// the probe's report is let go, so the next opening of the Record dialog measures again.
pub(super) fn forget_stale_probe(files: &Files, notes: &[CodedError]) {
    if notes.iter().any(|note| note.code == FALLBACK_NOTE) {
        tracing::info!(
            "recorder: a take fell back to another encoder, so the probe measures again"
        );
        forget(&files.probe());
    }
}

/// Removes a file of the recorder's, if it is there.
fn forget(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(%error, file = ?path.file_name(), "recorder: a file of its own wasn't removed")
        }
    }
}

/// Every encoder there was refused its session (as one can right after a restart or under load), so nothing was
/// measured. A report with no encoder at all is a computer without a hardware one: nothing to measure again.
fn every_encoder_refused(report: &ProbeReport) -> bool {
    !report.encoders.is_empty()
        && (report.encoders.iter()).all(|e| e.fps_1080p60 <= 0.0 && e.max_fps_1440p <= 0.0)
}

/// The Record dialog's data. `root`: the recordings folder; `measure`: write 1 MB there to see how fast it is (never
/// while a take records there). `own_take`: a take is the recorder's.
pub(super) fn recorder_setup(
    backend: &dyn Backend,
    files: &Files,
    probing: Probing<'_>,
    root: Option<&Path>,
    measure: bool,
    own_take: bool,
) -> Shown {
    let report = probe_report(files, probing);
    let folder = root.filter(|_| measure).map(check_folder);
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
    Shown { setup, report }
}

/// Plans take `id` from what the dialog showed: its config, landing in `target`, and what to tell the person before
/// Record.
/// - The quality marked best for a whole screen or an app's windows on the take's display, with the camera on or off
///   (the list offers it first), else the first offered, else Smooth.
/// - The camera is that offer's own, fitted beside the screen. An offer without room for a camera records without it,
///   and says so (`capture.preflight.cameraWithScreen`).
/// - The encoders the probe chose for the screen and the camera, each with its fallbacks; without a report, the
///   backend's own pick.
pub(super) fn plan(
    shown: &Shown,
    options: &ArmOptions,
    id: &str,
    target: ImportTarget,
    created_ms: u64,
) -> Result<(SessionConfig, Vec<CodedError>), CodedError> {
    check_ids(options)?;
    let setup = &shown.setup;
    let display = match &options.what {
        WhatToRecord::Screen { display } | WhatToRecord::App { display, .. } => display.as_str(),
        WhatToRecord::Camera => return plan_camera(shown, options, id, target, created_ms),
    };
    let fit = |camera_on: bool| {
        (setup.displays.iter())
            .find(|d| d.display == display)
            .or(setup.displays.first())
            .map(|d| match &d.with_camera {
                Some(with) if camera_on => with.clone(),
                _ => d.no_camera.clone(),
            })
            .unwrap_or_else(|| Fitted {
                choices: setup.choices.clone(),
                offers: setup.offers.clone(),
            })
    };
    let best = |fit: &Fitted| {
        let offer = (fit.offers.iter())
            .find(|o| o.best_native)
            .or(fit.offers.first());
        offer.cloned()
    };
    let mut options = options.clone();
    let mut notes = Vec::new();
    let mut fitted = fit(options.camera.is_some());
    let mut offer = best(&fitted);
    if options.camera.is_some()
        && let Some(without) = offer.as_ref().filter(|o| o.camera.is_none())
    {
        // No room for the camera at this quality: the screen alone, as the dialog's note says.
        let note = (without.camera_note.clone())
            .unwrap_or_else(|| CodedError::new("capture.preflight.cameraWithScreen"));
        tracing::info!(
            take = id,
            "recorder: the camera has no room beside the screen, so it isn't recorded"
        );
        notes.push(note);
        options.camera = None;
        options.camera_bubble = false;
        fitted = fit(false);
        offer = best(&fitted);
    }
    let preset = offer.as_ref().map_or_else(PresetId::default, |o| o.id);
    let camera_choice =
        (offer.as_ref().and_then(|o| o.camera.clone())).or(fitted.choices.camera.clone());
    let camera_preset = camera_choice
        .as_ref()
        .map_or(PresetId::Camera720, |c| c.preset);
    let size = (setup.devices.displays.iter())
        .find(|d| d.id == display && d.width > 0 && d.height > 0)
        .map(|d| (d.width, d.height));
    let screen_choice = (fitted.choices.native.as_ref())
        .or(fitted.choices.screen.as_ref())
        .map(|c| ScreenChoice {
            preset,
            ..c.clone()
        });
    let (encoders, camera_encoders) = match (&shown.report, &screen_choice) {
        (Some(report), Some(screen)) => {
            let camera = (camera_choice.as_ref())
                .filter(|_| options.camera.is_some())
                .map(|c| {
                    let camera = CameraChoice {
                        preset: camera_preset,
                        ..c.clone()
                    };
                    camera_encoders_for(report, &camera, Some(screen), size)
                });
            (
                encoders_for(report, screen, size),
                camera.unwrap_or_default(),
            )
        }
        _ => (Vec::new(), Vec::new()),
    };
    let os = klipolo_platform::caps::current().os;
    let choices = TakeChoices {
        preset,
        camera_preset,
        cursor_caps: setup.caps.cursor,
        created_ms,
        os: format!("{:?} {}", os.family, os.version),
        target,
        encoders,
        camera_encoders,
    };
    Ok((session_config(&options, id, &choices), notes))
}

/// Plans a take of the camera alone ("Record yourself"): the camera at its own best on this computer
/// (`probe::camera_alone`: 1080p30 where an encoder holds it, else 720p30) with the probe's encoder and its fallbacks,
/// and the mic. Without a report, 720p30 on the backend's own pick. No camera chosen: `capture.error.noDevice`.
fn plan_camera(
    shown: &Shown,
    options: &ArmOptions,
    id: &str,
    target: ImportTarget,
    created_ms: u64,
) -> Result<(SessionConfig, Vec<CodedError>), CodedError> {
    if options.camera.is_none() {
        return Err(words::no_device());
    }
    let choice = shown.report.as_ref().and_then(|r| Some((r, camera_alone(r)?)));
    let camera_preset = choice.as_ref().map_or(PresetId::Camera720, |(_, c)| c.preset);
    let camera_encoders = (choice.as_ref())
        .map(|(report, c)| camera_encoders_for(report, c, None, None))
        .unwrap_or_default();
    let os = klipolo_platform::caps::current().os;
    let choices = TakeChoices {
        preset: PresetId::default(),
        camera_preset,
        cursor_caps: shown.setup.caps.cursor,
        created_ms,
        os: format!("{:?} {}", os.family, os.version),
        target,
        encoders: Vec::new(),
        camera_encoders,
    };
    Ok((session_config(options, id, &choices), Vec::new()))
}

/// An id the OS gave: bounded, without control characters.
pub(super) fn is_os_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 1_024 && !id.chars().any(char::is_control)
}

/// The page's device, display and app ids are names the OS gave (`is_os_id`).
fn check_ids(o: &ArmOptions) -> Result<(), CodedError> {
    let what: [Option<&String>; 2] = match &o.what {
        WhatToRecord::Screen { display } => [Some(display), None],
        WhatToRecord::Camera => [None, None],
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
