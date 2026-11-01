//! The screen smoke run on a Mac, by hand only (the consent guard refuses under `KLIPOLO_TEST`): one short real take
//! through the whole recorder (the session core, the writer, Stop's remux) with the real macOS backend, then technical
//! numbers only: frames, keyframes, frame intervals, bitrate, sizes, sound frames and levels, cursor lines, each source's
//! start on the recording clock, the take's notes. It never looks at what was recorded, uploads nothing, and deletes
//! the take when done (`--keep` leaves it for an `ffprobe` count, to be deleted right after).
//!
//! `cargo run -p klipolo-capture --features real-capture,synthetic --example screen_smoke -- <scratch dir> <seconds>
//!  [display | app <bundle id> | window <id> | camera] [--preset light|smooth|native30|native60|camera720|camera1080]
//!  [--encoder <id>] [--mic] [--computer-sound] [--app-sound] [--no-cursor | --cursor separate]
//!  [--kill-cursor-at <s>] [--max-secs <s>] [--keep] [--with-camera] [--shell] [--bubble] [--pause]`
//!
//! The cursor is recorded burned in unless `--cursor separate` records it as its own layer, as the app's
//! preflight does on macOS (this example decides by its flags alone). `--kill-cursor-at <s>` stops
//! the cursor source `s` seconds after it started, so the watchdog's live fallback shows. Takes are 5-90 s; `--max-secs`
//! allows up to 300 (a 3-minute take checks the clicks' drift and the resident size).
//!
//! `camera` records the Mac's built-in camera alone (never a Continuity camera), at the camera preset.
//! `--with-camera` records it beside the screen at Camera720; `--shell` runs the real menu-bar item, HUD and keys;
//! `--bubble` shows the camera bubble too (both of those, and the worker's CPU is printed for each run).
//!
//! Without `--shell` there's no menu-bar item or HUD (a quiet shell that runs AppKit's loop, so the mouse monitor gets
//! its events); the take starts at once (no countdown), a second in with the shell, and stops after `seconds`.

// The real backend with a quiet shell (or its own with `--shell`), and the cursor source stopped on request.
#[cfg(target_os = "macos")]
mod quiet;
#[cfg(target_os = "macos")]
mod report;

fn main() {
    #[cfg(target_os = "macos")]
    mac::run();
    #[cfg(not(target_os = "macos"))]
    println!("screen_smoke: macOS only");
}

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::time::Duration;

    use klipolo_capture::preflight::cursor::CursorPlan;

    use klipolo_capture::backend::*;
    use klipolo_capture::media::Codec;
    use klipolo_capture::protocol::Command;
    use klipolo_capture::run::threads::run_with;
    use klipolo_capture::session::*;
    use klipolo_capture::shell::KeyBindings;
    use klipolo_model::project::{FormatKind, RecordingKind, SourceRole};
    use klipolo_platform::caps::OsFamily;

    use crate::quiet::Quiet;
    use crate::report::report;

    pub fn run() {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(tracing::Level::INFO)
            .init();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let has = |flag: &str| args.iter().any(|a| a == flag);
        let value =
            |flag: &str| (args.iter().position(|a| a == flag)).and_then(|i| args.get(i + 1));
        let scratch = Path::new(args.first().expect("a scratch folder"));
        let seconds: u32 = args.get(1).and_then(|s| s.parse().ok()).expect("seconds");
        let longest = value("--max-secs")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(90)
            .min(300);
        assert!(
            (5..=longest).contains(&seconds),
            "a short take: 5-{longest} s"
        );
        let bubble = has("--bubble");
        let shell = bubble || has("--shell");
        let with_camera = bubble || has("--with-camera");
        let kill = value("--kill-cursor-at").and_then(|s| s.parse::<f64>().ok());
        let backend = Box::new(Quiet(klipolo_capture::platform::backend(), kill, shell));
        let devices = backend.devices();
        let display = devices.displays.first().expect("a display").id.clone();
        let target = match args.get(2).map(String::as_str) {
            Some("app") => Target::App {
                app_id: args.get(3).expect("a bundle id").clone(),
                display: display.clone(),
            },
            Some("window") => Target::Window {
                id: args.get(3).expect("a window id").clone(),
            },
            _ => Target::Display { id: display },
        };
        let preset = match value("--preset").map(String::as_str) {
            Some("smooth") => PresetId::Smooth,
            Some("native30") => PresetId::Native30,
            Some("native60") => PresetId::Native60,
            Some("camera720") => PresetId::Camera720,
            Some("camera1080") => PresetId::Camera1080,
            _ => PresetId::Light,
        };
        let camera = args.get(2).map(String::as_str) == Some("camera");
        let encoders = (value("--encoder").into_iter())
            .map(|id| EncoderPick {
                codec: if id.contains("hevc") {
                    Codec::Hevc
                } else {
                    Codec::H264
                },
                id: id.clone(),
            })
            .collect();
        let app_sound = has("--app-sound");
        let mut sources = vec![if camera {
            // The Mac's own camera only, never a Continuity camera (a phone).
            let built_in = |c: &&DeviceInfo| c.transport == Transport::BuiltIn;
            let device = devices
                .cameras
                .iter()
                .find(built_in)
                .expect("a built-in camera");
            let device = device.id.clone();
            let spec = SourceSpec::Camera {
                device,
                preset,
                encoders,
            };
            source("camera", SourceRole::Camera, spec)
        } else {
            let spec = SourceSpec::Screen(VideoSpec {
                target,
                preset,
                cursor: match (has("--no-cursor"), value("--cursor").map(String::as_str)) {
                    (true, _) => CursorPlan::None,
                    (_, Some("separate")) => CursorPlan::Separate,
                    _ => CursorPlan::Burned,
                },
                sound: app_sound,
                encoders,
                bubble: false,
                bubble_mirrored: false,
            });
            source("screen", SourceRole::Screen, spec)
        }];
        if with_camera && !camera {
            let built_in = |c: &&DeviceInfo| c.transport == Transport::BuiltIn;
            let device = (devices.cameras.iter().find(built_in))
                .expect("a built-in camera")
                .id
                .clone();
            let spec = SourceSpec::Camera {
                device,
                preset: PresetId::Camera720,
                encoders: Vec::new(),
            };
            sources.push(source("camera", SourceRole::Camera, spec));
        }
        if app_sound {
            let of = SourceSpec::AppSound {
                of: "screen".into(),
            };
            sources.push(source("appSound", SourceRole::SystemAudio, of));
        }
        if has("--computer-sound") {
            sources.push(source(
                "computerSound",
                SourceRole::SystemAudio,
                SourceSpec::System,
            ));
        }
        if has("--mic") {
            let mic = devices.mics.first().expect("a mic").id.clone();
            let latency = klipolo_capture::platform::macos::input_latency(&mic);
            println!("mic input latency (Core Audio's report): {latency:?}");
            let (name, half) = (mic.clone(), u64::from(seconds) * 500);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(half));
                let rates = klipolo_capture::platform::macos::input_rates(&name);
                println!("mic rates mid-take (nominal, actual): {rates:?}");
            });
            sources.push(source(
                "mic",
                SourceRole::Mic,
                SourceSpec::Mic { device: mic },
            ));
        }
        let dir = SessionDir::create(scratch, "smoke").expect("a new take folder");
        let kind = if camera {
            RecordingKind::Camera
        } else {
            RecordingKind::Screen
        };
        let mut cfg = config(sources, kind);
        cfg.camera_bubble = bubble;
        write_config(&dir, &cfg).expect("the config");
        // With the shell, a second for AppKit's loop (and the bubble's window) before the start.
        let lead = if shell { 1_000 } else { 0 };
        let mut script = vec![
            (lead, Command::Start),
            (lead + seconds * 1_000, Command::Stop),
        ];
        // `--pause`: one pause (from 7 s to 11 s), for the fresh-picture check after a resume.
        if has("--pause") {
            script.push((lead + 7_000, Command::Pause));
            script.push((lead + 11_000, Command::Resume));
            script.sort_by_key(|(ms, _)| *ms);
        }
        cpu_between(lead + 5_000, lead + seconds * 1_000 - 2_000);
        let code = run_with(backend, dir.clone(), false, script);
        println!("exit code {code}");
        report(&dir, preset);
        if !has("--keep") {
            std::fs::remove_dir_all(&dir.root).expect("the take deleted");
            println!("take deleted");
        }
    }

    /// Prints this process's CPU (user and system, % of one core) between `from_ms` and `to_ms` after now: the take's
    /// steady state, its start and its finishing left out.
    fn cpu_between(from_ms: u32, to_ms: u32) {
        let cpu_us = || {
            // SAFETY: getrusage writes into a zeroed out-struct.
            let mut u: libc::rusage = unsafe { std::mem::zeroed() };
            unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
            let us = |t: libc::timeval| t.tv_sec as u64 * 1_000_000 + t.tv_usec as u64;
            us(u.ru_utime) + us(u.ru_stime)
        };
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(u64::from(from_ms)));
            let (start, at) = (cpu_us(), std::time::Instant::now());
            std::thread::sleep(Duration::from_millis(u64::from(
                to_ms.saturating_sub(from_ms),
            )));
            let (used, wall) = (cpu_us() - start, at.elapsed().as_micros() as u64);
            println!(
                "worker CPU: {:.1} % of one core over {:.1} s",
                100.0 * used as f64 / wall.max(1) as f64,
                wall as f64 / 1e6
            );
        });
    }

    fn source(id: &str, role: SourceRole, spec: SourceSpec) -> SourceConfig {
        SourceConfig {
            id: id.into(),
            role,
            label: format!("capture.source.{id}"),
            spec,
        }
    }

    fn config(sources: Vec<SourceConfig>, kind: RecordingKind) -> SessionConfig {
        SessionConfig {
            id: "smoke".into(),
            created_ms: 0,
            kind,
            sources,
            countdown_s: 0,
            auto_pause: None,
            target: ImportTarget::NewProject {
                name: "Smoke".into(),
                formats: vec![FormatKind::Landscape],
            },
            game: None,
            app_version: "smoke".into(),
            os: "macos".into(),
            expected_minutes: 1,
            words: BTreeMap::new(),
            keys: KeyBindings::defaults(OsFamily::Macos),
            hud_corner: Default::default(),
            camera_bubble: false,
            mirror_camera: true,
        }
    }
}
