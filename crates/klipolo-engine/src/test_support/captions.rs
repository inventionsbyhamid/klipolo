//! Caption helpers for the engine's tests (doc-hidden): caption frames drawn through the preview's
//! renderer, through an export job and its worker's start-up, or through an export pass's own frame path, and an image
//! comparison (the caption drawer over a synthetic project is `caption_drawer`).

use std::sync::Arc;

use image::RgbaImage;
use klipolo_model::captions::captions;
use klipolo_model::project::{AssetKind, BackgroundKind, FormatKind, Project};
use klipolo_model::speech::Speech;
use klipolo_model::time::secs_to_us as secs;

use super::captions_project;
use crate::api::ExportQuality;
use crate::export::job::ExportJob;
use crate::export::naming::plan_files;
use crate::export::runner::Batch;
use crate::export::worker::fonts_before_frame_zero;
use crate::plan::{PlanContext, frame_plan};
use crate::present::Output;
use crate::registry::Registry;
use crate::render::GpuBackend;
use crate::render::captions::CaptionStats;
use crate::render::device::RenderDevice;
use crate::render::export::FormatTargets;
use crate::render::yuv::{Yuv, YuvPixels};
use crate::session::{PreviewBackend, RenderRequest};

/// `captions_project` whose voice file is sound only, so its frames draw nothing but the captions, over mid-grey: the
/// styles' black outlines and shadows show on it as well as their white and coloured words.
pub fn sound_only_captions_project(style: &str, words: &[(&str, f64, f64)]) -> (Project, Speech) {
    let (mut p, speech) = captions_project(style, words);
    for a in &mut p.assets {
        a.kind = AssetKind::Audio;
    }
    p.background.kind = BackgroundKind::Solid;
    p.background.color = "#808080".into();
    (p, speech)
}

/// A frame drawn by `backend` at `size` as an image.
async fn draw(
    backend: &mut GpuBackend,
    ctx: &PlanContext,
    t: f64,
    (w, h): (u32, u32),
) -> RgbaImage {
    let plan = frame_plan(ctx, secs(t), FormatKind::Portrait);
    let f = backend
        .render(RenderRequest {
            plan,
            width: w,
            height: h,
            output: Output::Rgba,
        })
        .await
        .expect("a frame")
        .finish()
        .await
        .expect("its pixels");
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    let mut img = RgbaImage::new(w, h);
    for (y, row) in img.rows_mut().enumerate() {
        let from = y * f.stride as usize;
        for (x, px) in row.enumerate() {
            let at = from + x * 4;
            px.0.copy_from_slice(&f.bytes[at..at + 4]);
        }
    }
    img
}

/// A 9:16 project with `style`'s captions over `words` (text, start s, end s) on mid-grey, its portrait frame at `t` s
/// drawn at `size` by the session preview's renderer (`GpuBackend`, waiting for its assets as the `render_plan` helper
/// does).
pub async fn render_caption_frame(
    style: &str,
    words: &[(&str, f64, f64)],
    t: f64,
    size: (u32, u32),
) -> RgbaImage {
    let mut frames = render_caption_frames(style, words, &[t], size).await;
    frames.pop().expect("one frame")
}

/// `render_caption_frame` at each of `times` (s), every frame drawn by one backend: its caption drawer is kept from
/// frame to frame, uploading only what moved.
pub async fn render_caption_frames(
    style: &str,
    words: &[(&str, f64, f64)],
    times: &[f64],
    size: (u32, u32),
) -> Vec<RgbaImage> {
    let (p, speech) = sound_only_captions_project(style, words);
    let pages = Arc::new(captions(&p, &speech));
    let ctx = PlanContext::new(Arc::new(p)).with_captions(pages);
    let mut backend = GpuBackend::new(crate::render::mode::RenderMode::Contained)
        .await
        .expect("a GPU")
        .wait_for_assets();
    let mut frames = Vec::with_capacity(times.len());
    for &t in times {
        frames.push(draw(&mut backend, &ctx, t, size).await);
    }
    frames
}

/// The caption drawer's stats after each frame of `style` over `words` at `times` (s), drawn by an export pass's own
/// frame path: one export renderer drawing into one NV12 format through
/// `GpuBackend::render_formats` (`Compositor::render_yuv`), read back as a SoftwareSafe export's frames are, each
/// frame's pixels waited for as its encoder would.
pub async fn caption_stats_via_export_pass(
    style: &str,
    words: &[(&str, f64, f64)],
    times: &[f64],
    size: (u32, u32),
) -> Vec<CaptionStats> {
    let (p, speech) = sound_only_captions_project(style, words);
    let pages = Arc::new(captions(&p, &speech));
    let ctx = PlanContext::new(Arc::new(p)).with_captions(pages);
    let dev = RenderDevice::new().await.expect("a GPU");
    let mut backend = GpuBackend::for_export_on(dev);
    let target = backend
        .format_target(size, Yuv::Nv12, false)
        .expect("a read-back format");
    let mut targets = FormatTargets::new(vec![target]);
    let (mut pixels, mut stats) = (Vec::new(), Vec::with_capacity(times.len()));
    for &t in times {
        let plan = frame_plan(&ctx, secs(t), FormatKind::Portrait);
        let done = backend
            .render_formats(std::slice::from_ref(&plan), &mut targets, &mut pixels)
            .await
            .expect("an export frame");
        for frame in pixels.drain(..) {
            match frame {
                YuvPixels::Readback(mut pending) => {
                    pending.wait().await.expect("its pixels");
                }
                #[cfg(target_os = "macos")]
                YuvPixels::Surface(_) => unreachable!("a read-back format"),
            }
        }
        done.wait().await.expect("its GPU work done");
        stats.push(targets.iter().next().expect("one format").text_stats().1);
    }
    stats
}

/// The same frame through an export: a job made by `ExportJob::create` in a temp folder (its frozen `Captions`, its
/// fonts folder), reopened as the worker does, the worker's font start-up (`fonts_before_frame_zero`), its context
/// (`with_cursors`, the job's captions and text language) and its renderer (`GpuBackend::for_export_on`), drawn without
/// encoding. Returns the frame and the reopened job.
pub async fn render_caption_frame_via_export(
    style: &str,
    words: &[(&str, f64, f64)],
    t: f64,
    size: (u32, u32),
) -> (RgbaImage, ExportJob) {
    let dir = tempfile::tempdir().expect("a temp folder");
    let (p, speech) = sound_only_captions_project(style, words);
    let files = plan_files(&p, &[FormatKind::Portrait], ExportQuality::High, dir.path());
    let batch = Batch {
        project: p,
        fps: 30,
        quality: ExportQuality::High,
        files,
        pcm_dir: None,
        speech: Arc::new(speech),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor: false,
    };
    let made = ExportJob::create(&batch, 10).expect("the export job");
    let job = ExportJob::open(&made.dir).expect("the job reopened");
    fonts_before_frame_zero(&job);
    let dev = RenderDevice::new().await.expect("a GPU");
    let mut backend = GpuBackend::for_export_on(dev);
    let ctx = PlanContext::with_cursors(
        Arc::new(job.project.clone()),
        Registry::builtin(),
        Arc::default(),
    )
    .with_captions(job.captions.clone())
    .with_text_language(job.text_language.as_str().into());
    let img = draw(&mut backend, &ctx, t, size).await;
    (img, job)
}

/// The share of pixels whose every channel is within `tol` of `b`'s, and the mean absolute difference per channel.
pub fn compare(a: &RgbaImage, b: &RgbaImage, tol: u8) -> (f64, f64) {
    assert_eq!(a.dimensions(), b.dimensions(), "the images' sizes");
    let (mut within, mut sum) = (0u64, 0u64);
    for (p, q) in a.pixels().zip(b.pixels()) {
        let d = p.0.iter().zip(q.0).map(|(x, y)| x.abs_diff(y));
        let (most, total) = d.fold((0u8, 0u64), |(m, s), v| (m.max(v), s + u64::from(v)));
        within += u64::from(most <= tol);
        sum += total;
    }
    let n = u64::from(a.width()) * u64::from(a.height());
    (within as f64 / n as f64, sum as f64 / (n * 4) as f64)
}
