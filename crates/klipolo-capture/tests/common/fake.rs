//! The import tests' fakes: a finished take's folder and manifest (`manifest_with`), the app's media probe (`probe_ok`), a
//! T2 Mac's probe (`t2_report`), and a backend that counts every call that could capture (`FakeCounting`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use klipolo_capture::backend::{
    AppAudioApi, AudioSpec, Backend, CaptureCaps, DeviceInfo, Devices, DisplayInfo, EncoderResult,
    HotkeyApi, IndicatorPlace, Permission, Permissions, PickerKind, PreviewPipe, ProbeReport,
    ScreenApi, Transport, VideoPipes, VideoSpec,
};
use klipolo_capture::manifest::{Manifest, ManifestRun, ManifestSource};
use klipolo_capture::media::Codec;
use klipolo_capture::pipe::{AudioPipe, PipeError};
use klipolo_capture::session::{SessionDir, write_config};
use klipolo_capture::shell::Shell;
use klipolo_capture::synthetic::FakeShell;
use klipolo_model::ids::new_id;
use klipolo_model::project::{Asset, AssetKind, RecordingKind, SourceRole};
use klipolo_model::time::Us;

use super::config::config_with;

/// A source of `manifest_with`: its id, its role and its runs as `(offset µs, seconds)`.
pub type Source<'a> = (&'a str, SourceRole, &'a [(Us, f64)]);

/// A finished take in a new folder (kept for the test run): each source's runs as `(offset µs, seconds)`, each run's
/// file holding its length in µs as text (what `probe_ok` reads back). A source without runs is in the config only, as
/// the manifest leaves it out.
pub fn manifest_with(sources: &[Source]) -> (Manifest, SessionDir) {
    let root: PathBuf = tempfile::tempdir().unwrap().keep();
    let dir = SessionDir::create(&root, "take").unwrap();
    let roles: Vec<(&str, SourceRole)> = sources.iter().map(|&(id, role, _)| (id, role)).collect();
    let cfg = config_with(&roles);
    write_config(&dir, &cfg).unwrap();
    let mut duration_us = 0;
    let mut manifest_sources = vec![];
    for (source, &(id, role, runs)) in cfg.sources.iter().zip(sources) {
        let runs: Vec<ManifestRun> = runs
            .iter()
            .enumerate()
            .map(|(n, &(offset_us, seconds))| {
                let len = (seconds * 1e6).round() as Us;
                let file = SessionDir::run_key(id, n as u32 + 1, role.is_video());
                std::fs::create_dir_all(dir.root.join(id)).unwrap();
                std::fs::write(dir.path(&file), len.to_string()).unwrap();
                duration_us = duration_us.max(offset_us + len);
                run(&file, offset_us, len, 0.0)
            })
            .collect();
        if !runs.is_empty() {
            manifest_sources.push(ManifestSource {
                id: id.into(),
                role,
                label: source.label.clone(),
                runs,
            });
        }
    }
    let m = Manifest {
        session: cfg.id.clone(),
        kind: RecordingKind::Screen,
        duration_us,
        sources: manifest_sources,
        markers: vec![],
        cursor: None,
        game: None,
        notes: vec![],
        recovered: false,
        target: cfg.target.clone(),
    };
    (m, dir)
}

/// A manifest run (through serde, so the fields later tasks add take their defaults).
pub fn run(file: &str, offset_us: Us, duration_us: Us, drift_ppm: f64) -> ManifestRun {
    serde_json::from_value(serde_json::json!({
        "file": file,
        "offsetUs": offset_us,
        "durationUs": duration_us,
        "driftPpm": drift_ppm,
        "approximate": false,
    }))
    .unwrap()
}

/// The app's probe as `manifest_with`'s files answer it: a 1080p30 video for `.mp4`, sound for `.caf`, each as long as
/// its file says; a missing file doesn't probe.
pub fn probe_ok() -> impl FnMut(&Path, &str) -> Result<Asset, String> {
    |path: &Path, name: &str| {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let duration_us: Us = text.trim().parse().map_err(|_| "not media".to_string())?;
        let video = path.extension().is_some_and(|e| e == "mp4");
        Ok(Asset {
            id: new_id(),
            kind: if video {
                AssetKind::Video
            } else {
                AssetKind::Audio
            },
            path: path.to_string_lossy().into_owned(),
            name: name.into(),
            duration_us,
            width: if video { 1920 } else { 0 },
            height: if video { 1080 } else { 0 },
            fps: if video { 30.0 } else { 0.0 },
            rotation: None,
            has_audio: !video,
            audio_lead_us: None,
            made_with: None,
        })
    }
}

/// An encoder as the probe measures it: 1440p60 held up to 60, and nothing at 1080p120.
pub fn encoder(
    id: &str,
    codec: Codec,
    dedicated: bool,
    fps_1080p60: f32,
    flat_1440: f32,
) -> EncoderResult {
    EncoderResult {
        id: id.into(),
        codec,
        hardware: true,
        dedicated,
        engine: id.into(),
        fps_1080p60,
        fps_1440p60: flat_1440.min(60.0),
        fps_1080p120: 0.0,
        max_fps_1440p: flat_1440,
        latency_p95_ms: 12.0,
    }
}

/// A 2018-2020 Intel Mac with a T2: the T2's dedicated HEVC engine (holds 1080p60, about 324 Mpx/s flat out,
/// short of 1440p60) and the Intel iGPU's H.264, which shares the game's GPU.
pub fn t2_report() -> ProbeReport {
    let mut ave = encoder("hevc.ave", Codec::Hevc, true, 60.0, 88.0);
    ave.fps_1440p60 = 50.0;
    ProbeReport {
        encoders: vec![ave, encoder("h264.igpu", Codec::H264, false, 60.0, 70.0)],
        took_ms: 4_000,
    }
}

/// A computer with a 2560×1600 60 Hz display, a built-in camera, these mics and built-in speakers, that can record and
/// has every permission. Every call that could capture is counted and refused.
pub struct FakeCounting {
    pub devices: Devices,
    pub caps: CaptureCaps,
    pub permissions: Permissions,
    calls: AtomicUsize,
}

impl FakeCounting {
    pub fn new(mics: Vec<DeviceInfo>) -> Self {
        let device = |id: &str, name: &str| DeviceInfo {
            id: id.into(),
            name: name.into(),
            transport: Transport::BuiltIn,
            is_virtual: false,
            in_use: false,
            suspended: false,
        };
        Self {
            devices: Devices {
                displays: vec![display("display-1", 2560, 1600, 60)],
                apps: vec![],
                cameras: vec![device("camera-1", "FaceTime HD Camera")],
                mics,
                output: Some(device("speakers-1", "MacBook Pro Speakers")),
            },
            caps: CaptureCaps {
                screen: ScreenApi::ScreenCaptureKit,
                app_audio: AppAudioApi::ScreenCaptureKit,
                hotkeys: HotkeyApi::Carbon,
                focus_events: true,
                picker: PickerKind::OurList,
                hud: true,
                os_supported: true,
                indicator: IndicatorPlace::MenuBar,
                cursor: klipolo_capture::backend::CursorCaps::none(),
            },
            permissions: Permissions {
                screen: Permission::Granted,
                camera: Permission::Granted,
                mic: Permission::Granted,
            },
            calls: AtomicUsize::new(0),
        }
    }

    /// Calls to `video`, `audio`, `preview` and `probe`: preflight must make none.
    pub fn capture_calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn counted(&self) -> PipeError {
        self.calls.fetch_add(1, Ordering::SeqCst);
        PipeError::Unsupported
    }
}

pub fn display(id: &str, width: u32, height: u32, refresh_hz: u32) -> DisplayInfo {
    DisplayInfo {
        id: id.into(),
        name: id.into(),
        width,
        height,
        refresh_hz,
    }
}

impl Backend for FakeCounting {
    fn caps(&self) -> CaptureCaps {
        self.caps.clone()
    }
    fn devices(&self) -> Devices {
        self.devices.clone()
    }
    fn permissions(&self) -> Permissions {
        self.permissions
    }
    fn video(&self, _spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        Err(self.counted())
    }
    fn audio(&self, _spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        Err(self.counted())
    }
    fn preview(&self, _camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        Err(self.counted())
    }
    fn probe(&self, _budget: Duration) -> ProbeReport {
        self.counted();
        ProbeReport::default()
    }
    fn shell(&self) -> Box<dyn Shell> {
        Box::new(FakeShell::new(vec![]).0)
    }
}
