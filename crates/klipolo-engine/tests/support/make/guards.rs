//! Helpers for the main app's guards: a checked filter drawn by a `Headless`
//! in either mode, its guard events and their acknowledgements, the app's preview backend, and export frames of a
//! checked generator with their submissions counted.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use klipolo_engine::api::EngineEvent;
use klipolo_engine::headless::Headless;
use klipolo_engine::make::budget::Cost;
use klipolo_engine::plan::{PlanContext, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::registry::pack::{LoadedContrib, Verdict, hex, load_files};
use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind};
use klipolo_engine::render::GpuBackend;
use klipolo_engine::render::device::RenderDevice;
use klipolo_engine::render::mode::RenderMode;
use klipolo_engine::render::warm::Warmup;
use klipolo_engine::session::{PreviewBackend, RenderRequest};
use klipolo_model::contrib::{ContribDef, ContribId, ContribKind, PaddingSpec};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::Us;
use klipolo_model::timeline::EffectInstance;
use sha2::{Digest, Sha256};

use super::{bx_px, fx, image_project, test_registry};

/// A `Headless` drawing a 100 × 100 px red image at (400, 300) of a 1920 × 1080 frame under one checked filter,
/// `com.test.fx/effect.halo`, padded by 16 px; and the FirstDraw marks it acknowledged so far.
pub struct GuardedHeadless {
    pub h: Headless,
    project: Arc<Project>,
    acknowledged: usize,
    _dir: tempfile::TempDir,
}

/// [`GuardedHeadless`] with `body` as its filter, drawn in `mode`.
pub async fn headless_with_checked_filter(body: &str, mode: RenderMode) -> GuardedHeadless {
    guarded(test_registry(&[("halo", body, Some(16.0))]), mode).await
}

/// [`headless_with_checked_filter`] with the filter verified at `cost` (so the preview samples it).
pub async fn headless_with_verified_filter(
    body: &str,
    cost: Cost,
    mode: RenderMode,
) -> GuardedHeadless {
    let def = super::padded_filter_def("halo", Some(16.0));
    let shader = ShaderEffect::assemble_untrusted(&def, ShaderKind::Filter, body)
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let verdict = Verdict::of(cost).expect("a verdict that runs");
    guarded(
        Registry::builtin_with_test_shader(&def, shader.verified(&verdict)),
        mode,
    )
    .await
}

/// A `Headless` in `mode` drawing one full-frame layer of the checked white generator (`com.test.fx/generator.white`):
/// it reads no time, so after its first frame its picture comes from the static cache.
pub async fn headless_with_static_checked_generator(mode: RenderMode) -> GuardedHeadless {
    let dir = tempfile::tempdir().unwrap();
    let project = super::generator_project(super::full_box(), 0.0, Vec::new());
    GuardedHeadless {
        h: Headless::with_mode(test_registry(&[]), mode).await.unwrap(),
        project: Arc::new(project),
        acknowledged: 0,
        _dir: dir,
    }
}

async fn guarded(registry: Arc<Registry>, mode: RenderMode) -> GuardedHeadless {
    let dir = tempfile::tempdir().unwrap();
    let project = image_project(dir.path(), [255, 0, 0], (400, 300, 100), vec![fx("halo")]);
    GuardedHeadless {
        h: Headless::with_mode(registry, mode).await.unwrap(),
        project: Arc::new(project),
        acknowledged: 0,
        _dir: dir,
    }
}

impl GuardedHeadless {
    /// The frame at `at`, 1920 × 1080, rows packed.
    pub async fn render_at(&mut self, at: Us) -> Vec<u8> {
        let f = self
            .h
            .render(
                self.project.clone(),
                at,
                FormatKind::Landscape,
                (1920, 1080),
            )
            .await
            .unwrap();
        packed(&f.bytes, f.stride, (1920, 1080))
    }

    /// The frame at `at`, `size` pixels (a small one draws fast: the 2 s bound on a draw mark stays out of the way).
    pub async fn render_sized(&mut self, at: Us, size: (u32, u32)) {
        self.h
            .render(self.project.clone(), at, FormatKind::Landscape, size)
            .await
            .unwrap();
    }

    /// Waits until the compile thread has made every pipeline asked for (none in `Contained`).
    pub async fn wait_for_pipelines(&mut self) {
        self.h.wait_for_pipelines().await;
    }

    /// The guard events the frames so far raised, as the engine sends them to the app.
    pub fn events(&self) -> &[EngineEvent] {
        self.h.events()
    }

    pub fn count(&self, what: fn(&EngineEvent) -> bool) -> usize {
        self.events().iter().filter(|e| what(e)).count()
    }

    /// The app's answer to every `FirstDraw` so far: its mark is on disk (`EngineCommand::MarkWritten`, by request).
    pub fn acknowledge_marks(&mut self) {
        self.acknowledged = acknowledge(&mut self.h, self.acknowledged);
    }
}

/// Answers every `FirstDraw` of `h` after the first `done` (its mark is on disk); how many there are now.
pub fn acknowledge(h: &mut Headless, done: usize) -> usize {
    let asked: Vec<(ContribId, u64)> = h
        .events()
        .iter()
        .filter_map(|e| match e {
            EngineEvent::FirstDraw { contrib, seq } => Some((contrib.clone(), *seq)),
            _ => None,
        })
        .collect();
    for (contrib, seq) in &asked[done..] {
        h.mark_written(contrib, *seq);
    }
    asked.len()
}

/// `bytes` with `stride` bytes a row, `size` pixels, packed.
pub fn packed(bytes: &[u8], stride: u32, size: (u32, u32)) -> Vec<u8> {
    let row = size.0 as usize * 4;
    bytes
        .chunks(stride as usize)
        .take(size.1 as usize)
        .flat_map(|r| &r[..row])
        .copied()
        .collect()
}

/// The backend the app's engine session runs on (the app's `engine.rs`): the warm-up's, else its own.
pub async fn app_preview_backend() -> GpuBackend {
    GpuBackend::for_preview(Some(Warmup::start(RenderMode::Interactive)))
        .await
        .unwrap()
}

/// A frame of a project with nothing in it, through the backend's own render path.
pub async fn render_blank_frame(backend: &mut GpuBackend) {
    let project = Arc::new(Project::new("Blank"));
    let ctx = PlanContext::new(project);
    let plan = frame_plan(&ctx, 0, FormatKind::Landscape);
    backend
        .render(RenderRequest {
            plan,
            width: 64,
            height: 36,
            output: Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
}

/// A frame drawn by the export's renderer, and the submissions its checked passes took.
pub struct ExportFrame {
    pub rgba: Vec<u8>,
    pub submissions: u32,
}

/// One 1920 × 1080 frame of a full-frame layer of the checked generator `body` (the waves preset's settings, or
/// `detail` when the body names it) verified at `cost`, drawn by the export's renderer (`GpuBackend::for_export_on`).
pub async fn export_frame_with_checked_generator(body: &str, cost: Cost) -> ExportFrame {
    let def = if body.contains("params.detail") {
        super::draft_def_with(
            r#"[{"key":"detail","label":"D","type":"float","min":0,"max":1,"default":0}]"#,
        )
    } else {
        super::draft_def()
    };
    let shader = ShaderEffect::assemble_untrusted(&def, ShaderKind::Generator, body)
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let verdict = Verdict::of(cost).expect("a verdict that runs");
    let registry = Registry::builtin_with_test_shader(&def, shader.verified(&verdict));
    let dev = RenderDevice::new().await.unwrap();
    let mut backend = GpuBackend::for_export_on(dev);
    let project = Arc::new(super::generator_piece_project(
        &def,
        0.0,
        2.0,
        None,
        Vec::new(),
    ));
    let ctx = PlanContext::with_registry(project, registry);
    let plan = frame_plan(&ctx, 500_000, FormatKind::Landscape);
    let f = backend
        .render(RenderRequest {
            plan,
            width: 1920,
            height: 1080,
            output: Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    ExportFrame {
        rgba: packed(&f.bytes, f.stride, (1920, 1080)),
        submissions: backend.checked_pass_submissions(),
    }
}

/// An export job's frames: the export worker's renderer with the job's registry (`export::contrib::job_registry`).
pub struct ExportJobFrames {
    backend: GpuBackend,
    registry: Arc<Registry>,
    project: Arc<Project>,
    _dir: tempfile::TempDir,
}

/// An export job whose one contribution is the checked filter `body` (`lib.k3f9q2ma/effect.halo.v1`, padded by 16 px)
/// with a Light verdict, on the red image of [`headless_with_checked_filter`].
pub async fn export_job_with_checked_filter(body: &str) -> ExportJobFrames {
    use klipolo_engine::export::contrib::{JobContribs, job_registry, write_job_contribs};
    let dir = tempfile::tempdir().unwrap();
    let id = ContribId::new("lib.k3f9q2ma", "effect.halo.v1");
    let def = ContribDef {
        padding: Some(PaddingSpec {
            px: 16.0,
            param: None,
            per_unit: 0.0,
            max_px: 64.0,
        }),
        ..ContribDef::new(id.as_str(), ContribKind::Effect, "Halo")
    };
    let c = loaded(&id, &def, body);
    let contribs = JobContribs {
        runnable: vec![(c, Verdict::of(Cost::with_export_ms(1.0)).unwrap())],
        withheld: Vec::new(),
    };
    let job = dir.path().join("job");
    std::fs::create_dir_all(&job).unwrap();
    write_job_contribs(&job, &contribs).unwrap();
    let registry = job_registry(&job);
    assert!(registry.shader(&id).is_some(), "the job draws it");
    let project = image_project(
        dir.path(),
        [255, 0, 0],
        (400, 300, 100),
        vec![EffectInstance {
            id: "halo".into(),
            ..EffectInstance::new(id.clone())
        }],
    );
    let dev = RenderDevice::new().await.unwrap();
    ExportJobFrames {
        backend: GpuBackend::for_export_on(dev),
        registry,
        project: Arc::new(project),
        _dir: dir,
    }
}

impl ExportJobFrames {
    /// Frame `n` at 30 fps, 1920 × 1080, rows packed.
    pub async fn render_frame(&mut self, n: u32) -> Vec<u8> {
        let ctx = PlanContext::with_registry(self.project.clone(), self.registry.clone());
        let plan = frame_plan(&ctx, i64::from(n) * 1_000_000 / 30, FormatKind::Landscape);
        let f = self
            .backend
            .render(RenderRequest {
                plan,
                width: 1920,
                height: 1080,
                output: Output::Rgba,
            })
            .await
            .unwrap()
            .finish()
            .await
            .unwrap();
        packed(&f.bytes, f.stride, (1920, 1080))
    }
}

/// `body` under `def` as a version folder loads (its hashes written).
fn loaded(id: &ContribId, def: &ContribDef, body: &str) -> LoadedContrib {
    let mut files = BTreeMap::new();
    files.insert("contrib.json".to_string(), serde_json::to_vec(def).unwrap());
    files.insert("effect.wgsl".to_string(), body.as_bytes().to_vec());
    let hashes: BTreeMap<&String, String> = files
        .iter()
        .map(|(n, b)| (n, hex(&Sha256::digest(b))))
        .collect();
    let hashes = serde_json::to_vec(&hashes).unwrap();
    files.insert("hashes.json".to_string(), hashes);
    load_files(id, files, PathBuf::new()).unwrap()
}

/// A checked white generator (`com.test.fx/generator.white`) in a 100 × 100 px box, under `count` checked filters
/// `com.test.fx/effect.p<i>`, each of its own text (each its own pipeline): a project and its registry.
pub fn project_with_many_checked_filters(count: usize) -> (Project, Arc<Registry>) {
    let names: Vec<String> = (0..count).map(|i| format!("p{i}")).collect();
    let bodies: Vec<String> = (0..count)
        .map(|i| {
            format!(
                "fn effect(uv: vec2<f32>) -> vec4<f32> {{ return kl_sample_a(uv) * {}.0 / 1000.0; }}",
                1000 - i
            )
        })
        .collect();
    let filters: Vec<(&str, &str, Option<f32>)> = names
        .iter()
        .zip(&bodies)
        .map(|(n, b)| (n.as_str(), b.as_str(), None))
        .collect();
    let registry = test_registry(&filters);
    let effects = names.iter().map(|n| fx(n)).collect();
    let p = super::generator_project(bx_px(400, 300, 100, 100), 0.0, effects);
    (p, registry)
}
