//! The cursor of a screen take on macOS: `MacCursorSource`, the `CursorSource` the
//! backend gives every screen target, started only inside a take the person started. ScreenCaptureKit leaves the
//! system cursor out of the frames for a separate cursor (`screen`'s `showsCursor`), and this source records it:
//! - **`monitor`:** a global `NSEvent` monitor with the mouse-only mask (no permission: Apple asks for trust only for
//!   key events; never an event tap, a prompt or a key) on the main queue. Each mouse event goes out at its own hardware
//!   time, mapped into the captured area as it is at that moment.
//! - **`sampler`** (a utility thread): the cursor's shapes (`shapes`, over `image` and `table`) and visibility, the area
//!   (`bounds`),
//!   a beat every 500 ms with nothing else sent, and the monitor's self-check (`platform::common::cursor_checks`).
//! - **`poll`:** the self-check's fallback, `mouseLocation` and `pressedMouseButtons` at 240 Hz (`clicks: sampled`).
//!
//! Every AppKit call off the main thread runs inside an autorelease pool (Cap's leak, #2023).

mod bounds;
mod check;
mod image;
mod monitor;
mod poll;
mod sampler;
mod shapes;
mod table;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;

use klipolo_cursor::ClickSource;
use klipolo_platform::clock::host_ns;
use objc2_app_kit::NSEvent;

use self::bounds::AreaCell;
use crate::backend::Target;
use crate::cursor::{CursorEvent, CursorSource};
use crate::guard::assert_real_capture_allowed;
use crate::pipe::PipeError;
use crate::platform::common::cursor_checks::Why;
use crate::platform::common::cursor_pace::top_left;

/// What the source's parts share: the area, the switches, and what was sent when.
#[derive(Debug, Default)]
struct Link {
    area: AreaCell,
    /// The main display's height in points, as f64 bits (AppKit's points flip against it).
    main_h: AtomicU64,
    paused: AtomicBool,
    stop: AtomicBool,
    /// The poll sends the positions and buttons from here; the monitor sends nothing.
    sampled: AtomicBool,
    /// Host ns of the last event anyone sent (the beat's rule) and of the monitor's last event (the move check).
    last_sent: AtomicU64,
    last_monitor: AtomicU64,
    /// Host ns of the main queue's last run of the sampler's block (the monitor's handler runs there too).
    main_alive: AtomicU64,
    /// The take's first and last monitor downs: (the event's own time, its time on the host clock), for the drift check
    /// (Step 5: the last click's file time minus its event time equals the first's within 1 ms).
    first_down: [AtomicU64; 2],
    last_down: [AtomicU64; 2],
    /// The monitor events' smallest lag (host ns at delivery minus their own time): the event clock against the host's.
    least_lag: AtomicI64,
    dropped: AtomicU64,
}

impl Link {
    /// `ev` to `out` without waiting; a full queue drops it, counted.
    fn send(&self, out: &SyncSender<CursorEvent>, ev: CursorEvent) {
        match out.try_send(ev) {
            Ok(()) => self.last_sent.store(host_ns(), Ordering::Release),
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn main_h(&self) -> f64 {
        f64::from_bits(self.main_h.load(Ordering::Relaxed))
    }

    fn set_main_h(&self, h: f64) {
        self.main_h.store(h.to_bits(), Ordering::Relaxed);
    }

    /// A monitor down: its own time `event_ns` and its host time `at_ns`, as the take's first (once) and last.
    fn down_seen(&self, event_ns: u64, at_ns: u64) {
        let first = &self.first_down;
        if first[1]
            .compare_exchange(0, at_ns, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            first[0].store(event_ns, Ordering::Release);
        }
        self.last_down[0].store(event_ns, Ordering::Relaxed);
        self.last_down[1].store(at_ns, Ordering::Relaxed);
    }

    /// Where the pointer is now (`NSEvent.mouseLocation`, needing no permission), in Core Graphics' global points.
    fn pointer(&self) -> (f64, f64) {
        let at = NSEvent::mouseLocation();
        top_left((at.x, at.y), self.main_h())
    }
}

/// A down the monitor delivered (its own time, its button), for the self-check.
type Down = (u64, u8);

/// This process's resident size in MB (the take's RSS check, Step 5: flat within 1 MB over 3 minutes).
fn resident_mb() -> f64 {
    // SAFETY: a plain struct of integers, filled by the call for its own flavour.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: our own pid, the flavour matching the struct.
    let ok = unsafe {
        libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V2, (&raw mut info).cast())
    };
    if ok == 0 {
        info.ri_resident_size as f64 / 1_048_576.0
    } else {
        0.0
    }
}

/// See the module notes.
pub struct MacCursorSource {
    target: Target,
    link: Arc<Link>,
    monitor: Option<monitor::Token>,
    sampler: Option<JoinHandle<()>>,
    /// `ClickSource` as 0 (events) or 1 (sampled): the sampler's to change.
    clicks: Arc<AtomicU8>,
}

impl MacCursorSource {
    /// Opens nothing: the monitor and the sampler wait for `start`. `clicks`: what this Mac's monitor gives
    /// (`CursorCaps::clicks`), until the self-check says otherwise.
    pub fn new(target: &Target, clicks: ClickSource) -> Result<Self, PipeError> {
        assert_real_capture_allowed("cursor");
        let clicks = u8::from(clicks != ClickSource::Events);
        Ok(Self {
            target: target.clone(),
            link: Arc::default(),
            monitor: None,
            sampler: None,
            clicks: Arc::new(AtomicU8::new(clicks)),
        })
    }
}

impl CursorSource for MacCursorSource {
    /// The area first, then the monitor (on the main queue) and the sampler. A monitor that can't be installed is the
    /// self-check's first reason: the take samples its clicks from the start.
    fn start(&mut self, out: SyncSender<CursorEvent>) -> Result<(), PipeError> {
        assert_real_capture_allowed("cursor");
        if self.sampler.is_some() {
            return Err(PipeError::Busy);
        }
        self.link.set_main_h(bounds::main_height());
        if let Some(area) = bounds::area_of(&self.target) {
            self.link.area.set(area);
            let (w_pts, h_pts) = (area.2 as f32, area.3 as f32);
            let host_ns = host_ns();
            self.link.send(
                &out,
                CursorEvent::Area {
                    host_ns,
                    w_pts,
                    h_pts,
                },
            );
        }
        let (downs, seen) = sync_channel::<Down>(64);
        let mut why = (self.clicks() != ClickSource::Events).then_some(Why::NoMonitor);
        if why.is_none() {
            match monitor::install(out.clone(), self.link.clone(), downs) {
                Ok(token) => self.monitor = Some(token),
                Err(e) => {
                    tracing::warn!(error = %e, "cursor: the mouse monitor couldn't be installed");
                    why = Some(Why::NoMonitor);
                }
            }
        }
        let parts = (self.link.clone(), seen, self.clicks.clone());
        match sampler::Sampler::new(out, self.target.clone(), parts).spawn(why) {
            Ok(handle) => self.sampler = Some(handle),
            Err(e) => {
                self.monitor = None;
                return Err(PipeError::Os(format!("the cursor's sampler: {e}")));
            }
        }
        tracing::info!(
            monitor = self.monitor.is_some(),
            "cursor: the macOS source started"
        );
        Ok(())
    }

    fn set_paused(&mut self, paused: bool) {
        self.link.paused.store(paused, Ordering::Release);
    }

    fn stop(&mut self) {
        self.link.stop.store(true, Ordering::Release);
        // Removed on the main queue; the handler sends nothing once `stop` is set.
        self.monitor = None;
        if let Some(sampler) = self.sampler.take()
            && sampler.join().is_err()
        {
            tracing::error!("cursor: the sampler panicked");
        }
    }

    fn clicks(&self) -> ClickSource {
        match self.clicks.load(Ordering::Acquire) {
            0 => ClickSource::Events,
            _ => ClickSource::Sampled,
        }
    }
}

impl Drop for MacCursorSource {
    fn drop(&mut self) {
        self.stop();
    }
}
