//! The child side of editing copies: `Klipolo --proxy-worker <job folder>/proxy.json` (like export). It owns its
//! own runtime, GPU device and VideoToolbox sessions, so a crash or a stuck encoder can't take the app down. It runs
//! at utility QoS and holds no power activity (nobody waits for it, so it never keeps the computer awake).
//!
//! The path per frame: the source decoded on the hardware path at full size, drawn (and scaled) by the compositor as
//! a one-layer plan at the copy's size, converted to NV12 on the GPU into the encoder's IOSurfaces
//! (zero-copy) and encoded by VideoToolbox with `power_efficient` (short GOP, no B-frames, about 0.1 bpp, no sound).
//! Each 10 s segment has its own encoder session (the next one opens while this one encodes), so a finished segment
//! is closed, fsynced and verified before the run moves on. Once a second the same decode also draws the filmstrip's
//! tile for that second (`Fill::Full`). Then the segments are joined by stream copy and renamed into the cache.
//!
//! It stops after the current frame on `pause` (stdin) and when its parent is gone (stdin or stdout closed), keeping
//! every finished segment; between segments it also pauses when the computer runs hot or the disk is almost full.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use klipolo_model::project::FormatKind;

use super::job::{CopyRecord, JOINED_FILE, ProxyJob, ProxyJobHandle};
use super::protocol::ProxyLine;
use super::run::{
    FREE_FLOOR, FailCode, PauseReason, ProxyEnv, RenderError, RunOutcome, Segments,
    missing_message, record_outcome, run_segments, source_problem, unwritable_message,
};
use crate::export::encode::{EncoderSetup, EncoderThread, Input, Opened, VideoIn};
use crate::export::job::PartStatus;
use crate::export::plumbing::{LOCK_WAIT, arm_deadline, listen, lock_job, parent_gone};
use crate::export::protocol::{ErrorCode, Lines};
use crate::export::quality::{AudioCodec, Container, EncodeSettings, VideoCodec};
use crate::export::segment::{SegmentLedger, SegmentPlan};
use crate::export::shared::{Control, WorkerError};
use crate::media::sprite::{PER_CHUNK, SLOT_US, Sprite, chunk_of, tile_size};
use crate::media::thumbs::{Fill, chunk_path, write_chunk};
use crate::plan::{BackgroundPlan, FramePlan, Layer, SourceRef, VideoLayer, VideoRole};
use crate::platform::encode::{CodecRole, background_options, candidates};
use crate::render::GpuBackend;
use crate::render::export::{ExportFailure, FormatTargets, FrameDone};
use crate::render::yuv::{PlaneLayout, Yuv, YuvPixels};
use crate::session::{PreviewBackend, RenderRequest};

/// Progress lines at most this often.
const PROGRESS_EVERY_MS: u128 = 250;
/// Filmstrip tiles are drawn at this many times their size and box-averaged down on the CPU: a 4K source is about 30×
/// its 72 px tile, and one draw that small turns thin lines into dots.
const TILE_SUPERSAMPLE: usize = 4;

pub fn main(args: &[String]) -> i32 {
    let _ = ffmpeg::init();
    let Some(path) = args.first() else {
        eprintln!("usage: --proxy-worker <proxy.json>");
        return 2;
    };
    arm_deadline();
    // Utility for every thread this process starts (decoders, encoders, the runtime's): nobody waits for a copy.
    crate::qos::set_ceiling(crate::qos::Qos::Utility);
    crate::qos::set_current(crate::qos::Qos::Utility);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("klipolo-proxy")
        .on_thread_start(|| crate::qos::set_current(crate::qos::Qos::Utility))
        .build();
    match runtime {
        Ok(rt) => rt.block_on(run(Path::new(path))),
        Err(_) => 2,
    }
}

async fn run(job_path: &Path) -> i32 {
    let mut out = Lines::stdout();
    let dir = job_path.parent().unwrap_or(Path::new("."));
    // One worker per job: a resume that starts while an old worker winds down waits for it.
    let _lock = match lock_job(dir, LOCK_WAIT).await {
        Ok(lock) => lock,
        Err(_) => {
            let _ = out.send(&ProxyLine::Failed {
                message: "This editing copy is already being made.".into(),
                missing: false,
                code: FailCode::Other,
            });
            return 2;
        }
    };
    let control = Arc::new(Control::default());
    listen(control.clone());
    // No `power::begin` here: it would keep the computer awake for work nobody waits for.
    match run_job(dir, &mut out, control.clone()).await {
        RunOutcome::Finished => 0,
        RunOutcome::Paused(_) if control.stopped() => 3,
        RunOutcome::Paused(_) => 0,
        RunOutcome::Failed { .. } => 2,
    }
}

/// A line to the parent; a closed stdout means the parent is gone (the run stops after the current frame).
fn send<W: Write>(out: &mut Lines<W>, line: &ProxyLine, control: &Control) {
    if !control.stopped() && out.send(line).is_err() {
        parent_gone(control);
    }
}

/// Makes (or continues) the copy in `dir` and tells `out` how it went, ending with `Done`, `Paused` or `Failed`.
/// In-process tests call this with their own `Lines` and `Control`.
pub async fn run_job<W: Write>(
    dir: &Path,
    out: &mut Lines<W>,
    control: Arc<Control>,
) -> RunOutcome {
    let outcome = make(dir, out, &control).await;
    let line = match &outcome {
        Ok(path) => ProxyLine::Done { path: path.clone() },
        Err(RunOutcome::Paused(reason)) => ProxyLine::Paused { reason: *reason },
        Err(RunOutcome::Failed {
            message,
            missing,
            code,
        }) => ProxyLine::Failed {
            message: message.clone(),
            missing: *missing,
            code: *code,
        },
        Err(RunOutcome::Finished) => {
            unreachable!("`make` returns the copy's path when it finishes")
        }
    };
    send(out, &line, &control);
    match outcome {
        Ok(_) => RunOutcome::Finished,
        Err(outcome) => {
            // Why it stopped, for the app after a restart (the segment loop has saved its own ends already).
            if let Ok(mut job) = ProxyJob::open(dir) {
                record_outcome(&mut job, &outcome);
            }
            outcome
        }
    }
}

fn failed(message: impl Into<String>) -> RunOutcome {
    RunOutcome::failed(FailCode::Other, message)
}

async fn make<W: Write>(
    dir: &Path,
    out: &mut Lines<W>,
    control: &Arc<Control>,
) -> Result<PathBuf, RunOutcome> {
    let started = Instant::now();
    let mut job = ProxyJob::open(dir).map_err(failed)?;
    // An earlier run delivered the copy, then stopped before removing its job folder.
    if job.out.is_file() {
        let _ = std::fs::remove_dir_all(&job.dir);
        return Ok(job.out.clone());
    }
    let Some(choice) = candidates(CodecRole::H264).first() else {
        return Err(failed(
            "This computer has no H.264 encoder for editing copies.",
        ));
    };
    // What earlier runs finished must still be there; the last one is verified again. One that isn't is made again.
    let redo = tokio::task::block_in_place(|| job.recheck());
    if !redo.is_empty() {
        eprintln!("proxy: segments {redo:?} are made again");
    }
    // A joined copy that fails its check had a damaged segment that the cheap resume check can't see (bytes changed at
    // the same size): every segment is verified in full once, the damaged ones are made again and it joins again.
    let mut verified_all = false;
    let path = loop {
        if job.next_pending().is_some() {
            render_pending(&mut job, choice, out, control).await?;
        }
        // The segments must be of the source as it is now: one edited after the last segment (or before a finished job
        // was resumed) would deliver an old version's copy.
        if let Some(outcome) = source_problem(&job) {
            return Err(outcome);
        }
        match tokio::task::block_in_place(|| deliver(&job)) {
            Ok(path) => break path,
            Err(Delivery::Stopped(outcome)) => return Err(outcome),
            Err(Delivery::CheckFailed(message)) if !verified_all => {
                verified_all = true;
                let redo = tokio::task::block_in_place(|| job.verify_all());
                eprintln!(
                    "proxy: the joined copy failed its check ({message}); segments {redo:?} are made again"
                );
                if redo.is_empty() {
                    return Err(failed(format!(
                        "The editing copy couldn't be finished: {message}"
                    )));
                }
            }
            Err(Delivery::CheckFailed(message)) => {
                return Err(failed(format!(
                    "The editing copy couldn't be finished: {message}"
                )));
            }
        }
    };
    refill_tiles(&job, &path, out, control).await;
    eprintln!(
        "proxy done {:.2} s ({} frames {}×{})",
        started.elapsed().as_secs_f64(),
        job.total_frames,
        job.width,
        job.height
    );
    Ok(path)
}

/// Draws the filmstrip chunks the runs left without a full fill (`Tiles::missing`) from the finished copy, which seeks
/// fast (a keyframe every half second). A run that resumed inside a chunk can't fill it (its first tiles came before
/// the pause), and a tile that wasn't drawn drops its chunk: without this their keyframe tiles would stay. A
/// pause, or a failure here, leaves those chunks as they are: the copy is finished either way.
async fn refill_tiles<W: Write>(
    job: &ProxyJob,
    copy: &Path,
    out: &mut Lines<W>,
    control: &Arc<Control>,
) {
    let Some(mut tiles) = Tiles::new(job) else {
        return;
    };
    let missing = tiles.missing();
    if missing.is_empty() || !control.running() {
        return;
    }
    let started = Instant::now();
    let mut backend = match GpuBackend::for_export().await {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("proxy: thumbnails not refilled: {e}");
            return;
        }
    };
    let mut plan = copy_plan(job);
    if let Some(Layer::Video(v)) = plan.layers.first_mut() {
        // The copy: upright, on its own frame grid, the source's shape.
        let file: Arc<str> = Arc::from(copy.to_string_lossy().as_ref());
        v.source.path = file.clone();
        v.source.original = file;
        v.source.fps = job.rate.f64();
        v.source.rotation = Some(0);
        (v.source.width, v.source.height) = (job.width, job.height);
    }
    let (w, h) = tiles.draw_size();
    let mut drawn = 0;
    for chunk in missing {
        let first = u64::from(chunk) * u64::from(PER_CHUNK);
        let last = tiles.last_slot.min(first + u64::from(PER_CHUNK) - 1);
        for slot in first..=last {
            if !control.running() || !tiles.wants(slot) {
                break;
            }
            let t = (slot as i64 * SLOT_US).min(job.duration_us - 1).max(0);
            plan.at_us = t;
            if let Some(Layer::Video(v)) = plan.layers.first_mut() {
                v.source.time_us = t;
            }
            let request = RenderRequest::rgba(plan.clone(), w, h);
            let rendered = match PreviewBackend::render(&mut backend, request).await {
                Ok(rendered) => rendered.finish().await,
                Err(e) => Err(e),
            };
            match rendered {
                Ok(rgba) => {
                    drawn += 1;
                    if let Some(chunk) = tiles.put(slot, &rgba.bytes, rgba.stride as usize) {
                        send(out, &ProxyLine::Thumbs { chunk }, control);
                    }
                }
                Err(e) => {
                    eprintln!("proxy: thumbnail tile {slot} not drawn from the copy: {e}");
                    tiles.sprite = None;
                    break;
                }
            }
        }
    }
    eprintln!(
        "proxy thumbnails refilled from the copy: {drawn} tiles in {:.2} s",
        started.elapsed().as_secs_f64()
    );
}

/// Renders the job's pending segments on a new GPU pipeline (`run_segments`); anything but finishing them ends the run.
async fn render_pending<W: Write>(
    job: &mut ProxyJob,
    choice: &crate::platform::encode::EncoderChoice,
    out: &mut Lines<W>,
    control: &Arc<Control>,
) -> Result<(), RunOutcome> {
    if !Path::new(&job.source).is_file() {
        let message = missing_message(&job.source);
        return Err(RunOutcome::failed(FailCode::Missing, message));
    }
    let backend = GpuBackend::for_export().await.map_err(failed)?;
    let total = job.total_frames;
    send(
        out,
        &ProxyLine::Progress {
            done: job.frames_done(),
            total,
        },
        control,
    );
    let env = ProxyEnv::system();
    let mut segments = GpuSegments::new(job, backend, choice, control.clone(), out);
    let outcome = tokio::task::block_in_place(|| run_segments(job, &env, control, &mut segments));
    drop(segments);
    match outcome {
        RunOutcome::Finished => Ok(()),
        outcome => Err(outcome),
    }
}

/// Why a delivery didn't happen.
enum Delivery {
    /// A pause or failure that ends the run (a full disk, a cache that can't be written).
    Stopped(RunOutcome),
    /// The join or its check failed: a segment may be damaged.
    CheckFailed(String),
}

/// Joins the verified segments into `joined.mp4` (stream copy, index first), checks it, renames it to the job's
/// `out` (then syncs the folder) and removes the job folder.
fn deliver(job: &ProxyJob) -> Result<PathBuf, Delivery> {
    let bytes: u64 = job.segments.iter().map(|s| s.bytes).sum();
    let root = job.out.parent().unwrap_or(&job.dir).to_path_buf();
    // The joined file needs as much again as its segments.
    if klipolo_platform::durable::free_space(&job.dir).is_some_and(|free| free < FREE_FLOOR + bytes)
    {
        return Err(Delivery::Stopped(RunOutcome::Paused(PauseReason::DiskFull)));
    }
    let files: Vec<(PathBuf, u64)> = job
        .segments
        .iter()
        .map(|s| (job.path(&s.file), s.start_frame))
        .collect();
    let joined = job.path(JOINED_FILE);
    let started = Instant::now();
    crate::export::finalize::join_files(
        &files,
        None,
        &joined,
        Container::Mp4,
        job.rate,
        job.total_frames,
        &mut || {},
    )
    .map_err(|e| match e.code {
        ErrorCode::DiskFull => Delivery::Stopped(RunOutcome::Paused(PauseReason::DiskFull)),
        ErrorCode::Io => Delivery::Stopped(RunOutcome::failed(
            FailCode::CacheUnwritable,
            unwritable_message(&e.message),
        )),
        _ => Delivery::CheckFailed(e.message),
    })?;
    let stopped = |e: std::io::Error| {
        Delivery::Stopped(match RenderError::from_io(&e) {
            RenderError::DiskFull => RunOutcome::Paused(PauseReason::DiskFull),
            _ => RunOutcome::failed(
                FailCode::CacheUnwritable,
                unwritable_message(&e.to_string()),
            ),
        })
    };
    // What the copy is of, before it is in place: a copy without its record is never mapped.
    CopyRecord::of(job).write(&job.out).map_err(|e| {
        let _ = std::fs::remove_file(&joined);
        stopped(e)
    })?;
    std::fs::rename(&joined, &job.out).map_err(|e| {
        let _ = std::fs::remove_file(&joined);
        stopped(e)
    })?;
    // Its keyframe index beside it, from its sample table: a preview decoder of the copy never demuxes it whole
    // Without one, decoders build theirs as before.
    let indexed = crate::media::keyframes::from_sample_table(&job.out)
        .ok_or_else(|| std::io::Error::other("no complete sample table"))
        .and_then(|keys| crate::media::keyframes::write_beside(&job.out, &keys));
    if let Err(e) = indexed {
        eprintln!("proxy: the copy's keyframe index wasn't written: {e}");
    }
    if let Err(e) = klipolo_platform::durable::fsync_dir(&root) {
        eprintln!("proxy: the cache folder couldn't be synced: {e}");
    }
    eprintln!("proxy join {:.2} s", started.elapsed().as_secs_f64());
    if let Err(e) = std::fs::remove_dir_all(&job.dir) {
        eprintln!("proxy: the job folder couldn't be removed: {e}");
    }
    Ok(job.out.clone())
}

/// The copy's one-layer frame: the source filling the canvas (turned upright by the compositor).
fn copy_plan(job: &ProxyJob) -> FramePlan {
    let full = crate::plan::framing::FULL;
    FramePlan {
        format: FormatKind::Landscape,
        canvas: (job.width, job.height),
        at_us: 0,
        background: BackgroundPlan::Solid { color: [0, 0, 0] },
        layers: vec![Layer::Video(VideoLayer {
            role: VideoRole::Main,
            id: Arc::from("copy"),
            source: SourceRef {
                asset_id: Arc::from(job.stem.as_str()),
                path: Arc::from(job.source.as_str()),
                original: Arc::from(job.source.as_str()),
                time_us: 0,
                fps: job.source_fps,
                rotation: None,
                width: job.source_width,
                height: job.source_height,
            },
            dest: full,
            crop: full,
            rotation: 0.0,
            opacity: 1.0,
            radius: 0.0,
            mirror: false,
            border: false,
            look: None,
            effects: Vec::new(),
            pad_px: 0.0,
            blend: klipolo_model::motion::Blend::Normal,
            reach: 1.0,
        })],
        missing: Vec::new(),
    }
}

/// The thumbnail chunk being refilled from the copy's decode (one tile per second).
struct Tiles {
    root: PathBuf,
    stem: String,
    tile: (u32, u32),
    /// The chunk being filled; None when this run can't fill the current chunk from its first slot.
    sprite: Option<Sprite>,
    /// The last slot the file has.
    last_slot: u64,
    /// The averaged tile's RGB and its 4:2:0 planes (scratch, reused).
    rgb: Vec<[u8; 3]>,
    planes: [Vec<u8>; 3],
}

impl Tiles {
    fn new(job: &ProxyJob) -> Option<Self> {
        let stem = job.thumbs_stem.clone()?;
        Some(Self {
            root: job.thumbs_root.clone(),
            stem,
            tile: tile_size(job.source_width, job.source_height),
            sprite: None,
            last_slot: (job.rate.frame_start_us(job.total_frames.saturating_sub(1)) / SLOT_US)
                as u64,
            rgb: Vec::new(),
            planes: Default::default(),
        })
    }

    /// The chunks the source has (`0..=chunk_of(last_slot)`) that have no full fill (`.f`) yet: a stat each.
    fn missing(&self) -> Vec<u32> {
        (0..=chunk_of(self.last_slot))
            .filter(|&c| !chunk_path(&self.root, &self.stem, c, Fill::Full).exists())
            .collect()
    }

    /// Whether `slot`'s tile is wanted: its chunk is being filled, or starts here and isn't refilled yet.
    fn wants(&mut self, slot: u64) -> bool {
        let chunk = chunk_of(slot);
        if self.sprite.as_ref().is_some_and(|s| s.chunk == chunk) {
            return true;
        }
        self.sprite = None;
        let first = slot.is_multiple_of(u64::from(PER_CHUNK));
        if first && !chunk_path(&self.root, &self.stem, chunk, Fill::Full).exists() {
            self.sprite = Some(Sprite::new(chunk, self.tile));
        }
        self.sprite.is_some()
    }

    /// The size the tile is drawn at (`TILE_SUPERSAMPLE` times the tile).
    fn draw_size(&self) -> (u32, u32) {
        let k = TILE_SUPERSAMPLE as u32;
        (self.tile.0 * k, self.tile.1 * k)
    }

    /// Puts `slot`'s tile (RGBA rows `stride` bytes apart, at `draw_size`) and writes the chunk once it is full.
    /// Returns the chunk written.
    fn put(&mut self, slot: u64, rgba: &[u8], stride: usize) -> Option<u32> {
        let (w, h) = (self.tile.0 as usize, self.tile.1 as usize);
        box_average(rgba, stride, (w, h), TILE_SUPERSAMPLE, &mut self.rgb);
        rgb_to_jpeg_yuv(&self.rgb, (w, h), &mut self.planes);
        let sprite = self.sprite.as_mut()?;
        let planes = [
            &self.planes[0][..],
            &self.planes[1][..],
            &self.planes[2][..],
        ];
        sprite.put(slot, planes, [w, w / 2, w / 2]);
        let full =
            slot % u64::from(PER_CHUNK) == u64::from(PER_CHUNK) - 1 || slot >= self.last_slot;
        if !full {
            return None;
        }
        let mut sprite = self.sprite.take()?;
        sprite.fill_holes();
        match write_chunk(
            &klipolo_platform::durable::Os,
            &self.root,
            &self.stem,
            &sprite,
            Fill::Full,
        ) {
            Ok(()) => Some(sprite.chunk),
            Err(e) => {
                // Thumbnails are a nicety: the copy goes on (a full disk pauses it between segments anyway).
                eprintln!("proxy: thumbnail chunk {} not written: {e}", sprite.chunk);
                None
            }
        }
    }
}

/// Averages `factor × factor` blocks of opaque RGBA (rows `stride` bytes apart) into `w × h` RGB pixels.
fn box_average(
    rgba: &[u8],
    stride: usize,
    (w, h): (usize, usize),
    factor: usize,
    out: &mut Vec<[u8; 3]>,
) {
    out.clear();
    let n = (factor * factor) as u32;
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 3];
            for dy in 0..factor {
                let row = (y * factor + dy) * stride;
                for dx in 0..factor {
                    let at = row + (x * factor + dx) * 4;
                    if let Some(p) = rgba.get(at..at + 3) {
                        for c in 0..3 {
                            sum[c] += u32::from(p[c]);
                        }
                    }
                }
            }
            out.push(sum.map(|v| ((v + n / 2) / n) as u8));
        }
    }
}

/// RGB to full-range BT.601 4:2:0, JPEG's own colours (the sprite's format), into `planes` (16-bit fixed point).
fn rgb_to_jpeg_yuv(rgb: &[[u8; 3]], (w, h): (usize, usize), planes: &mut [Vec<u8>; 3]) {
    let (cw, ch) = (w / 2, h / 2);
    planes[0].resize(w * h, 16);
    planes[1].resize(cw * ch, 128);
    planes[2].resize(cw * ch, 128);
    let px = |x: usize, y: usize| rgb.get(y * w + x).map_or([0; 3], |p| p.map(i32::from));
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = px(x, y);
            planes[0][y * w + x] =
                ((19_595 * r + 38_470 * g + 7_471 * b + 32_768) >> 16).clamp(0, 255) as u8;
        }
    }
    for y in 0..ch {
        for x in 0..cw {
            let mut sum = [0i32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = px(2 * x + dx, 2 * y + dy);
                for c in 0..3 {
                    sum[c] += p[c];
                }
            }
            let [r, g, b] = sum.map(|v| (v + 2) / 4);
            planes[1][y * cw + x] = (((-11_059 * r - 21_709 * g + 32_768 * b + 32_768) >> 16) + 128)
                .clamp(0, 255) as u8;
            planes[2][y * cw + x] =
                (((32_768 * r - 27_439 * g - 5_329 * b + 32_768) >> 16) + 128).clamp(0, 255) as u8;
        }
    }
}

/// Where a segment's time went (logged per segment).
#[derive(Default)]
struct Timing {
    open_wait: f64,
    draw: f64,
    tiles: f64,
    gpu_wait: f64,
    queue: f64,
    finish: f64,
}

/// The worker's GPU pipeline: one encoder session per segment, frames drawn and handed over zero-copy.
struct GpuSegments<'a, W: Write> {
    rt: tokio::runtime::Handle,
    backend: GpuBackend,
    targets: Option<FormatTargets>,
    layout: Option<PlaneLayout>,
    ledger: ProxyJobHandle,
    setup: EncoderSetup,
    /// The next segment's encoder, opened while this one encodes.
    next: Option<(u32, EncoderThread, Opened)>,
    plan: FramePlan,
    pixels: Vec<YuvPixels>,
    tiles: Option<Tiles>,
    control: Arc<Control>,
    out: &'a mut Lines<W>,
    sent: Instant,
    config: Option<u32>,
}

impl<'a, W: Write> GpuSegments<'a, W> {
    fn new(
        job: &ProxyJob,
        backend: GpuBackend,
        choice: &crate::platform::encode::EncoderChoice,
        control: Arc<Control>,
        out: &'a mut Lines<W>,
    ) -> Self {
        let settings = EncodeSettings {
            codec: VideoCodec::H264High,
            container: Container::Mp4,
            bitrate: job.bitrate,
            max_bitrate: job.bitrate / 2 * 3,
            b_frames: 0,
            // A keyframe every half second: scrubbing decodes at most half a second.
            gop: ((job.rate.f64() / 2.0).round() as u32).max(1),
            audio: AudioCodec::Aac192,
        };
        Self {
            rt: tokio::runtime::Handle::current(),
            backend,
            targets: None,
            layout: None,
            ledger: ProxyJobHandle::new(job),
            setup: EncoderSetup {
                output: 0,
                settings,
                software: false,
                size: (job.width, job.height),
                rate: job.rate,
                segment_frames: job.segment_frames,
                options: background_options(choice),
            },
            next: None,
            plan: copy_plan(job),
            pixels: Vec::with_capacity(1),
            tiles: Tiles::new(job),
            control,
            out,
            sent: Instant::now(),
            config: None,
        }
    }

    /// Starts segment `index`'s encoder (it opens on its own thread).
    fn start_encoder(
        &self,
        job: &ProxyJob,
        index: u32,
    ) -> Result<(EncoderThread, Opened), RenderError> {
        let s = &job.segments[index as usize];
        let plan = SegmentPlan {
            index,
            start_frame: s.start_frame,
            frames: s.frames,
            path: self.ledger.segment_path(0, index),
        };
        let ledger: Arc<dyn SegmentLedger> = Arc::new(self.ledger.clone());
        EncoderThread::start(ledger, self.setup, vec![plan]).map_err(worker_error)
    }

    /// The source time proxy frame `n` shows.
    fn set_time(&mut self, job: &ProxyJob, n: u64) {
        let t = job.rate.frame_start_us(n).min(job.duration_us - 1).max(0);
        self.plan.at_us = t;
        if let Some(Layer::Video(v)) = self.plan.layers.first_mut() {
            v.source.time_us = t;
        }
    }

    async fn render_segment(
        &mut self,
        job: &ProxyJob,
        index: u32,
    ) -> Result<(u64, u32), RenderError> {
        let s = job.segments[index as usize].clone();
        let mut timing = Timing::default();
        let at = Instant::now();
        let (encoder, opened) = match self.next.take() {
            Some((i, encoder, opened)) if i == index => (encoder, opened),
            _ => self.start_encoder(job, index)?,
        };
        let mut encoder = Some(encoder);
        match opened.await {
            Ok(Ok(config)) => self.config = Some(config),
            Ok(Err(e)) => return Err(worker_error(e)),
            Err(_) => {
                return Err(RenderError::Failed(
                    "the encoder stopped while opening".into(),
                ));
            }
        }
        timing.open_wait = at.elapsed().as_secs_f64();
        if self.targets.is_none() {
            let target = self
                .backend
                .format_target(
                    (job.width, job.height),
                    Yuv::Nv12,
                    cfg!(target_os = "macos"),
                )
                .map_err(RenderError::Failed)?;
            self.layout = Some(target.layout());
            self.targets = Some(FormatTargets::new(vec![target]));
        }
        let layout = self.layout.expect("made with the targets");
        let started = Instant::now();
        // Frames drawn and submitted, waiting for their GPU work before the encoder takes them (two in flight).
        let mut in_flight: VecDeque<(u64, FrameDone, YuvPixels)> = VecDeque::with_capacity(3);
        let end = s.start_frame + s.frames;
        let next_at = s.start_frame + s.frames / 2;
        for n in s.start_frame..end {
            if !self.control.running() {
                // Dropping the encoder abandons the open segment and deletes its file.
                drop(encoder.take());
                return Err(RenderError::Stopped);
            }
            if n == next_at {
                self.open_next(job, index);
            }
            self.set_time(job, n);
            let at = Instant::now();
            let targets = self.targets.as_mut().expect("made above");
            let done = self
                .backend
                .render_formats(std::slice::from_ref(&self.plan), targets, &mut self.pixels)
                .await
                .map_err(|f| export_failure(f, job))?;
            let frame = self
                .pixels
                .pop()
                .ok_or_else(|| RenderError::Failed("no frame was drawn".into()))?;
            timing.draw += at.elapsed().as_secs_f64();
            // A frame that starts a new second draws that second's tile (the copy's rate is the source's, fractional
            // or not).
            let slot = (job.rate.frame_start_us(n) / SLOT_US) as u64;
            if n == 0 || slot != (job.rate.frame_start_us(n - 1) / SLOT_US) as u64 {
                let at = Instant::now();
                self.tile(slot).await;
                timing.tiles += at.elapsed().as_secs_f64();
            }
            in_flight.push_back((n, done, frame));
            if in_flight.len() >= 2 {
                let (n, done, frame) = in_flight.pop_front().expect("two in flight");
                let enc = encoder.as_mut().expect("open while frames are drawn");
                deliver_frame(enc, n, done, frame, layout, &mut timing).await?;
            }
            self.progress(job, n + 1 - s.start_frame);
        }
        while let Some((n, done, frame)) = in_flight.pop_front() {
            let enc = encoder.as_mut().expect("open while frames are drawn");
            deliver_frame(enc, n, done, frame, layout, &mut timing).await?;
        }
        // Flushes VideoToolbox, closes the segment and waits for its check (fsync, frame count, duration, CRC).
        let at = Instant::now();
        encoder
            .take()
            .expect("still open")
            .finish()
            .map_err(worker_error)?;
        timing.finish = at.elapsed().as_secs_f64();
        let verified = self
            .ledger
            .take(index)
            .ok_or_else(|| RenderError::Failed(format!("segment {index} wasn't verified")))?;
        self.config = Some(verified.config);
        let frames = s.frames as f64;
        let ms = |v: f64| v * 1000.0 / frames.max(1.0);
        eprintln!(
            "proxy segment {index}: {} frames, {:.1} fps; open wait {:.2} s, finish {:.2} s; ms/frame draw {:.1}, tiles {:.2}, GPU wait {:.1}, encoder queue {:.1}",
            s.frames,
            frames / started.elapsed().as_secs_f64().max(1e-3),
            timing.open_wait,
            timing.finish,
            ms(timing.draw),
            ms(timing.tiles),
            ms(timing.gpu_wait),
            ms(timing.queue),
        );
        Ok(verified.file)
    }

    /// Opens the next pending segment's encoder now, so its session is ready when this segment ends.
    fn open_next(&mut self, job: &ProxyJob, index: u32) {
        let next = index + 1;
        let pending = job
            .segments
            .get(next as usize)
            .is_some_and(|s| s.status != PartStatus::Done);
        if pending && self.next.is_none() {
            match self.start_encoder(job, next) {
                Ok((encoder, opened)) => self.next = Some((next, encoder, opened)),
                Err(e) => eprintln!("proxy: the next encoder didn't start early: {e:?}"),
            }
        }
    }

    /// Draws second `slot`'s filmstrip tile from the same decode (the decoders' caches hold the frame) and puts it.
    async fn tile(&mut self, slot: u64) {
        let Some(tiles) = self.tiles.as_mut() else {
            return;
        };
        if !tiles.wants(slot) {
            return;
        }
        let (w, h) = tiles.draw_size();
        let request = RenderRequest::rgba(self.plan.clone(), w, h);
        let rendered = match PreviewBackend::render(&mut self.backend, request).await {
            Ok(rendered) => rendered.finish().await,
            Err(e) => Err(e),
        };
        let chunk = match rendered {
            Ok(rgba) => tiles.put(slot, &rgba.bytes, rgba.stride as usize),
            Err(e) => {
                eprintln!("proxy: thumbnail tile {slot} not drawn: {e}");
                tiles.sprite = None;
                None
            }
        };
        if let Some(chunk) = chunk {
            send(self.out, &ProxyLine::Thumbs { chunk }, &self.control);
        }
    }

    fn progress(&mut self, job: &ProxyJob, in_segment: u64) {
        if self.sent.elapsed().as_millis() >= PROGRESS_EVERY_MS {
            self.sent = Instant::now();
            let line = ProxyLine::Progress {
                done: (job.frames_done() + in_segment).min(job.total_frames),
                total: job.total_frames,
            };
            send(self.out, &line, &self.control);
        }
    }
}

impl<W: Write> Segments for GpuSegments<'_, W> {
    fn render(&mut self, job: &ProxyJob, index: u32) -> Result<(u64, u32), RenderError> {
        let rt = self.rt.clone();
        // `run_segments` runs inside `block_in_place`: blocking on the GPU pipeline here is allowed.
        rt.block_on(self.render_segment(job, index))
    }

    fn encoder_config(&self) -> Option<u32> {
        self.config
    }
}

/// Waits for `frame`'s GPU work, then hands it to the encoder (waiting while its queue is full).
async fn deliver_frame(
    encoder: &mut EncoderThread,
    index: u64,
    done: FrameDone,
    frame: YuvPixels,
    layout: PlaneLayout,
    timing: &mut Timing,
) -> Result<(), RenderError> {
    let at = Instant::now();
    done.wait().await.map_err(RenderError::Failed)?;
    let queued = Instant::now();
    timing.gpu_wait += (queued - at).as_secs_f64();
    let video = match frame {
        #[cfg(target_os = "macos")]
        YuvPixels::Surface(buffer) => VideoIn::Surface(buffer),
        YuvPixels::Readback(mut pending) => {
            VideoIn::Planes(pending.wait().await.map_err(RenderError::Failed)?, layout)
        }
    };
    encoder
        .push(Input { index, video })
        .await
        .map_err(worker_error)?;
    timing.queue += queued.elapsed().as_secs_f64();
    Ok(())
}

/// An encoder thread's error as a reason to pause or fail:
/// - the encoder itself (VideoToolbox couldn't start a session, or failed encoding: usually another app holding it, or
///   an export) pauses as busy and is retried, never falling back to software;
/// - a full disk pauses until space frees up;
/// - a segment file that can't be written or checked (`Io`: read-only, no permission, a failing drive) fails with the
///   OS's reason and isn't retried by itself;
/// - anything else fails.
fn worker_error(e: WorkerError) -> RenderError {
    match e.code {
        ErrorCode::EncoderUnavailable | ErrorCode::Encode => RenderError::EncoderBusy,
        ErrorCode::DiskFull => RenderError::DiskFull,
        ErrorCode::Io => RenderError::CacheUnwritable(e.message),
        _ => RenderError::Failed(e.message),
    }
}

/// A frame that couldn't be drawn: a source gone missing fails by name; anything else fails with the reason.
fn export_failure(f: ExportFailure, job: &ProxyJob) -> RenderError {
    match f {
        ExportFailure::Decode(_) if !Path::new(&job.source).is_file() => {
            RenderError::Missing(missing_message(&job.source))
        }
        ExportFailure::Decode(message)
        | ExportFailure::Content(message)
        | ExportFailure::Gpu(message) => RenderError::Failed(message),
        // An editing copy draws its source alone: never over a budget, but said by its code if it were.
        ExportFailure::Frame(coded) => RenderError::Failed(coded.code),
        // Its disk didn't answer within a minute: never called missing.
        ExportFailure::NotAnswering(path) => {
            RenderError::Failed(crate::render::pool::not_answering(&path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run resumed inside a chunk can't fill it, so the chunks without a full fill are drawn from the
    /// finished copy (`refill_tiles`); a chunk already refilled isn't drawn again.
    #[test]
    fn chunks_without_a_full_fill_are_found_for_the_refill() {
        use super::super::decide::ProxyPolicy;
        use super::super::facts::SourceFacts;
        use crate::platform::gpu::GpuClass;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("talk.mp4");
        std::fs::write(&src, b"a stand-in for a video").unwrap();
        let facts = SourceFacts {
            width: 1920,
            height: 1080,
            rate: crate::export::shared::FrameRate::whole(30),
            codec: "h264".into(),
            ten_bit: false,
            ten_bit_hw: false,
            hdr: false,
            max_key_interval_us: 2_000_000,
            variable_rate: false,
            hw_decodes: true,
            // 100 s: slots 0–99, chunks 0 and 1.
            duration_us: 100_000_000,
            start_us: 0,
            id: None,
        };
        let policy = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
        let job = ProxyJob::create(
            &dir.path().join("media/proxies"),
            &src.to_string_lossy(),
            &facts,
            &policy,
        )
        .unwrap();
        let mut tiles = Tiles::new(&job).expect("the source has a thumbnails stem");
        assert_eq!(tiles.missing(), [0, 1]);
        // Chunk 0 refilled by the run; chunk 1 started after a resume at 70 s, so the run never filled it.
        let full = |chunk| chunk_path(&tiles.root, &tiles.stem, chunk, Fill::Full);
        std::fs::create_dir_all(full(0).parent().unwrap()).unwrap();
        std::fs::write(full(0), b"jpeg").unwrap();
        assert!(
            !tiles.wants(70),
            "a chunk is only started at its first slot"
        );
        assert_eq!(tiles.missing(), [1]);
        assert!(
            tiles.wants(64),
            "the refill starts chunk 1 at its first slot"
        );
    }

    #[test]
    fn only_the_encoder_itself_is_busy() {
        let error = |code| WorkerError {
            code,
            message: "writing a segment: Read-only file system".into(),
            retryable: true,
        };
        assert_eq!(
            worker_error(error(ErrorCode::EncoderUnavailable)),
            RenderError::EncoderBusy
        );
        assert_eq!(
            worker_error(error(ErrorCode::Encode)),
            RenderError::EncoderBusy
        );
        assert_eq!(
            worker_error(error(ErrorCode::DiskFull)),
            RenderError::DiskFull
        );
        assert_eq!(
            worker_error(error(ErrorCode::Io)),
            RenderError::CacheUnwritable("writing a segment: Read-only file system".into())
        );
        assert!(matches!(
            worker_error(error(ErrorCode::Internal)),
            RenderError::Failed(_)
        ));
    }

    #[test]
    fn a_tile_is_averaged_and_turned_into_jpeg_colours() {
        // A 2 × 2 tile drawn 4× (8 × 8): the left half red, the right half alternating black and white columns
        // (which average to mid grey, where one sample per pixel would pick black or white).
        let (w, h, k) = (2usize, 2usize, 4usize);
        let stride = w * k * 4;
        let mut rgba = vec![255u8; stride * h * k];
        for y in 0..h * k {
            for x in 0..w * k {
                let p = &mut rgba[y * stride + x * 4..y * stride + x * 4 + 3];
                let colour = if x < k {
                    [255, 0, 0]
                } else if x % 2 == 0 {
                    [0, 0, 0]
                } else {
                    [255, 255, 255]
                };
                p.copy_from_slice(&colour);
            }
        }
        let (mut rgb, mut planes) = (Vec::new(), <[Vec<u8>; 3]>::default());
        box_average(&rgba, stride, (w, h), k, &mut rgb);
        assert_eq!(
            rgb,
            [[255, 0, 0], [128, 128, 128], [255, 0, 0], [128, 128, 128]]
        );
        rgb_to_jpeg_yuv(&rgb, (w, h), &mut planes);
        // JPEG's BT.601 full range: red is Y 76, grey 128 has Y 128.
        assert_eq!(planes[0], [76, 128, 76, 128]);
        // Chroma of the 2 × 2 block: half red, half grey.
        let (cb, cr) = (planes[1][0], planes[2][0]);
        assert!(
            (cb as i32 - 106).abs() <= 1 && (cr as i32 - 192).abs() <= 1,
            "{cb} {cr}"
        );
    }
}
