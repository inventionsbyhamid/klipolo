//! The perf harness's build timing: the caption and title build threads each keep a `BuildTimes`, off
//! unless the harness turns it on, since a timed build waits for the GPU (on its own thread, never the render thread).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

/// The most builds kept between two `take`s (older ones go).
const MAX_TIMES: usize = 4096;

/// One build, timed: the build thread's CPU until its submission, and from its submission until the GPU was done
/// (other work on the queue, a frame's, included).
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct BuildTime {
    pub cpu_ms: f64,
    pub gpu_ms: f64,
}

/// A build thread's timing switch and the builds it timed.
pub(crate) struct BuildTimes {
    on: AtomicBool,
    times: Mutex<Vec<BuildTime>>,
}

impl BuildTimes {
    pub(crate) const fn new() -> Self {
        Self {
            on: AtomicBool::new(false),
            times: Mutex::new(Vec::new()),
        }
    }

    /// Times every build from now on, or stops.
    pub(crate) fn set(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    /// The builds timed since the last call.
    pub(crate) fn take(&self) -> Vec<BuildTime> {
        std::mem::take(&mut *self.times.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// A build starts: when, if builds are timed.
    pub(crate) fn start(&self) -> Option<Instant> {
        self.on.load(Ordering::Relaxed).then(Instant::now)
    }

    /// The build that started at `started` has submitted its work: waits for the GPU and keeps its times (nothing when
    /// it wasn't timed).
    pub(crate) fn finish(&self, started: Option<Instant>, device: &wgpu::Device) {
        let Some(started) = started else {
            return;
        };
        let submitted = Instant::now();
        let _ = device.poll(wgpu::PollType::Wait);
        let time = BuildTime {
            cpu_ms: (submitted - started).as_secs_f64() * 1e3,
            gpu_ms: submitted.elapsed().as_secs_f64() * 1e3,
        };
        let mut times = self.times.lock().unwrap_or_else(PoisonError::into_inner);
        if times.len() == MAX_TIMES {
            times.remove(0);
        }
        times.push(time);
    }
}
