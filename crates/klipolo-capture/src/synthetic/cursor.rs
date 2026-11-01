//! `SyntheticCursor`: the fake backend's cursor source (`Backend::cursor_source`), following a `CursorScript` on its own
//! thread in real time: the area first, then positions on a circle, clicks, shape changes, and the failures the
//! watchdog is for (going silent, failing, switching to sampled clicks). Nothing here touches a device; it never asks
//! the consent guard (synthetic).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;

use klipolo_cursor::ClickSource;
use klipolo_platform::clock::host_ns;

use super::{Worker, wait_until};
use crate::cursor::{CursorEvent, CursorSource, ShapeImage};
use crate::pipe::PipeError;

/// What the synthetic cursor does. Times are seconds after its `start`.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorScript {
    /// Positions a second.
    pub hz: u32,
    /// One turn of the circle takes this long.
    pub turn_s: f64,
    /// A click (down, then up a position later) this often.
    pub click_every_s: Option<f64>,
    /// The shape changes this often, between two shapes (each sent whole once, then by its key).
    pub shape_every_s: Option<f64>,
    /// From here it sends nothing more (a monitor that stopped delivering).
    pub silent_after_s: Option<f64>,
    /// Here it sends `Failed` and stops.
    pub fail_after_s: Option<f64>,
    /// Here it sends `ClicksSampled` (once).
    pub degrade_after_s: Option<f64>,
    pub area_pts: (f32, f32),
    pub clicks: ClickSource,
}

impl Default for CursorScript {
    /// 60 positions a second around a 4 s circle, a click every 2 s, a shape change every 5 s, on a 1440 × 900 point
    /// display, with exact click events.
    fn default() -> Self {
        Self {
            hz: 60,
            turn_s: 4.0,
            click_every_s: Some(2.0),
            shape_every_s: Some(5.0),
            silent_after_s: None,
            fail_after_s: None,
            degrade_after_s: None,
            area_pts: (1440.0, 900.0),
            clicks: ClickSource::Events,
        }
    }
}

impl CursorScript {
    pub fn silent_after(self, s: f64) -> Self {
        Self {
            silent_after_s: Some(s),
            ..self
        }
    }

    pub fn fail_after(self, s: f64) -> Self {
        Self {
            fail_after_s: Some(s),
            ..self
        }
    }

    pub fn degrade_after(self, s: f64) -> Self {
        Self {
            degrade_after_s: Some(s),
            ..self
        }
    }
}

/// See the module notes.
pub struct SyntheticCursor {
    script: CursorScript,
    paused: Arc<AtomicBool>,
    worker: Option<Worker>,
}

impl SyntheticCursor {
    pub fn new(script: CursorScript) -> Self {
        Self {
            script,
            paused: Arc::default(),
            worker: None,
        }
    }
}

impl CursorSource for SyntheticCursor {
    fn start(&mut self, out: SyncSender<CursorEvent>) -> Result<(), PipeError> {
        if self.worker.is_some() {
            return Err(PipeError::Busy);
        }
        let (script, paused) = (self.script.clone(), self.paused.clone());
        self.worker = Some(Worker::spawn("fake-cursor", move |stop| {
            run(&script, &paused, stop, &out)
        }));
        Ok(())
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    fn stop(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.stop();
        }
    }

    fn clicks(&self) -> ClickSource {
        self.script.clicks
    }
}

/// The script, from now until `stop` (a full `out` drops, as a real source's).
fn run(
    script: &CursorScript,
    paused: &AtomicBool,
    stop: &AtomicBool,
    out: &SyncSender<CursorEvent>,
) {
    let start = host_ns();
    let (w_pts, h_pts) = script.area_pts;
    let _ = out.try_send(CursorEvent::Area {
        host_ns: start,
        w_pts,
        h_pts,
    });
    let step_ns = 1_000_000_000 / u64::from(script.hz.max(1));
    let past = |at_s: f64, s: Option<f64>| s.is_some_and(|s| at_s >= s);
    let (mut degraded, mut held, mut sent) = (false, false, [false; 2]);
    for k in 1u64.. {
        let due = start + k * step_ns;
        if !wait_until(due, stop) {
            return;
        }
        let at_s = (due - start) as f64 / 1e9;
        if past(at_s, script.fail_after_s) {
            let _ = out.try_send(CursorEvent::Failed);
            return;
        }
        if past(at_s, script.silent_after_s) {
            return;
        }
        if paused.load(Ordering::Relaxed) {
            continue;
        }
        if !degraded && past(at_s, script.degrade_after_s) {
            degraded = true;
            let _ = out.try_send(CursorEvent::ClicksSampled { host_ns: due });
        }
        let turn = (at_s / script.turn_s.max(0.01)).fract() * std::f64::consts::TAU;
        let (x, y) = (
            (0.5 + 0.3 * turn.cos()) as f32,
            (0.5 + 0.3 * turn.sin()) as f32,
        );
        let _ = out.try_send(CursorEvent::Move { host_ns: due, x, y });
        let every = |s: Option<f64>| {
            s.filter(|s| *s > 0.0)
                .map(|s| (s * script.hz as f64).round().max(1.0) as u64)
        };
        if held || every(script.click_every_s).is_some_and(|n| k.is_multiple_of(n)) {
            held = !held;
            let down = held;
            let _ = out.try_send(CursorEvent::Button {
                host_ns: due,
                btn: 0,
                down,
                x,
                y,
            });
        }
        if let Some(n) = every(script.shape_every_s).filter(|n| k.is_multiple_of(*n)) {
            let which = ((k / n) % 2) as usize;
            let key = which as u64 + 1;
            let ev = if std::mem::replace(&mut sent[which], true) {
                CursorEvent::Shape { host_ns: due, key }
            } else {
                CursorEvent::NewShape {
                    host_ns: due,
                    key,
                    image: shape(which),
                }
            };
            let _ = out.try_send(ev);
        }
    }
}

/// One of the two fake shapes: 8 × 8, a solid colour each, the hotspot at the top left.
fn shape(which: usize) -> ShapeImage {
    let colour = if which == 0 {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    };
    ShapeImage {
        px: (8, 8),
        rgba: colour.repeat(64),
        hotspot_px: (0.0, 0.0),
        pts: 8.0,
        sys: None,
    }
}
