//! The first-run probe on macOS (`klipolo --record-worker --probe`): each hardware H.264 and HEVC encoder (at
//! most four, the dedicated ones first) measured on generated frames only, never capture, within the budget (≤ 5 s),
//! by `probe_plan`'s schedule: every encoder's 1080p60 paced (the rate it holds and its p95
//! latency, at Smooth's bitrate), then every encoder's 1440p flat out (`max_fps_1440p`, the headroom for the display's
//! own size), then on Apple silicon every dedicated encoder's 1080p120 (Slow motion), then every dedicated encoder's
//! 1440p60 paced (Sharp) unless its flat-out run already showed it.
//!
//! A run that wouldn't fit in what is left of the budget is skipped (its rate stays 0, so `probe::decide` never picks
//! it). The rate is the encoder's output rate (packets over the time between the first and the last), so a slow start
//! or the last frame's latency doesn't count against it. Memory: the generated frames (4 at 1080p, 12 MB; 4 at 1440p,
//! 22 MB) and one session at a time, all freed at the end.

use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};

use cidre::{arc, cv};
use klipolo_platform::clock::host_ns;
use klipolo_platform::qos::{self, Qos};

use super::encode::{EncoderSpec, VtEncoder};
use super::encoders::{EncoderInfo, hardware_encoders};
use super::frames::generated_nv12;
use crate::backend::{EncoderResult, PresetId, ProbeReport};
use crate::pipe::PACKET_CHANNEL;
use crate::platform::common::probe_plan::{self, Measure, PER_RUN, RESERVE};
use crate::platform::common::vt_ids::{dedicated_on_mac, engine_on_mac};
use crate::preset::preset;

/// Encoders measured at most.
const MAX_ENCODERS: usize = 4;

pub(super) fn run(budget: Duration) -> ProbeReport {
    let started = Instant::now();
    let until = started + budget.saturating_sub(RESERVE);
    qos::set_current(Qos::UserInteractive);
    let arch = klipolo_platform::caps::current().os.cpu_arch;
    let mut encoders = hardware_encoders();
    encoders.sort_by_key(|e| !dedicated_on_mac(&e.id, &arch));
    encoders.truncate(MAX_ENCODERS);
    let mut results: Vec<EncoderResult> = (encoders.iter())
        .map(|e| EncoderResult {
            id: e.id.clone(),
            codec: e.codec,
            hardware: true,
            dedicated: dedicated_on_mac(&e.id, &arch),
            engine: engine_on_mac(&e.id, &arch).to_owned(),
            fps_1080p60: 0.0,
            fps_1440p60: 0.0,
            fps_1080p120: 0.0,
            max_fps_1440p: 0.0,
            latency_p95_ms: 0.0,
        })
        .collect();
    let dedicated: Vec<bool> = results.iter().map(|r| r.dedicated).collect();
    let plan = probe_plan::plan(&dedicated, &arch, budget);
    let hd = generated_nv12(1920, 1080, 4);
    let mut qhd: Option<Vec<arc::R<cv::PixelBuf>>> = None;
    for step in &plan {
        let (e, r) = (&encoders[step.encoder], &mut results[step.encoder]);
        let at = |id| preset(id, e.codec).bitrate_bps;
        let (window, smooth, sharp, slow) = (
            step.window,
            at(PresetId::Smooth),
            at(PresetId::Sharp),
            at(PresetId::SlowMo),
        );
        match step.measure {
            Measure::Hd60 => {
                if let Some(m) = measure(e, &hd, 60, smooth, Pace::Paced(window), until) {
                    (r.fps_1080p60, r.latency_p95_ms) = (m.fps, m.p95_ms);
                }
            }
            Measure::QhdFlatOut => {
                let qhd = qhd.get_or_insert_with(|| generated_nv12(2560, 1440, 4));
                if let Some(m) = measure(e, qhd, 60, sharp, Pace::FlatOut(window), until) {
                    r.max_fps_1440p = m.fps;
                }
            }
            Measure::Hd120 => {
                if let Some(m) = measure(e, &hd, 120, slow, Pace::Paced(window), until) {
                    r.fps_1080p120 = m.fps;
                }
            }
            Measure::Qhd60 if probe_plan::infer_1440p60(r.max_fps_1440p) => r.fps_1440p60 = 60.0,
            Measure::Qhd60 => {
                let qhd = qhd.get_or_insert_with(|| generated_nv12(2560, 1440, 4));
                if let Some(m) = measure(e, qhd, 60, sharp, Pace::Paced(window), until) {
                    r.fps_1440p60 = m.fps;
                }
            }
        }
    }
    for r in &results {
        tracing::info!(?r, "probe: measured");
    }
    ProbeReport {
        encoders: results,
        took_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    }
}

#[derive(Clone, Copy)]
enum Pace {
    /// One frame every 1/fps, for this long.
    Paced(Duration),
    /// Frames as fast as the encoder takes them, for this long.
    FlatOut(Duration),
}

struct Measured {
    fps: f32,
    p95_ms: f32,
}

/// One run of `e` on `frames` (all one size) at `fps`; None when it doesn't fit before `until` or the encoder fails.
fn measure(
    e: &EncoderInfo,
    frames: &[arc::R<cv::PixelBuf>],
    fps: u32,
    bitrate_bps: u64,
    pace: Pace,
    until: Instant,
) -> Option<Measured> {
    let (window, paced) = match pace {
        Pace::Paced(d) => (d, true),
        Pace::FlatOut(d) => (d, false),
    };
    let first = frames.first()?;
    let made = Instant::now();
    if made + window + PER_RUN > until {
        tracing::info!(
            id = e.id,
            ?window,
            "probe: a run that no longer fits is skipped"
        );
        return None;
    }
    let spec = EncoderSpec {
        encoder_id: e.id.clone(),
        codec: e.codec,
        width: first.width() as u32,
        height: first.height() as u32,
        fps,
        bitrate_bps,
    };
    let (tx, rx) = sync_channel(PACKET_CHANNEL);
    let (events, _) = sync_channel(4);
    let mut vt = match VtEncoder::new(&spec, tx, events) {
        Ok(vt) => vt,
        Err(error) => {
            tracing::warn!(id = e.id, %error, "probe: the encoder didn't start");
            return None;
        }
    };
    if !paced {
        vt.flat_out();
    }
    let (start, start_ns) = (Instant::now(), host_ns());
    let setup_ms = start.duration_since(made).as_millis();
    let frame_ns = 1_000_000_000 / u64::from(fps.max(1));
    let mut n = 0u64;
    while start.elapsed() < window {
        if paced {
            let due = start + Duration::from_nanos(n * frame_ns);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        let frame = &frames[(n as usize) % frames.len()];
        vt.encode(frame, start_ns + n * frame_ns, false).ok()?;
        rx.try_iter().for_each(drop);
        n += 1;
    }
    let p95_ms = vt.latency_ms(0.95);
    let finishing = Instant::now();
    vt.finish();
    rx.try_iter().for_each(drop);
    let finish_ms = finishing.elapsed().as_millis();
    tracing::info!(
        id = e.id,
        setup_ms,
        finish_ms,
        "probe: a run's session costs"
    );
    let (outputs, first_ns, last_ns) = vt.meter();
    let span_ns = last_ns.saturating_sub(first_ns?);
    let rate = if outputs > 1 && span_ns > 0 {
        (outputs - 1) as f64 * 1e9 / span_ns as f64
    } else {
        0.0
    };
    // Paced, the encoder can't be seen going faster than it was fed; the last frames' catch-up mustn't read as more.
    let rate = if paced {
        rate.min(f64::from(fps))
    } else {
        rate
    };
    Some(Measured {
        fps: rate as f32,
        p95_ms,
    })
}
