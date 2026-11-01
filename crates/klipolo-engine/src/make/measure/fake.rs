//! The CI meter: a frame's GPU time is its drawn pixels × a fixed cost a pixel, plus, with `per_trip`, a cost per
//! loop trip a pixel runs at the frame's settings (the host's counter read by `audit::trips_per_pixel` once per settings,
//! colours and points included, and size), so measurements are deterministic on any GPU, lavapipe included. It draws
//! nothing else; a baseline frame costs nothing. Every frame it times is logged (`log`), for the tests to read what was
//! measured where. With `quantised` it times as a GPU whose timestamps aren't trusted and whose fence wait polls
//! (as on an Intel Mac): a submission's time is its fixed cost plus its frames', rounded up to the poll's step,
//! so work that ends within one step adds nothing to a single frame.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::BoxFuture;

use super::{FrameReq, Meter, Pcts, Sample};
use crate::headless::Headless;
use crate::make::audit::{AuditDraw, trips_per_pixel};
use crate::make::envelope::numeric;
use crate::plan::pack_params;
use crate::registry::ShaderKind;

/// One frame the fake meter timed.
#[derive(Clone, Debug, PartialEq)]
pub struct Metered {
    pub size: (u32, u32),
    /// The layer's settings as numbers (`envelope::numeric`).
    pub values: BTreeMap<String, f64>,
    /// At half size.
    pub half: bool,
}

pub struct FakeMeter {
    ns_per_pixel: f64,
    ns_per_trip: f64,
    /// Real time each timed frame takes (tests of the time budget).
    delay: std::time::Duration,
    /// A size whose frames take this many ms whatever the pixels (tests of the export stop).
    at_size: Option<((u32, u32), f64)>,
    /// The cost a pixel reaches at the layer's end, rising evenly from `ns_per_pixel` at its start (a cost that grows
    /// over the layer).
    ns_at_end: Option<f64>,
    /// A size whose frames fail (tests of the step a failure names).
    fail_at: Option<(u32, u32)>,
    /// What the first frame at a size and scale costs more when it wasn't warmed first (a real GPU's pipeline compile
    /// and new targets), and the sizes and scales warmed this request.
    first_frame_ms: f64,
    warmed: HashSet<((u32, u32), bool)>,
    /// From this timed frame of a request on (1-based), each takes this much real time too.
    delay_from: Option<(usize, std::time::Duration)>,
    timed: usize,
    /// The effect targets' peak a frame reports: at full size, at half size.
    peaks: (u64, u64),
    /// A scene's CPU time a frame it reports (µs), whatever was timed.
    scene_cpu_us: f64,
    /// Every size a frame was drawn at, baselines and warm-ups included.
    touched: Arc<Mutex<Vec<(u32, u32)>>>,
    trips: HashMap<(String, (u32, u32)), f64>,
    log: Arc<Mutex<Vec<Metered>>>,
    /// Fenced, quantised timing: the poll's step and a submission's fixed cost, in ms.
    quantum: Option<(f64, f64)>,
    /// Every `n`-th batch of more than one frame takes this many ms more (another app's work on the GPU).
    batch_stall: Option<(usize, f64)>,
    stalls: usize,
    /// Every copy of a batch encodes one pass fewer than a single frame does.
    copies_skip_a_pass: bool,
    /// Every batch of more than one frame: its frames, and the GPU time they keep the GPU busy, in ms.
    batches: Arc<Mutex<Vec<(u32, f64)>>>,
}

impl FakeMeter {
    pub fn new(ns_per_pixel: f64) -> Self {
        Self {
            ns_per_pixel,
            ns_per_trip: 0.0,
            delay: std::time::Duration::ZERO,
            at_size: None,
            ns_at_end: None,
            fail_at: None,
            first_frame_ms: 0.0,
            warmed: HashSet::new(),
            delay_from: None,
            timed: 0,
            peaks: (0, 0),
            scene_cpu_us: 40.0,
            touched: Arc::default(),
            trips: HashMap::new(),
            log: Arc::default(),
            quantum: None,
            batch_stall: None,
            stalls: 0,
            copies_skip_a_pass: false,
            batches: Arc::default(),
        }
    }

    /// Times as a GPU whose timestamps aren't trusted and whose fence wait polls every `step_ms`: a
    /// submission of `k` frames that cost `c` ms each takes `step_ms × ⌈(fixed_ms + k·c) / step_ms⌉`, a baseline frame
    /// (no pass) `fixed_ms` rounded up the same way.
    pub fn quantised(mut self, step_ms: f64, fixed_ms: f64) -> Self {
        self.quantum = Some((step_ms, fixed_ms));
        self
    }

    /// Every `every`-th batch of more than one frame takes `ms` more, as when another app's work holds the GPU.
    pub fn batch_stall(mut self, every: usize, ms: f64) -> Self {
        self.batch_stall = Some((every, ms));
        self
    }

    /// Every copy of a batch encodes one pass fewer than a single frame (all alike): a batch the meter mustn't take a slope
    /// from, as a picture every copy reused would make it.
    pub fn copies_skip_a_pass(mut self) -> Self {
        self.copies_skip_a_pass = true;
        self
    }

    /// Every batch of more than one frame so far: its frames, and the GPU time they keep the GPU busy (shared).
    pub fn batches(&self) -> Arc<Mutex<Vec<(u32, f64)>>> {
        self.batches.clone()
    }

    /// A submission's time: `busy` ms of frames, rounded up to the poll's step after the fixed cost when quantised.
    fn submitted(&self, busy: f64) -> f64 {
        match self.quantum {
            Some((step, fixed)) => step * ((fixed + busy) / step - 1e-9).ceil(),
            None => busy,
        }
    }

    /// Adds `ns` × the loop trips a pixel runs at the frame's settings.
    pub fn per_trip(mut self, ns: f64) -> Self {
        self.ns_per_trip = ns;
        self
    }

    /// Each timed frame also takes `delay` of real time, as a real GPU's would.
    pub fn delay(mut self, delay: std::time::Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Frames at `size` take `ms`, whatever their pixels.
    pub fn at_size(mut self, size: (u32, u32), ms: f64) -> Self {
        self.at_size = Some((size, ms));
        self
    }

    /// A pixel's cost rises evenly over the layer, from `ns_per_pixel` at its start to `ns` at its end.
    pub fn rising(mut self, ns: f64) -> Self {
        self.ns_at_end = Some(ns);
        self
    }

    /// Frames at `size` fail, as a lost readback would.
    pub fn fail_at(mut self, size: (u32, u32)) -> Self {
        self.fail_at = Some(size);
        self
    }

    /// The first frame at each size and scale (full or half) costs `ms` more unless it was warmed (`Meter::warm`)
    /// first this request, as a real GPU's first frame pays for its pipelines and targets.
    pub fn first_frame(mut self, ms: f64) -> Self {
        self.first_frame_ms = ms;
        self
    }

    /// From the `n`-th timed frame of a request on (1-based), each takes `delay` of real time too: the time runs out at
    /// a frame the test chooses.
    pub fn delay_from(mut self, n: usize, delay: std::time::Duration) -> Self {
        self.delay_from = Some((n, delay));
        self
    }

    /// The effect targets' peak each frame reports, at full size and at half size.
    pub fn peaks(mut self, full: u64, half: u64) -> Self {
        self.peaks = (full, half);
        self
    }

    /// A scene's CPU time a frame, in µs (40 unless set: Light).
    pub fn scene_cpu_us(mut self, us: f64) -> Self {
        self.scene_cpu_us = us;
        self
    }

    /// Every size a frame was drawn at, baselines and warm-ups included (shared, as `log`).
    pub fn touched(&self) -> Arc<Mutex<Vec<(u32, u32)>>> {
        self.touched.clone()
    }

    /// Every frame timed so far (shared: the test keeps it while the worker owns the meter).
    pub fn log(&self) -> Arc<Mutex<Vec<Metered>>> {
        self.log.clone()
    }
}

impl FakeMeter {
    /// `frames` copies of `req` in one submission: their time, and the peak they report.
    async fn timed(
        &mut self,
        h: &mut Headless,
        req: &FrameReq,
        frames: u32,
    ) -> Result<Sample, String> {
        self.touched
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(req.size);
        if req.baseline {
            // No pass of the layer's: the submission's own cost alone, held up as the layer's is.
            let gpu_ms = if self.quantum.is_some() {
                self.submitted(0.0)
            } else {
                0.0
            };
            return Ok(Sample {
                gpu_ms: gpu_ms + self.batched(frames, 0.0),
                ..Sample::default()
            });
        }
        self.timed += 1;
        if let Some((n, delay)) = self.delay_from
            && self.timed >= n
        {
            tokio::time::sleep(delay).await;
        }
        let layer = &req.layer;
        if self.fail_at == Some(req.size) {
            return Err("the fake meter's frame failed".into());
        }
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Metered {
                size: req.size,
                values: numeric(&layer.def, &layer.params),
                half: req.half,
            });
        let mut pixels = f64::from(req.size.0) * f64::from(req.size.1);
        if layer.kind == ShaderKind::Generator && (req.half || layer.half_always()) {
            pixels /= 4.0;
        }
        let mut ns = match self.ns_at_end {
            Some(end) => {
                let p = (req.at_us as f64 / 1e6 / layer.seconds.max(1e-6)).clamp(0.0, 1.0);
                self.ns_per_pixel + (end - self.ns_per_pixel) * p
            }
            None => self.ns_per_pixel,
        };
        if self.ns_per_trip > 0.0
            && let Some(shader) = layer.shader()
        {
            let key = (format!("{:?}", layer.params), req.size);
            let trips = match self.trips.get(&key) {
                Some(t) => *t,
                None => {
                    let draw = AuditDraw {
                        effect: shader,
                        params: pack_params(shader, &layer.params),
                        size: req.size,
                        at: (req.at_us as f64 / 1e6) as f32,
                        duration: layer.seconds as f32,
                        cache: Some(&layer.audit),
                    };
                    let t = trips_per_pixel(h.device(), &draw).await?;
                    self.trips.insert(key, t);
                    t
                }
            };
            ns += self.ns_per_trip * trips;
        }
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let mut one = match self.at_size {
            Some((size, ms)) if size == req.size => ms,
            _ => pixels * ns / 1e6,
        };
        if self.warmed.insert((req.size, req.half)) {
            one += self.first_frame_ms;
        }
        let busy = one * f64::from(frames);
        let gpu_ms = self.submitted(busy) + self.batched(frames, busy);
        Ok(Sample {
            gpu_ms,
            cpu_us: 0.0,
            peak_bytes: if req.half { self.peaks.1 } else { self.peaks.0 },
            uneven: false,
            // The layer's pass and the frame's own; a batch's copies one fewer when they skip one.
            passes: if frames > 1 && self.copies_skip_a_pass {
                1
            } else {
                2
            },
        })
    }
}

impl FakeMeter {
    /// A batch of more than one frame, keeping the GPU `busy` ms, is logged; what a stall adds to it (`batch_stall`).
    fn batched(&mut self, frames: u32, busy: f64) -> f64 {
        if frames <= 1 {
            return 0.0;
        }
        self.batches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((frames, busy));
        match self.batch_stall {
            Some((every, ms)) => {
                self.stalls += 1;
                if self.stalls.is_multiple_of(every) {
                    ms
                } else {
                    0.0
                }
            }
            None => 0.0,
        }
    }
}

impl Meter for FakeMeter {
    fn frame<'a>(
        &'a mut self,
        h: &'a mut Headless,
        req: &'a FrameReq,
    ) -> BoxFuture<'a, Result<Sample, String>> {
        Box::pin(self.timed(h, req, 1))
    }

    fn batch<'a>(
        &'a mut self,
        h: &'a mut Headless,
        req: &'a FrameReq,
        frames: u32,
    ) -> BoxFuture<'a, Result<Sample, String>> {
        Box::pin(self.timed(h, req, frames.max(1)))
    }

    fn warm<'a>(
        &'a mut self,
        h: &'a mut Headless,
        req: &'a FrameReq,
    ) -> BoxFuture<'a, Result<(), String>> {
        let _ = h;
        self.warmed.insert((req.size, req.half));
        self.touched
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(req.size);
        Box::pin(async { Ok(()) })
    }

    fn begin(&mut self) {
        self.warmed.clear();
        self.timed = 0;
    }

    fn scene_cpu(&self, _timed: Pcts) -> Pcts {
        Pcts::flat(self.scene_cpu_us)
    }

    fn fenced(&self, h: &Headless) -> bool {
        let _ = h;
        self.quantum.is_some()
    }
}
