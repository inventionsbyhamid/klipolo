//! The worker shell's smoke run on a Mac: the real worker shell (menu-bar item, HUD, shortcuts while armed, the take's
//! power activity), driven by a scripted session, with no capture and no prompt. It prints technical facts only: this
//! process's own windows (count, level, size, on screen), whether the take's power assertion is held, timings and
//! memory; never another app's window, a name, or anything on screen. The shortcuts are registered for about two
//! seconds, while "armed". Run by hand, never in tests (the consent guard refuses under `KLIPOLO_TEST`):
//! `cargo run -p klipolo-capture --features real-capture --example shell_smoke`.
//!
//! `-- quit` instead asks AppKit to terminate mid-take, as the system's quit event does (logout, Activity
//! Monitor): the session should get the menu's Stop, and the process should end with the session's own code
//! (3 here, never AppKit's 0). `-- quit-stuck`: a session that never ends; the process ends with 1 after 10 s.

fn main() {
    #[cfg(target_os = "macos")]
    mac::run();
    #[cfg(not(target_os = "macos"))]
    println!("shell_smoke: macOS only");
}

#[cfg(target_os = "macos")]
mod mac {
    use std::collections::BTreeMap;
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use klipolo_capture::profile::Corner;
    use klipolo_capture::shell::{
        Flash, KeyBindings, Level, MenuAction, SessionEnd, ShellInput, ShellIo, ShellOutput,
        StatusView, WorkerState,
    };
    use klipolo_platform::caps::OsFamily;
    use objc2::runtime::AnyObject;
    use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID};
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, ns_string};

    pub fn run() {
        // The shell's own warnings (a shortcut that didn't register, a missing item) go to stderr.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(tracing::Level::INFO)
            .init();
        let backend = klipolo_capture::platform::backend();
        let (input, inputs) = sync_channel(64);
        let (to_shell, output) = sync_channel(64);
        let end = SessionEnd::default();
        let mode = std::env::args().nth(1).unwrap_or_default();
        let script = {
            let end = end.clone();
            std::thread::spawn(move || match mode.as_str() {
                "quit" | "quit-stuck" => quit_script(&to_shell, &inputs, &end, mode == "quit"),
                _ => script(&to_shell, &inputs),
            })
        };
        backend.shell().run(ShellIo {
            input,
            output,
            keys: KeyBindings::defaults(OsFamily::Macos),
            words: BTreeMap::new(),
            hud_corner: Corner::TopRight,
            hud_display: None,
            end,
            camera_bubble: false,
        });
        let quit_at = script.join().expect("the script");
        println!("run returned {:?} after Quit", quit_at.elapsed());
        println!("rss after the shell: {} KB", rss_kb());
    }

    fn script(to: &SyncSender<ShellOutput>, inputs: &Receiver<ShellInput>) -> Instant {
        let send = |out| to.send(out).expect("the shell is running");
        sleep(Duration::from_millis(600));
        report("started (the item, no HUD)");
        send(ShellOutput::Keys(true));
        send(status(WorkerState::Armed, 0, None, None));
        sleep(Duration::from_millis(800));
        report("armed (shortcuts registered)");
        for left in [3, 2, 1] {
            send(status(WorkerState::Countdown, 0, Some(left), Some(-30.0)));
            sleep(Duration::from_millis(300));
        }
        report("countdown from the app's Record (HUD stays hidden)");
        send(status(WorkerState::Recording, 0, None, Some(-20.0)));
        sleep(Duration::from_millis(300));
        println!("take activity while recording: {}", assertions());
        send(ShellOutput::Flash(Flash::Marker));
        let flashed = Instant::now();
        sleep(Duration::from_millis(400));
        report("0.4 s into a marker flash");
        for s in 1..=4 {
            send(status(WorkerState::Recording, s, None, Some(-20.0)));
            sleep(Duration::from_millis(500));
        }
        println!("{:?} after the flash:", flashed.elapsed());
        report("after the flash");
        send(ShellOutput::Keys(false));
        send(status(WorkerState::Stopping, 4, None, None));
        sleep(Duration::from_millis(300));
        println!("take activity while stopping: {}", assertions());
        send(status(WorkerState::Ready, 4, None, None));
        sleep(Duration::from_millis(300));
        println!("take activity once ready: {}", assertions());
        println!("rss with the item and HUD: {} KB", rss_kb());
        println!(
            "inputs while armed (shortcuts, clicks): {}",
            inputs.try_iter().count()
        );
        let quit_at = Instant::now();
        send(ShellOutput::Quit);
        quit_at
    }

    /// A take recording for a second, then AppKit's `terminate:` (the system quit's path), then the session's answer:
    /// it stops, finishes and ends with 3 (`ends`), or never ends.
    fn quit_script(
        to: &SyncSender<ShellOutput>,
        inputs: &Receiver<ShellInput>,
        end: &SessionEnd,
        ends: bool,
    ) -> Instant {
        let send = |out| to.send(out).expect("the shell is running");
        sleep(Duration::from_millis(600));
        send(status(WorkerState::Recording, 0, None, Some(-20.0)));
        sleep(Duration::from_millis(1_000));
        println!("take activity while recording: {}", assertions());
        let asked = Instant::now();
        cidre::dispatch::Queue::main().async_mut(|| {
            let mtm = objc2::MainThreadMarker::new().expect("the main queue");
            objc2_app_kit::NSApplication::sharedApplication(mtm).terminate(None);
        });
        let stop = inputs.recv_timeout(Duration::from_secs(3));
        println!(
            "the session got {stop:?} {:?} after the quit (want Ok(Menu(Stop)))",
            asked.elapsed()
        );
        assert_eq!(stop.ok(), Some(ShellInput::Menu(MenuAction::Stop)));
        if ends {
            send(status(WorkerState::Stopping, 1, None, None));
            sleep(Duration::from_millis(500));
            send(status(WorkerState::Ready, 1, None, None));
            send(ShellOutput::Quit);
            sleep(Duration::from_millis(100));
            println!(
                "the session ends with 3, {:?} after the quit",
                asked.elapsed()
            );
            end.set(3);
        }
        sleep(Duration::from_secs(30));
        println!("still running 30 s after the quit: the shell didn't end the process");
        asked
    }

    fn status(
        state: WorkerState,
        elapsed_s: i64,
        countdown_s: Option<u32>,
        mic_dbfs: Option<f32>,
    ) -> ShellOutput {
        ShellOutput::Status(StatusView {
            state,
            elapsed_us: elapsed_s * 1_000_000,
            lines: vec!["Screen: 60 fps".into(), "Mic".into()],
            level: Level::Ok,
            countdown_s,
            mic_dbfs,
            held: None,
            by_key: false,
            bubble: false,
        })
    }

    /// This process's windows on screen: (level, width, height) each. Only its own; nothing else is read.
    fn report(when: &str) {
        let own = i64::from(std::process::id());
        let Some(list) =
            CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, kCGNullWindowID)
        else {
            return println!("{when}: no window list");
        };
        // SAFETY: CFArray and CFDictionary are toll-free bridged to NSArray and NSDictionary; the list lives here.
        let list: &NSArray<NSDictionary<NSString, AnyObject>> =
            unsafe { &*(&*list as *const _ as *const NSArray<NSDictionary<NSString, AnyObject>>) };
        let number = |d: &NSDictionary<NSString, AnyObject>, key: &NSString| {
            (d.objectForKey(key)?.downcast_ref::<NSNumber>()).map(NSNumber::doubleValue)
        };
        let mut mine = Vec::new();
        for window in list.iter() {
            if number(&window, ns_string!("kCGWindowOwnerPID")).map(|p| p as i64) != Some(own) {
                continue;
            }
            let layer = number(&window, ns_string!("kCGWindowLayer")).unwrap_or(-1.0);
            let bounds = window.objectForKey(ns_string!("kCGWindowBounds"));
            let size = bounds.and_then(|b| {
                let b = b.downcast_ref::<NSDictionary>()?;
                let get = |key: &NSString| {
                    (b.objectForKey(key.as_ref())?.downcast_ref::<NSNumber>())
                        .map(NSNumber::doubleValue)
                };
                Some((get(ns_string!("Width"))?, get(ns_string!("Height"))?))
            });
            mine.push((layer, size));
        }
        println!(
            "{when}: {} window(s) of ours on screen {mine:?}",
            mine.len()
        );
    }

    /// The power assertions macOS lists for this process (`pmset -g assertions`, read only).
    fn assertions() -> String {
        let own = format!("pid {}(", std::process::id());
        let out = std::process::Command::new("/usr/bin/pmset")
            .args(["-g", "assertions"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let kinds: Vec<&str> = (out.lines())
            .filter(|l| l.contains(&own))
            .filter_map(|l| {
                ["PreventUserIdleDisplaySleep", "PreventUserIdleSystemSleep"]
                    .into_iter()
                    .find(|k| l.contains(k))
            })
            .collect();
        if kinds.is_empty() {
            "none".into()
        } else {
            kinds.join(", ")
        }
    }

    fn rss_kb() -> String {
        let pid = std::process::id().to_string();
        std::process::Command::new("/bin/ps")
            .args(["-o", "rss=", "-p", &pid])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default()
    }
}
