//! The preflight's smoke run on a Mac: the real backend's preflight (caps, devices, permissions, the battery), timed,
//! with no capture and no prompt. It prints counts, transports, sizes and timings only, never a device's or an app's
//! name (for privacy). Run by hand, never in tests (the consent guard refuses under `KLIPOLO_TEST`):
//! `cargo run -p klipolo-capture --features real-capture --example preflight_smoke`.

fn main() {
    #[cfg(target_os = "macos")]
    mac::run();
    #[cfg(not(target_os = "macos"))]
    println!("preflight_smoke: macOS only");
}

#[cfg(target_os = "macos")]
mod mac {
    use std::time::{Duration, Instant};

    use cpal::traits::{DeviceTrait, HostTrait};
    use klipolo_capture::backend::{Devices, Transport};

    pub fn run() {
        let backend = klipolo_capture::platform::backend();
        let caps = backend.caps();
        println!(
            "caps: os_supported {}, screen {:?}, hotkeys {:?}",
            caps.os_supported, caps.screen, caps.hotkeys
        );
        let (devices, took) = timed(|| backend.devices());
        report("main thread", &devices, took);
        let (mics, took) = timed(|| backend.mics());
        println!("mics() alone: {} in {took:?}", mics.len());
        let (permissions, took) = timed(|| backend.permissions());
        println!("permissions: {permissions:?} in {took:?}");
        println!(
            "battery: {:?}",
            klipolo_platform::power::battery().map(|b| (b.percent, b.charging))
        );
        cpal_names(&devices);
        // From another thread while the main thread runs its loop, as a command of the app's does: the screens' names
        // come through the main queue.
        let worker = std::thread::spawn(move || timed(|| backend.devices()));
        while !worker.is_finished() {
            cidre::cf::RunLoop::run_in_mode(cidre::cf::RunLoopMode::default(), 0.02, true);
        }
        let (devices, took) = worker.join().expect("the worker thread");
        report("another thread", &devices, took);
    }

    fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
        let t = Instant::now();
        let out = f();
        (out, t.elapsed())
    }

    fn report(from: &str, d: &Devices, took: Duration) {
        let named = d.displays.iter().filter(|s| !s.name.is_empty()).count();
        let sizes: Vec<_> = d
            .displays
            .iter()
            .map(|s| (s.width, s.height, s.refresh_hz))
            .collect();
        println!("devices() from the {from} in {took:?}");
        println!(
            "  displays: {} (named {named}), main first: {sizes:?}",
            d.displays.len()
        );
        let transports = |list: &[klipolo_capture::backend::DeviceInfo]| -> Vec<Transport> {
            list.iter().map(|x| x.transport).collect()
        };
        println!(
            "  cameras: {} {:?}, in use {}, suspended {}",
            d.cameras.len(),
            transports(&d.cameras),
            d.cameras.iter().filter(|c| c.in_use).count(),
            d.cameras.iter().filter(|c| c.suspended).count()
        );
        println!(
            "  mics: {} {:?}, virtual {}; output {:?}",
            d.mics.len(),
            transports(&d.mics),
            d.mics.iter().filter(|m| m.is_virtual).count(),
            d.output.as_ref().map(|o| o.transport)
        );
        println!(
            "  apps: {}, games {}, frontmost {}",
            d.apps.len(),
            d.apps.iter().filter(|a| a.is_game).count(),
            d.apps.iter().filter(|a| a.frontmost).count()
        );
    }

    /// Every mic id is a name cpal lists, since the microphone finds its device by name (names only; nothing opens).
    fn cpal_names(d: &Devices) {
        let names: Vec<String> = cpal::default_host()
            .devices()
            .map(|all| all.filter_map(|x| x.name().ok()).collect())
            .unwrap_or_default();
        let found = d.mics.iter().filter(|m| names.contains(&m.id)).count();
        println!("mic ids cpal finds by name: {found}/{}", d.mics.len());
    }
}
