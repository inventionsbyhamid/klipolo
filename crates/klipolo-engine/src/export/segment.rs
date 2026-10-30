//! One format's segment files. Packets from the format's one encoder session go to `seg-NNNNN` files by their
//! pts: GOPs are closed, so a segment's first packet is its keyframe and no later packet belongs to an earlier
//! segment. A segment that got all its frames is closed (trailer), then fsynced, probed (frame count and duration)
//! and checksummed on a helper thread while encoding goes on, and only then recorded as done (in its
//! `SegmentLedger`: the export's manifest, or an editing copy's). A partial one is deleted.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use ffmpeg::{Packet, Rational, format};

use super::shared::{FrameRate, WorkerError, checksum, file_error, internal, io_error};
use crate::vendor::cap::enc_ffmpeg::h264::StreamTemplate;

/// Where a run's verified segments are recorded: the export manifest, or an editing copy's.
pub trait SegmentLedger: Send + Sync + 'static {
    /// The segment's file (absolute).
    fn segment_path(&self, output: usize, index: u32) -> PathBuf;
    /// Records a written, fsynced and verified segment atomically; returns frames it set back to pending (0 when none).
    fn segment_done(
        &self,
        output: usize,
        index: u32,
        file: (u64, u32),
        encoder: (u32, bool),
    ) -> Result<u64, WorkerError>;
}

/// A segment this run renders.
#[derive(Clone, Debug)]
pub struct SegmentPlan {
    pub index: u32,
    pub start_frame: u64,
    pub frames: u64,
    pub path: PathBuf,
}

struct Open {
    plan: usize,
    ctx: format::context::Output,
    time_base: Rational,
    written: u64,
}

pub struct SegmentWriter {
    ledger: Arc<dyn SegmentLedger>,
    output: usize,
    container: &'static str,
    plans: Vec<SegmentPlan>,
    segment_frames: u64,
    rate: FrameRate,
    stream: StreamTemplate,
    /// The encoder's parameter-set CRC and whether it is the software one: recorded with each segment.
    encoder: (u32, bool),
    current: Option<Open>,
    verifying: Option<JoinHandle<Result<(), WorkerError>>>,
}

fn broken(message: &str) -> WorkerError {
    internal(format!("segment: {message}"))
}

impl SegmentWriter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ledger: Arc<dyn SegmentLedger>,
        output: usize,
        container: &'static str,
        plans: Vec<SegmentPlan>,
        segment_frames: u64,
        rate: FrameRate,
        stream: StreamTemplate,
        encoder: (u32, bool),
    ) -> Self {
        Self {
            ledger,
            output,
            container,
            plans,
            segment_frames,
            rate,
            stream,
            encoder,
            current: None,
            verifying: None,
        }
    }

    /// Whether frame `n` starts one of this run's segments (it is encoded as a keyframe).
    pub fn starts_segment(&self, n: u64) -> bool {
        self.plans.iter().any(|p| p.start_frame == n)
    }

    /// Writes one packet (encoder time base one frame, `rate.frame_base()`; pts = the frame's index on the timeline).
    pub fn write(&mut self, packet: &mut Packet) -> Result<(), WorkerError> {
        let pts = packet
            .pts()
            .ok_or_else(|| broken("a packet without a time"))?;
        let index = (pts.max(0) as u64 / self.segment_frames) as u32;
        let current = self.current.as_ref().map(|o| self.plans[o.plan].index);
        if current != Some(index) {
            if current.is_some_and(|c| index < c) {
                return Err(broken("a packet went back to an earlier segment"));
            }
            if !packet.is_key() {
                return Err(broken("a segment doesn't start with a keyframe"));
            }
            self.close()?;
            self.open(index)?;
        }
        let open = self.current.as_mut().expect("a segment is open");
        let start = self.plans[open.plan].start_frame as i64;
        packet.set_pts(Some(pts - start));
        packet.set_dts(packet.dts().map(|d| d - start));
        packet.set_duration(1);
        packet.set_stream(0);
        packet.rescale_ts(self.rate.frame_base(), open.time_base);
        packet
            .write(&mut open.ctx)
            .map_err(|e| file_error(e, "writing a segment"))?;
        open.written += 1;
        Ok(())
    }

    fn open(&mut self, index: u32) -> Result<(), WorkerError> {
        let plan = self
            .plans
            .iter()
            .position(|p| p.index == index)
            .ok_or_else(|| broken("a packet outside this run's segments"))?;
        let path = &self.plans[plan].path;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io_error(e, "the segment folder"))?;
        }
        let mut ctx = format::output_as(path, self.container)
            .map_err(|e| file_error(e, "creating a segment"))?;
        self.stream
            .add_to(&mut ctx)
            .map_err(|e| broken(&e.to_string()))?;
        // An exact fractional rate (an editing copy of a 23.976 source) on a time base it divides evenly (1/24000: 1001
        // ticks a frame), so every frame keeps its exact time; whole rates keep the encoder's 90 kHz.
        if self.rate.den > 1
            && let Some(mut stream) = ctx.stream_mut(0)
        {
            stream.set_time_base(Rational(1, self.rate.num as i32));
        }
        ctx.write_header()
            .map_err(|e| file_error(e, "starting a segment"))?;
        let time_base = ctx.stream(0).map_or(Rational(1, 90_000), |s| s.time_base());
        self.current = Some(Open {
            plan,
            ctx,
            time_base,
            written: 0,
        });
        Ok(())
    }

    /// Closes the open segment, which must have every frame, and verifies and records it on a helper thread.
    fn close(&mut self) -> Result<(), WorkerError> {
        let Some(mut open) = self.current.take() else {
            return Ok(());
        };
        let plan = self.plans[open.plan].clone();
        if open.written != plan.frames {
            let _ = std::fs::remove_file(&plan.path);
            return Err(broken(&format!(
                "segment {} has {} of {} frames",
                plan.index, open.written, plan.frames
            )));
        }
        open.ctx
            .write_trailer()
            .map_err(|e| file_error(e, "finishing a segment"))?;
        drop(open);
        self.join_verifier()?;
        let (ledger, output, rate, encoder) =
            (self.ledger.clone(), self.output, self.rate, self.encoder);
        let verifier = std::thread::Builder::new()
            .name("klipolo-export-verify".into())
            .spawn(move || {
                crate::qos::set_current(crate::qos::Qos::Utility);
                let file = verify(&plan.path, plan.frames, rate, None)?;
                ledger
                    .segment_done(output, plan.index, file, encoder)
                    .map(drop)
            })
            .map_err(internal)?;
        self.verifying = Some(verifier);
        Ok(())
    }

    fn join_verifier(&mut self) -> Result<(), WorkerError> {
        match self.verifying.take().map(JoinHandle::join) {
            None | Some(Ok(Ok(()))) => Ok(()),
            Some(Ok(Err(e))) => Err(e),
            Some(Err(_)) => Err(internal("the segment check crashed")),
        }
    }

    /// The end of this run's input: the open segment is closed when complete, else deleted. Waits for the checks.
    pub fn finish(&mut self) -> Result<(), WorkerError> {
        let complete = self
            .current
            .as_ref()
            .is_some_and(|o| o.written == self.plans[o.plan].frames);
        let closed = if complete {
            self.close()
        } else {
            self.discard();
            Ok(())
        };
        let verified = self.join_verifier();
        closed.and(verified)
    }

    /// Drops the open segment (no trailer) and deletes it; segments already closed are still recorded.
    pub fn abandon(&mut self) {
        self.discard();
        if let Err(e) = self.join_verifier() {
            tracing::warn!(message = %e.message, "segment check");
        }
    }

    fn discard(&mut self) {
        if let Some(open) = self.current.take() {
            let path = self.plans[open.plan].path.clone();
            drop(open);
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Fsyncs a finished file, checks it has `frames` video frames lasting `frames / rate` (and, when given, a CRC it
/// must match) and returns its size and CRC-32.
pub fn verify(
    path: &Path,
    frames: u64,
    rate: FrameRate,
    expect: Option<(u64, u32)>,
) -> Result<(u64, u32), WorkerError> {
    let file = std::fs::File::open(path).map_err(|e| io_error(e, "opening a segment"))?;
    if expect.is_none() {
        file.sync_all()
            .map_err(|e| io_error(e, "saving a segment"))?;
    }
    let bytes = file
        .metadata()
        .map_err(|e| io_error(e, "reading a segment"))?
        .len();
    drop(file);
    if let Some((size, _)) = expect
        && size != bytes
    {
        return Err(broken("size changed"));
    }
    let (count, seconds) = probe(path).ok_or_else(|| broken("unreadable"))?;
    let expected = rate.seconds(frames);
    if count != frames || (seconds - expected).abs() > 1.5 * rate.seconds(1) {
        return Err(broken(&format!(
            "{count} frames, {seconds:.3} s (expected {frames}, {expected:.3} s)"
        )));
    }
    let crc = checksum(path).map_err(|e| io_error(e, "reading a segment"))?;
    if let Some((_, want)) = expect
        && want != crc
    {
        return Err(broken("checksum changed"));
    }
    Ok((bytes, crc))
}

/// The video stream's frame count and duration (seconds), from the file's index (nothing is decoded).
pub fn probe(path: &Path) -> Option<(u64, f64)> {
    let input = crate::media::open::input(path).ok()?;
    let video = input.streams().best(ffmpeg::media::Type::Video)?;
    let tb = video.time_base();
    let seconds =
        video.duration() as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator().max(1));
    Some((video.frames().max(0) as u64, seconds))
}
