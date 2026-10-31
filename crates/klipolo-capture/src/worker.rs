//! `klipolo --record-worker`: the recorder process the app relaunches itself as (as the export worker), in its own
//! process group, so it outlives an editor crash. Its forms:
//! - `--record-worker <session folder> [--resume]`: records the take whose `session.json` the app wrote (`resume`: goes
//!   on with it after a crashed worker, `run::resume`);
//! - `--record-worker --check-camera <camera id>`: Check camera's preview;
//! - `--record-worker --live-view <target> [<view>]`: the recorder's live view of `target` (a `backend::Target` as
//!   JSON), no sound, nothing written, for the big view of `view` pixels (`live_view_size::ViewPixels` as
//!   JSON), else the small one;
//! - `--record-worker --thumbs <targets>`: the Record dialog's stills of each screen and app, about once a second each
//!   (`run::thumbs`), at utility QoS; `targets` is a JSON list of `[key, target]` pairs, each target a
//!   `backend::Target` that shows a screen (a camera or the system's picker is a usage error);
//! - `--record-worker --meter <mic id>`: the Record dialog's mic meter, ten peaks a second of that mic alone
//!   (`run::meter`);
//! - `--record-worker --probe <out.json>`: the encoder probe, on generated frames only (so without the consent guard,
//!   `platform::backend_for_probe`), its report written atomically.
//!
//! A take's worker logs to its folder's `diagnostics.log` (at most 5 MB, rotated once), so an orphaned worker still
//! leaves a record; the other forms log to stderr. Capture needs the `real-capture` backend (only the app builds it);
//! without it every form exits 2. Exit codes: `protocol`.

mod log;

use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use klipolo_platform::durable::{Durability, write_atomic};
use klipolo_platform::qos::{self, Qos};

use crate::backend::{Backend, Target};
use crate::live_view_size::ViewPixels;
use crate::run::PreviewIo;
use crate::run::check::run_check_camera;
use crate::run::live_view::run_live_view;
use crate::run::meter::run_meter;
use crate::run::threads::run_session;
use crate::run::thumbs::run_thumbs;
use crate::session::SessionDir;

/// How long the first-run probe may take: 5 s at most.
const PROBE_BUDGET: Duration = Duration::from_secs(5);

/// `args`: the process's arguments; what follows `--record-worker` picks the form.
pub fn main(args: &[String]) -> i32 {
    let rest: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .skip_while(|a| *a != "--record-worker")
        .skip(1)
        .collect();
    match rest.as_slice() {
        ["--check-camera", camera] => {
            log::to_stderr();
            with_backend(|b| run_check_camera(b, camera))
        }
        ["--live-view", target, view @ ..] if view.len() <= 1 => {
            log::to_stderr();
            let view = view.first().map(|v| serde_json::from_str::<ViewPixels>(v));
            match (serde_json::from_str::<Target>(target), view.transpose()) {
                (Ok(target), Ok(view)) => with_backend(|b| run_live_view(b, &target, view)),
                (Err(error), _) | (_, Err(error)) => {
                    eprintln!("--live-view takes a target and a view size as JSON: {error}");
                    2
                }
            }
        }
        ["--thumbs", targets] => {
            log::to_stderr();
            match serde_json::from_str::<Vec<(String, Target)>>(targets) {
                Ok(targets) if targets.iter().all(|(_, t)| shows_a_screen(t)) => {
                    // The stills are background work: macOS passes the class on to the services that take them.
                    qos::set_current(Qos::Utility);
                    with_backend(|b| run_thumbs(&*b, &targets, PreviewIo::std()))
                }
                Ok(_) => {
                    eprintln!(
                        "--thumbs takes screens, apps and windows only, never a camera or the system's picker"
                    );
                    2
                }
                Err(error) => {
                    eprintln!("--thumbs takes a JSON list of [key, target] pairs: {error}");
                    2
                }
            }
        }
        ["--meter", mic] => {
            log::to_stderr();
            with_backend(|b| run_meter(&*b, mic, PreviewIo::std()))
        }
        ["--probe", out] => {
            log::to_stderr();
            match probe_backend() {
                Some(backend) => probe(&*backend, Path::new(out)),
                None => no_backend(),
            }
        }
        // A flag is never a folder: `--probe`, `--check-camera`, `--live-view`, `--thumbs` or `--meter` without its
        // argument is a usage error, not a take.
        [folder] | [folder, "--resume"] if !folder.starts_with("--") => {
            let dir = SessionDir::at(Path::new(folder));
            log::to_file(&dir.diagnostics());
            // Capture and encode at user-interactive, the writer at user-initiated: nothing in the worker is capped.
            qos::set_ceiling(Qos::UserInteractive);
            let resume = rest.len() == 2;
            tracing::info!(resume, "worker: recording");
            with_backend(|b| {
                let input = Box::new(BufReader::new(std::io::stdin()));
                run_session(b, dir, resume, input, Box::new(std::io::stdout()))
            })
        }
        _ => {
            eprintln!(
                "usage: --record-worker <session folder> [--resume] | --record-worker --check-camera <camera id> | \
                 --record-worker --live-view <target JSON> [<view JSON>] | --record-worker --thumbs <targets JSON> | \
                 --record-worker --meter <mic id> | --record-worker --probe <out.json>"
            );
            2
        }
    }
}

/// Whether a still can be taken of `target`: a display, an app's windows or a window.
fn shows_a_screen(target: &Target) -> bool {
    !matches!(target, Target::Camera { .. } | Target::Portal { .. })
}

/// This OS's backend (the consent guard runs first), or exit 2 in a build without `real-capture`.
fn with_backend(run: impl FnOnce(Box<dyn Backend>) -> i32) -> i32 {
    #[cfg(feature = "real-capture")]
    {
        run(crate::platform::backend())
    }
    #[cfg(not(feature = "real-capture"))]
    {
        let _ = run;
        no_backend()
    }
}

/// The probe's backend, without the consent guard (it encodes generated frames only), or None in a build without
/// `real-capture`.
fn probe_backend() -> Option<Box<dyn Backend>> {
    #[cfg(feature = "real-capture")]
    {
        Some(crate::platform::backend_for_probe())
    }
    #[cfg(not(feature = "real-capture"))]
    {
        None
    }
}

/// Exit 2: a build without `real-capture` can't record or probe.
fn no_backend() -> i32 {
    tracing::error!("worker: this build has no capture backend (real-capture)");
    eprintln!("this build can't record (no real-capture backend)");
    2
}

fn probe(backend: &dyn Backend, out: &Path) -> i32 {
    let report = backend.probe(PROBE_BUDGET);
    let written = serde_json::to_vec_pretty(&report)
        .map_err(std::io::Error::other)
        .and_then(|json| write_atomic(out, &json, Durability::Full));
    match written {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!(error = %e, "worker: the probe's report wasn't written");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrong_form_or_a_build_without_capture_exits_2() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(main(&args(&["klipolo", "--record-worker"])), 2);
        assert_eq!(main(&args(&["klipolo", "--record-worker", "--probe"])), 2);
        assert_eq!(
            main(&args(&["klipolo", "--record-worker", "--live-view"])),
            2
        );
        let not_a_target = args(&["klipolo", "--record-worker", "--live-view", "{}"]);
        assert_eq!(main(&not_a_target), 2);
        let screen = r#"{"kind":"display","id":"display-1"}"#;
        let not_a_view = args(&["klipolo", "--record-worker", "--live-view", screen, "{}"]);
        assert_eq!(main(&not_a_view), 2);
        assert_eq!(main(&args(&["klipolo", "--record-worker", "--thumbs"])), 2);
        assert_eq!(main(&args(&["klipolo", "--record-worker", "--meter"])), 2);
        let not_targets = args(&["klipolo", "--record-worker", "--thumbs", screen]);
        assert_eq!(main(&not_targets), 2);
        // A camera or the system's picker is never a still's target, in any build.
        for target in [
            r#"{"kind":"camera","id":"cam"}"#,
            r#"{"kind":"portal","restoreToken":null,"appId":null}"#,
        ] {
            let pairs = format!(r#"[["display-1",{screen}],["other",{target}]]"#);
            let thumbs = args(&["klipolo", "--record-worker", "--thumbs", &pairs]);
            assert_eq!(main(&thumbs), 2, "{target}");
        }
        if !cfg!(feature = "real-capture") {
            assert_eq!(
                main(&args(&[
                    "klipolo",
                    "--record-worker",
                    "--check-camera",
                    "cam"
                ])),
                2
            );
            let screens = format!(r#"[["display-1",{screen}]]"#);
            let thumbs = args(&["klipolo", "--record-worker", "--thumbs", &screens]);
            assert_eq!(main(&thumbs), 2);
            let meter = args(&["klipolo", "--record-worker", "--meter", "mic-1"]);
            assert_eq!(main(&meter), 2);
        }
    }
}
