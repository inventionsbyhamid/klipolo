//! `Compile` and `Measure`: the draft is checked and hardened (`assemble_untrusted`), then
//! measured at its corners, each stepped first (`measure`); a `resolution: auto` generator is resolved by 4 frames at
//! full and half size (PSNR ≥ 40 dB picks half; skipped for Over); a 256 × 144 thumbnail at the middle of its time goes
//! on its own line; and the output check's count is said (`audit`). The reply carries the guard's notes, the key it
//! measured (m11, with the ABI's minor version) and the hardened text's hash. Each step sends `Progress`.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use klipolo_model::contrib::{ContribDef, Resolution};

use super::cost::cost_of;
use super::{failed, send};
use crate::headless::Headless;
use crate::make::audit::{AUDIT_SIZE, AuditDraw, audit};
use crate::make::draft::{DraftLayer, shader_kind};
use crate::make::measure::Meter;
use crate::make::protocol::{Blob, Key, MeasureSpec, Step, WireKind, WorkerReply, fit_jpeg};
use crate::make::sizes::preview_size;
use crate::plan::pack_params;
use crate::registry::guard::{Diagnostic, Severity};
use crate::registry::key::{ContribSource, contrib_key};

/// The thumbnail's size.
pub const THUMBNAIL: (u32, u32) = (256, 144);
/// `auto` picks half size from this PSNR on.
pub const AUTO_PSNR_DB: f64 = 40.0;

/// One request's draft.
pub(super) struct Job<'a> {
    pub id: u64,
    pub kind: WireKind,
    pub def: &'a ContribDef,
    pub body: &'a str,
    pub spec: &'a MeasureSpec,
    pub tmp: &'a Path,
    /// When its measurement stops.
    pub deadline: crate::make::measure::Deadline,
}

/// `Compile`: checked, measured, thumbnailed, audited.
pub(super) async fn compile(
    meter: &mut dyn Meter,
    h: &mut Headless,
    job: &Job<'_>,
    out: &mut dyn Write,
) -> WorkerReply {
    let progress = |out: &mut dyn Write, step| {
        let _ = send(out, &WorkerReply::Progress { id: job.id, step }, step);
    };
    progress(out, Step::Harden);
    let draft = match draft_of(job) {
        Ok(draft) => draft,
        Err(reply) => return *reply,
    };
    progress(out, Step::Measure);
    let (cost, resolved) = match cost_of(meter, h, &draft, job.spec, job.deadline).await {
        Ok(found) => found,
        Err(e) => return failed(job.id, Step::Measure, &e),
    };
    progress(out, Step::Frames);
    match thumbnail(h, &draft).await {
        Ok(jpeg) => {
            let _ = send(
                out,
                &WorkerReply::Thumbnail { id: job.id, jpeg },
                Step::Frames,
            );
        }
        Err(e) => return failed(job.id, Step::Frames, &e),
    }
    let mut diagnostics: Vec<Diagnostic> = draft.notes.to_vec();
    if let Some(d) = unsanitised(h, &draft).await {
        diagnostics.push(d);
    }
    WorkerReply::Compiled {
        id: job.id,
        ok: true,
        diagnostics,
        facts: draft.facts().cloned(),
        cost: Some(cost),
        resolved,
        key: Some(key_of(job)),
        hardened: Some(draft.hardened),
        scene: None,
    }
}

/// `Measure` of a draft at the request's settings.
pub(super) async fn measure_only(
    meter: &mut dyn Meter,
    h: &mut Headless,
    job: &Job<'_>,
) -> WorkerReply {
    let draft = match draft_of(job).map_err(|reply| *reply) {
        Ok(draft) => draft,
        Err(WorkerReply::Compiled { diagnostics, .. }) => {
            let first = diagnostics
                .first()
                .map_or("it was refused", |d| d.message.as_str());
            return failed(
                job.id,
                Step::Rules,
                &format!("the shader was refused: {first}"),
            );
        }
        Err(reply) => return reply,
    };
    match cost_of(meter, h, &draft, job.spec, job.deadline).await {
        Ok((cost, _)) => WorkerReply::Measured {
            id: job.id,
            cost,
            hardened: Some(draft.hardened),
        },
        Err(e) => failed(job.id, Step::Measure, &e),
    }
}

/// The key of what is measured: its definition and source, under the ABI and its minor version.
fn key_of(job: &Job<'_>) -> Key {
    Key(contrib_key(job.def, &ContribSource::Wgsl(job.body.into())))
}

/// The draft, or the reply that refuses it (boxed: a reply is large).
fn draft_of(job: &Job<'_>) -> Result<DraftLayer, Box<WorkerReply>> {
    let Some(kind) = job.kind.shader() else {
        return Err(Box::new(failed(
            job.id,
            Step::Parse,
            "a scene is checked with checkScene",
        )));
    };
    if shader_kind(job.def.kind) != Some(kind) {
        return Err(Box::new(failed(
            job.id,
            Step::Rules,
            "the definition's kind isn't the request's",
        )));
    }
    let format = klipolo_model::project::FormatKind::Landscape;
    DraftLayer::new(job.def, job.body, job.spec.params.clone(), format, job.tmp).map_err(
        |diagnostics| {
            Box::new(WorkerReply::Compiled {
                id: job.id,
                ok: false,
                diagnostics,
                facts: None,
                cost: None,
                resolved: None,
                key: Some(key_of(job)),
                hardened: None,
                scene: None,
            })
        },
    )
}

/// `auto`: 4 frames at full and at half size; half when they agree to `AUTO_PSNR_DB`.
pub(super) async fn resolve_auto(
    h: &mut Headless,
    draft: &DraftLayer,
) -> Result<Resolution, String> {
    let size = preview_size(draft.format);
    let project = Arc::new(draft.project());
    let mut total = 0.0;
    let times = draft.times(4);
    for &at in &times {
        let mut frames = Vec::with_capacity(2);
        for half in [false, true] {
            draft.prepare(h, true, half);
            frames.push(h.render(project.clone(), at, draft.format, size).await?);
        }
        total += psnr(&frames[0].bytes, &frames[1].bytes);
    }
    Ok(if total / times.len() as f64 >= AUTO_PSNR_DB {
        Resolution::Half
    } else {
        Resolution::Full
    })
}

/// Of two frames' bytes (RGBA, the same stride), in dB; 99 for equal frames.
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len()).max(1) as f64;
    let mse: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2))
        .sum::<f64>()
        / n;
    if mse <= 1e-12 {
        99.0
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

/// The card's thumbnail: the draft alone at the middle of its time, over the checkerboard.
pub(super) async fn thumbnail(h: &mut Headless, draft: &DraftLayer) -> Result<Blob, String> {
    draft.prepare(h, false, false);
    h.set_transparent(true);
    let frame = h
        .render(
            Arc::new(draft.project()),
            draft.middle(),
            draft.format,
            THUMBNAIL,
        )
        .await?;
    let image = image::RgbImage::from_fn(THUMBNAIL.0, THUMBNAIL.1, |x, y| {
        let i = y as usize * frame.stride as usize + x as usize * 4;
        let under = if (x / 8 + y / 8) % 2 == 0 {
            102.0
        } else {
            153.0
        };
        let a = f32::from(frame.bytes[i + 3]) / 255.0;
        let px = |c: u8| (f32::from(c) + (1.0 - a) * under).round().clamp(0.0, 255.0) as u8;
        image::Rgb([
            px(frame.bytes[i]),
            px(frame.bytes[i + 1]),
            px(frame.bytes[i + 2]),
        ])
    });
    Blob::new(fit_jpeg(&image))
}

/// A warning when the output check replaced pixels at the middle of its time (`audit`).
async fn unsanitised(h: &Headless, draft: &DraftLayer) -> Option<Diagnostic> {
    let shader = draft.shader()?;
    let draw = AuditDraw {
        effect: shader,
        params: pack_params(shader, &draft.params),
        size: preview_size(draft.format),
        at: (draft.middle() as f64 / 1e6) as f32,
        duration: draft.seconds as f32,
        cache: Some(&draft.audit),
    };
    let u = audit(h.device(), &draw).await.ok()?;
    (u.total() > 0).then(|| {
        let all = AUDIT_SIZE.0 * AUDIT_SIZE.1;
        Diagnostic {
            severity: Severity::Warning,
            ..Diagnostic::info(
                "output.unsanitised",
                format!(
                    "The output check replaced {} of {all} pixels at 128 × 72: {} weren't finite (NaN or infinite), {} \
                     were outside 0–1 and {} had colour above their alpha.",
                    u.total(),
                    u.non_finite,
                    u.out_of_range,
                    u.over_alpha
                ),
            )
            .hint("Guard divisions and roots (`max(x, 1e-4)`), and return premultiplied colour (`rgb * a`).")
        }
    })
}
