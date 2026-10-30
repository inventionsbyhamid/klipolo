//! Preview frames from the worker: a draft drawn alone, over a mid-grey checkerboard so transparency
//! shows, or a dry-run copy of the project (`project`), drawn from the stills the app decoded. Each frame goes back as
//! its own JPEG (q85, fitted to a reply line) with its statistics (`stats`), or all of them as one contact sheet. While
//! the client says the GPU is busy, frames are submitted at least 33 ms apart.

mod pixels;
pub(crate) mod project;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::RgbImage;
use klipolo_model::contrib::ContribKind;
use klipolo_model::project::Project;
use klipolo_model::time::secs_to_us;

use pixels::{contact_sheet, opaque, over_checkerboard, packed};
pub use project::{ContribEntry, ProjectInputs, draft_not_checked, load};

use super::audit::{AuditDraw, audit};
use super::budget::STEP_LIMIT_MS;
use super::draft::DraftLayer;
use super::measure::{FrameReq, Meter};
use super::protocol::{FrameSource, FramesSpec, Key, MAX_FRAME_EDGE, MAX_FRAMES, fit_jpeg};
use super::scene_check::{SceneInput, check, read_fills};
use super::sizes::frame_size;
use super::stats::{FrameStats, frame_stats, loop_seam};
use crate::headless::Headless;
use crate::plan::pack_params;
use crate::registry::guard::Diagnostic;

/// Frames go at least this far apart while the GPU is busy.
pub const BUSY_SPACING: Duration = Duration::from_millis(33);
/// Notes a reply carries at most, each at most this long.
const MAX_NOTES: usize = 16;
const MAX_NOTE_CHARS: usize = 200;

/// One frame drawn.
pub struct FrameOut {
    pub index: u32,
    pub t: f64,
    pub jpeg: Vec<u8>,
    pub stats: FrameStats,
}

/// What a request drew.
pub struct Drawn {
    pub count: u32,
    pub notes: Vec<String>,
}

/// Why no frame could be drawn.
pub enum FramesError {
    /// The draft was refused, with the guard's diagnostics.
    Refused(Vec<Diagnostic>),
    /// Its inputs or the GPU failed.
    Failed(String),
}

/// What frames are drawn from.
pub enum Frames {
    Alone(DraftLayer),
    Project(ProjectInputs),
}

impl Frames {
    /// `spec`'s source, read from the session's `inputs` (`in/`), a draft's test image in `tmp`. A scene draft placed in
    /// a project draws only when `checked` says this worker checked its key this session.
    pub fn of(
        spec: &FramesSpec,
        inputs: &Path,
        tmp: &Path,
        checked: &dyn Fn(&Key) -> bool,
    ) -> Result<Self, FramesError> {
        match &spec.source {
            FrameSource::Alone { draft, contribs } if draft.def.kind == ContribKind::Scene => {
                let input = SceneInput::Stored {
                    bytes: draft.body.as_bytes(),
                };
                let fills = read_fills(inputs, contribs.as_ref()).map_err(FramesError::Failed)?;
                let c = check(&draft.def, input, fills).map_err(|problems| {
                    FramesError::Refused(problems.iter().map(Diagnostic::of_scene).collect())
                })?;
                let params = draft.params.clone();
                Ok(Frames::Alone(DraftLayer::of_scene(
                    &c.def,
                    c.scene,
                    c.registry,
                    params,
                    spec.format,
                )))
            }
            FrameSource::Alone { draft, .. } => DraftLayer::new(
                &draft.def,
                &draft.body,
                draft.params.clone(),
                spec.format,
                tmp,
            )
            .map(Frames::Alone)
            .map_err(FramesError::Refused),
            FrameSource::Project {
                snapshot,
                contribs,
                stills,
            } => load(inputs, (snapshot, contribs, stills), checked)
                .map(Frames::Project)
                .map_err(FramesError::Failed),
        }
    }
}

/// `spec`'s frames of `source` on `h`, each handed to `each` as it is drawn (a sheet once, at the end). A draft alone
/// is stepped first with `meter` (16², 64², then the frames' size), and refused when a frame would take over the step
/// limit: it may never have compiled, or have compiled Over.
pub async fn render_frames(
    h: &mut Headless,
    meter: &mut dyn Meter,
    spec: &FramesSpec,
    source: &Frames,
    mut each: impl FnMut(FrameOut) -> Result<(), String>,
) -> Result<Drawn, String> {
    let size = frame_size(spec.format, spec.long_edge.clamp(16, MAX_FRAME_EDGE));
    if let Frames::Alone(draft) = source {
        fits(meter, h, draft, size).await?;
    }
    let (project, alone): (Arc<Project>, Option<&DraftLayer>) = match source {
        Frames::Alone(draft) => {
            draft.prepare(h, false, false);
            h.set_transparent(true);
            (Arc::new(draft.project()), Some(draft))
        }
        Frames::Project(p) => {
            h.set_registry(p.registry.clone());
            h.set_stills(Some(p.stills.clone()));
            h.set_transparent(false);
            (p.project.clone(), None)
        }
    };
    let mut notes: Vec<String> = match source {
        Frames::Project(p) => p.notes.clone(),
        Frames::Alone(_) => Vec::new(),
    };
    let times: Vec<f64> = spec.times.iter().copied().take(MAX_FRAMES).collect();
    let (mut first, mut prev): (Option<Vec<f32>>, Option<Vec<f32>>) = (None, None);
    let mut sheet: Vec<(RgbImage, FrameStats)> = Vec::new();
    let mut last_submit: Option<Instant> = None;
    for (i, &t) in times.iter().enumerate() {
        if spec.busy
            && let Some(at) = last_submit
            && at.elapsed() < BUSY_SPACING
        {
            tokio::time::sleep(BUSY_SPACING - at.elapsed()).await;
        }
        last_submit = Some(Instant::now());
        let at_us = secs_to_us(t.max(0.0));
        let frame = h.render(project.clone(), at_us, spec.format, size).await?;
        let names = match source {
            Frames::Project(p) => Some(&p.names),
            Frames::Alone(_) => None,
        };
        for w in &frame.warnings {
            note(&mut notes, w.chars().take(MAX_NOTE_CHARS).collect());
        }
        for m in &frame.missing {
            let name = names
                .and_then(|n| n.get(&**m))
                .map_or("a file", String::as_str);
            note(
                &mut notes,
                format!("frame.mediaMissing: {}", project::short(name)),
            );
        }
        let rgba = packed(&frame.bytes, frame.stride, size);
        let (mut stats, plane) = frame_stats(&rgba, prev.as_deref());
        if let Some(draft) = alone {
            stats.nan_fixed = nan_fixed(h, draft, size, t).await;
        }
        if i + 1 == times.len() && i > 0 {
            stats.loop_seam = first.as_deref().map(|f| loop_seam(f, &plane));
        }
        let image = if alone.is_some() {
            over_checkerboard(&rgba, size)
        } else {
            opaque(&rgba, size)
        };
        if first.is_none() {
            first = Some(plane.clone());
        }
        prev = Some(plane);
        if spec.sheet {
            sheet.push((image, stats));
        } else {
            each(FrameOut {
                index: i as u32,
                t,
                jpeg: fit_jpeg(&image),
                stats,
            })?;
        }
        if h.device().is_lost() {
            return Err(crate::render::device::GPU_FAILED.into());
        }
    }
    if spec.sheet && !sheet.is_empty() {
        let (image, stats) = contact_sheet(&sheet);
        each(FrameOut {
            index: 0,
            t: times[0],
            jpeg: fit_jpeg(&image),
            stats,
        })?;
    }
    for note in &mut notes {
        *note = crate::registry::guard::shown(note);
    }
    Ok(Drawn {
        count: times.len() as u32,
        notes,
    })
}

/// Whether `draft`'s frames at `size` stay under the step limit: 16², 64², then `size`, each predicted from the last
/// and drawn only when the prediction allows.
async fn fits(
    meter: &mut dyn Meter,
    h: &mut Headless,
    draft: &DraftLayer,
    size: (u32, u32),
) -> Result<(), String> {
    let pixels = |s: (u32, u32)| f64::from(s.0) * f64::from(s.1);
    let mut sizes: Vec<(u32, u32)> = [(16, 16), (64, 64)]
        .into_iter()
        .filter(|s| pixels(*s) < pixels(size))
        .collect();
    sizes.push(size);
    draft.prepare(h, true, false);
    let project = Arc::new(draft.project());
    let mut last: Option<(f64, f64)> = None;
    for s in sizes {
        let predicted = last.map_or(0.0, |(ms, px)| ms * pixels(s) / px);
        if predicted > STEP_LIMIT_MS {
            return Err(too_slow(predicted, s));
        }
        let req = FrameReq::new(project.clone(), draft, draft.middle(), s);
        meter.warm(h, &req).await?;
        let sample = meter.frame(h, &req).await?;
        if sample.gpu_ms > STEP_LIMIT_MS {
            return Err(too_slow(sample.gpu_ms, s));
        }
        last = Some((sample.gpu_ms.max(1e-6), pixels(s)));
    }
    Ok(())
}

fn too_slow(ms: f64, size: (u32, u32)) -> String {
    format!(
        "a frame of it would take about {ms:.0} ms at {}×{}, over the {STEP_LIMIT_MS:.0} ms a frame may: make it \
         cheaper first",
        size.0, size.1
    )
}

fn note(notes: &mut Vec<String>, text: String) {
    if notes.len() < MAX_NOTES && !notes.contains(&text) {
        notes.push(text);
    }
}

/// The non-finite pixels the output check replaced at `t` (`audit`, at 128 × 72).
async fn nan_fixed(h: &Headless, draft: &DraftLayer, size: (u32, u32), t: f64) -> u64 {
    // A scene has no shader output to check: our own code draws it.
    let Some(shader) = draft.shader() else {
        return 0;
    };
    let draw = AuditDraw {
        effect: shader,
        params: pack_params(shader, &draft.params),
        size,
        at: t as f32,
        duration: draft.seconds as f32,
        cache: Some(&draft.audit),
    };
    audit(h.device(), &draw).await.map_or(0, |u| u.non_finite)
}
