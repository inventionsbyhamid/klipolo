//! Scene layers through the export's own drawing: one frame drawn by
//! `GpuBackend::render_formats` into a read-back target, as the export worker draws it, without encoding.

use std::sync::Arc;

use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::Us;

use crate::plan::{PlanContext, frame_plan};
use crate::registry::Registry;
use crate::render::GpuBackend;
use crate::render::device::RenderDevice;
use crate::render::export::FormatTargets;
use crate::render::yuv::Yuv;

/// The frame of `project` at `at_us` (landscape, `size` pixels, with `registry`) drawn as an export draws it: the notes
/// the export reports for it (code and time), or why it stopped.
pub async fn export_frame_notes(
    registry: Arc<Registry>,
    project: Project,
    at_us: Us,
    size: (u32, u32),
) -> Result<Vec<(String, Us)>, String> {
    export_frame(registry, project, at_us, size, false).await
}

/// [`export_frame_notes`] with the scene text build thread gone: an export never draws a scene without
/// its text.
#[cfg(any(test, feature = "test-support"))]
pub async fn export_frame_notes_without_text_thread(
    registry: Arc<Registry>,
    project: Project,
    at_us: Us,
    size: (u32, u32),
) -> Result<Vec<(String, Us)>, String> {
    export_frame(registry, project, at_us, size, true).await
}

async fn export_frame(
    registry: Arc<Registry>,
    project: Project,
    at_us: Us,
    size: (u32, u32),
    lose_text_thread: bool,
) -> Result<Vec<(String, Us)>, String> {
    let dev = RenderDevice::new().await?;
    let mut backend = GpuBackend::for_export_on(dev);
    #[allow(unused_mut)]
    let mut target = backend.format_target(size, Yuv::Nv12, false)?;
    #[cfg(any(test, feature = "test-support"))]
    if lose_text_thread {
        target.lose_scene_text_thread();
    }
    #[cfg(not(any(test, feature = "test-support")))]
    let _ = lose_text_thread;
    let mut targets = FormatTargets::new(vec![target]);
    let ctx = PlanContext::with_registry(Arc::new(project), registry);
    let plan = frame_plan(&ctx, at_us, FormatKind::Landscape);
    let mut pixels = Vec::new();
    backend
        .render_formats(std::slice::from_ref(&plan), &mut targets, &mut pixels)
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(targets.take_notes())
}
