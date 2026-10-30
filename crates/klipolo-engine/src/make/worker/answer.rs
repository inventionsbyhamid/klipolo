//! What each request does. `Validate` checks the text alone, with no GPU. `Compile`,
//! `Measure`, `Verify` and `Frames` draw on the worker's device, made by the first of them and kept warm (`make::gpu`),
//! with its fonts configured first (`make::fonts`). `Verify` loads the folder the app wrote into `in/` through the one
//! folder reader and checks it as `Compile` does, answering with the key it measured; a scene folder goes the way
//! `CheckScene` does (`scene`). A request that lost the device answers "the graphics card stopped this
//! shader" and ends the worker.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use klipolo_model::contrib::ContribDef;

use super::compile;
use super::scene::{Checked, SceneJob, check_scene, checked_only};
use super::{failed, send};
use crate::headless::Headless;
use crate::make::draft::shader_kind;
use crate::make::fonts::configure_worker_fonts;
use crate::make::frames::{Frames, FramesError, render_frames};
use crate::make::gpu::worker_headless;
use crate::make::measure::{COMPILE_BUDGET, Deadline, MEASURE_BUDGET, Meter};
use crate::make::protocol::{
    Blob, FramesSpec, SourceRef, Step, WireKind, WorkerReply, WorkerRequest,
};
use crate::make::scene_check::{SceneInput, read_fills, read_svgs};
use crate::make::session::{IN, TMP};
use crate::registry::guard;
use crate::registry::key::ContribSource;
use crate::registry::pack::{PackLimits, load_folder_as};
use crate::render::shared_fonts::SharedFonts;

/// What the device's loss is answered with.
pub const DEVICE_LOST: &str = "the graphics card stopped this shader";

pub(super) enum Answer {
    Reply(WorkerReply),
    /// The device was lost: this is the request's answer, then the worker exits.
    Lost(WorkerReply),
}

pub(super) struct Worker {
    session: PathBuf,
    meter: Box<dyn Meter>,
    /// Made from `in/fonts.json` at start, handed to the `Headless` when the first GPU request makes it.
    fonts: Option<SharedFonts>,
    gpu: Option<Headless>,
    /// The scene keys it checked within class this session.
    scenes: Checked,
}

impl Worker {
    pub fn new(session: &Path, meter: Box<dyn Meter>) -> Self {
        Self {
            fonts: Some(configure_worker_fonts(&session.join(IN))),
            session: session.to_owned(),
            meter,
            gpu: None,
            scenes: Checked::default(),
        }
    }

    /// `request`'s answer; `arrived` is when its line was read (a measurement's budget runs from it).
    pub async fn answer(
        &mut self,
        request: WorkerRequest,
        out: &mut dyn Write,
        arrived: Instant,
    ) -> Answer {
        let tmp = self.session.join(TMP);
        let (id, step) = (request.id().unwrap_or(0), request.first_step());
        let reply = match request {
            WorkerRequest::Validate {
                id,
                kind,
                def,
                body,
            } => return Answer::Reply(validate(id, kind, &def, &body)),
            WorkerRequest::Compile {
                id,
                kind,
                def,
                body,
                measure,
            } => {
                let h = match ensure(&mut self.gpu, &mut self.fonts).await {
                    Ok(h) => h,
                    Err(e) => return Answer::Reply(failed(id, Step::Compile, &e)),
                };
                let job = compile::Job {
                    id,
                    kind,
                    def: &def,
                    body: &body,
                    spec: &measure,
                    tmp: &tmp,
                    deadline: Deadline::new(arrived, measure.budget_ms, COMPILE_BUDGET),
                };
                compile::compile(&mut *self.meter, h, &job, out).await
            }
            WorkerRequest::Measure {
                id,
                source,
                measure,
            } => {
                let SourceRef::Draft { def, body } = source else {
                    return Answer::Reply(failed(
                        id,
                        Step::Measure,
                        "this worker doesn't measure library contributions yet",
                    ));
                };
                let h = match ensure(&mut self.gpu, &mut self.fonts).await {
                    Ok(h) => h,
                    Err(e) => return Answer::Reply(failed(id, Step::Measure, &e)),
                };
                let job = compile::Job {
                    id,
                    kind: wire_kind(&def),
                    def: &def,
                    body: &body,
                    spec: &measure,
                    tmp: &tmp,
                    deadline: Deadline::new(arrived, measure.budget_ms, MEASURE_BUDGET),
                };
                compile::measure_only(&mut *self.meter, h, &job).await
            }
            WorkerRequest::Frames { id, spec } => {
                if let Some(why) = spec.refusal() {
                    return Answer::Reply(failed(id, Step::Frames, why));
                }
                let inputs = self.session.join(IN);
                let h = match ensure(&mut self.gpu, &mut self.fonts).await {
                    Ok(h) => h,
                    Err(e) => return Answer::Reply(failed(id, Step::Frames, &e)),
                };
                self.meter.begin();
                // Frames an agent sees wait once for this computer's fonts, so a
                // text in an installed family isn't shown in its stand-in and "fixed" by the agent.
                super::scene::fonts_ready(super::scene::FONTS_WAIT).await;
                let dirs = (inputs.as_path(), tmp.as_path());
                frames(h, &mut *self.meter, id, &spec, dirs, out, &self.scenes).await
            }
            WorkerRequest::Verify {
                id,
                contrib,
                folder,
                measure,
                contribs,
                ..
            } => {
                // The folder the app wrote from the bytes it loaded, read through the one reader.
                let dir = self.session.join(IN).join(folder.as_str());
                let loaded = match load_folder_as(&dir, &contrib, &PackLimits::DEFAULT) {
                    Ok(loaded) => loaded,
                    Err(e) => {
                        let why = format!("the contribution's folder didn't load: {e}");
                        return Answer::Reply(failed(id, Step::Scan, &why));
                    }
                };
                if let ContribSource::Scene(bytes) = &loaded.source {
                    let inputs = self.session.join(IN);
                    let fills = match read_fills(&inputs, contribs.as_ref()) {
                        Ok(fills) => fills,
                        Err(e) => return Answer::Reply(failed(id, Step::Scan, &e)),
                    };
                    let h = match ensure(&mut self.gpu, &mut self.fonts).await {
                        Ok(h) => h,
                        Err(e) => return Answer::Reply(failed(id, Step::Compile, &e)),
                    };
                    let job = SceneJob {
                        id,
                        def: &loaded.def,
                        input: SceneInput::Stored { bytes },
                        fills,
                        spec: &measure,
                        deadline: Deadline::new(arrived, measure.budget_ms, MEASURE_BUDGET),
                    };
                    let reply = check_scene(&mut *self.meter, h, job, out, &mut self.scenes).await;
                    return self.lost_or(id, step, reply);
                }
                let ContribSource::Wgsl(body) = &loaded.source else {
                    return Answer::Reply(failed(id, Step::Parse, "an unknown source"));
                };
                let h = match ensure(&mut self.gpu, &mut self.fonts).await {
                    Ok(h) => h,
                    Err(e) => return Answer::Reply(failed(id, Step::Compile, &e)),
                };
                let job = compile::Job {
                    id,
                    kind: wire_kind(&loaded.def),
                    def: &loaded.def,
                    body,
                    spec: &measure,
                    tmp: &tmp,
                    deadline: Deadline::new(arrived, measure.budget_ms, MEASURE_BUDGET),
                };
                compile::compile(&mut *self.meter, h, &job, out).await
            }
            WorkerRequest::CheckScene {
                id,
                def,
                scene,
                svgs,
                contribs,
                measure,
                check_only,
            } => {
                let inputs = self.session.join(IN);
                let bytes = match submitted(&inputs.join(scene.as_str())) {
                    Ok(bytes) => bytes,
                    Err(e) => return Answer::Reply(failed(id, Step::Scan, &e)),
                };
                let svgs = match read_svgs(&inputs, &svgs) {
                    Ok(svgs) => svgs,
                    Err(e) => return Answer::Reply(failed(id, Step::Svg, &e)),
                };
                let fills = match read_fills(&inputs, Some(&contribs)) {
                    Ok(fills) => fills,
                    Err(e) => return Answer::Reply(failed(id, Step::Scan, &e)),
                };
                if check_only {
                    let input = SceneInput::Submitted {
                        bytes: &bytes,
                        svgs: &svgs,
                    };
                    return Answer::Reply(checked_only(id, &def, input, fills));
                }
                let h = match ensure(&mut self.gpu, &mut self.fonts).await {
                    Ok(h) => h,
                    Err(e) => return Answer::Reply(failed(id, Step::Compile, &e)),
                };
                let job = SceneJob {
                    id,
                    def: &def,
                    input: SceneInput::Submitted {
                        bytes: &bytes,
                        svgs: &svgs,
                    },
                    fills,
                    spec: &measure,
                    deadline: Deadline::new(arrived, measure.budget_ms, COMPILE_BUDGET),
                };
                check_scene(&mut *self.meter, h, job, out, &mut self.scenes).await
            }
            WorkerRequest::Shutdown => failed(0, step, "nothing to answer"),
        };
        self.lost_or(id, step, reply)
    }

    /// `reply`, unless the request lost the device: then the loss is its answer and the worker ends.
    fn lost_or(&self, id: u64, step: Step, reply: WorkerReply) -> Answer {
        if self.gpu.as_ref().is_some_and(|h| h.device().is_lost()) {
            crate::make::say!("render worker: the device was lost");
            return Answer::Lost(failed(id, step, DEVICE_LOST));
        }
        Answer::Reply(reply)
    }
}

/// A submitted scene's bytes from `path`: at most one byte past the format's cap, so the loader refuses an oversized
/// one with the cap named.
fn submitted(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let cap = klipolo_model::scene::SceneLimits::MAX_BYTES as u64 + 1;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(cap).read_to_end(&mut bytes))
        .map_err(|e| format!("the scene can't be read: {e}"))?;
    Ok(bytes)
}

/// The wire kind of a shader definition (a generator for anything that isn't a filter or a transition).
fn wire_kind(def: &ContribDef) -> WireKind {
    match shader_kind(def.kind) {
        Some(crate::registry::ShaderKind::Filter) => WireKind::Filter,
        Some(crate::registry::ShaderKind::Transition) => WireKind::Transition,
        _ => WireKind::Generator,
    }
}

/// The worker's `Headless`, made the first time (its fonts first).
async fn ensure<'h>(
    gpu: &'h mut Option<Headless>,
    fonts: &mut Option<SharedFonts>,
) -> Result<&'h mut Headless, String> {
    if gpu.is_none() {
        *gpu = Some(worker_headless(fonts.take()).await?);
    }
    Ok(gpu.as_mut().expect("made above"))
}

/// The checks on the text, and the loop guards on its IR, so `ok` means nothing before the GPU refuses it.
/// A shader that passed carries the guard's notes (`Checked::notes`).
fn validate(id: u64, kind: WireKind, def: &ContribDef, body: &str) -> WorkerReply {
    let Some(kind) = kind.shader() else {
        return failed(id, Step::Parse, "a scene is checked with checkScene");
    };
    let checked = guard::check(def, kind, body).and_then(|checked| {
        guard::harden::harden(&checked)?;
        Ok(checked)
    });
    match checked {
        Ok(checked) => WorkerReply::Validated {
            id,
            ok: true,
            diagnostics: checked.notes,
            facts: Some(checked.facts),
        },
        Err(diagnostics) => WorkerReply::Validated {
            id,
            ok: false,
            diagnostics,
            facts: None,
        },
    }
}

/// `spec`'s frames, each on its own line as it is drawn, then `Framed`. `dirs` are the session's `in/` and `tmp/`.
async fn frames(
    h: &mut Headless,
    meter: &mut dyn Meter,
    id: u64,
    spec: &FramesSpec,
    (inputs, tmp): (&Path, &Path),
    out: &mut dyn Write,
    scenes: &Checked,
) -> WorkerReply {
    let source = match Frames::of(spec, inputs, tmp, &|key| scenes.contains(key)) {
        Ok(source) => source,
        Err(FramesError::Refused(d)) => {
            let first = d.first().map_or("it was refused", |d| d.message.as_str());
            // A scene's problem names its place.
            let what = match d.first().and_then(|d| d.pointer.as_deref()) {
                Some(at) => format!("the scene was refused at {at}"),
                None => "the shader was refused".into(),
            };
            return failed(id, Step::Frames, &format!("{what}: {first}"));
        }
        Err(FramesError::Failed(e)) => return failed(id, Step::Frames, &e),
    };
    let drawn = render_frames(h, meter, spec, &source, |f| {
        let jpeg = Blob::new(f.jpeg)?;
        let line = WorkerReply::Frame {
            id,
            index: f.index,
            t: f.t,
            jpeg,
            stats: f.stats,
        };
        send(out, &line, Step::Frames).map_err(|e| e.to_string())
    })
    .await;
    match drawn {
        Ok(drawn) => WorkerReply::Framed {
            id,
            count: drawn.count,
            cost: None,
            notes: drawn.notes,
        },
        Err(e) => failed(id, Step::Frames, &e),
    }
}
