//! The draft layer: what the worker draws to check, measure and preview one shader or one
//! scene alone. A one-layer project, drawn by the worker's transparent `Headless`: a generator item over the whole
//! canvas (a scene's too), a filter on a fixed 50 %-grey-and-white test image at 70 % of the canvas, or a transition
//! between two test stills (served by `FrozenStills`, so nothing is decoded). Its baseline is the same project without
//! the pass (a generator's and a scene's: an empty frame; a filter's: the image alone; a transition's: the cut), so a
//! layer's cost is the frame minus the baseline.
//!
//! The shader is always rebuilt from the definition and body with `assemble_untrusted`: no hardened text is
//! ever read from anywhere, and the `ShaderEffect`s live in memory only. Measurements draw a copy that never caches or
//! steps (`measured`), so every measured frame draws its pass; previews draw the shader as it is. A scene draft
//! (`scene`) is the worker's own compile of the bytes it checked (`scene_check`), drawn by a registry holding it and the
//! generators its fills name; it never runs at half size.

use std::path::Path;
use std::sync::Arc;

use klipolo_model::contrib::{ContribDef, ContribKind, Resolution};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::time::{Us, secs_to_us};
use sha2::{Digest, Sha256};

mod build;
use build::{cut, test_image, transition_stills};

use klipolo_model::scene::timeline::CompiledScene;

use super::audit::AuditCache;
use super::envelope::Params;
use super::protocol::Key;
use crate::headless::Headless;
use crate::media::frozen::FrozenStills;
use crate::registry::guard::{Diagnostic, StaticFacts};
use crate::registry::{Registry, ShaderEffect, ShaderKind};

mod scene;

/// How long a draft layer lasts: `kl.progress` runs across it.
pub const DRAFT_SECONDS: f64 = 5.0;
/// The filters' test image, written once into the worker's `tmp/`.
pub const TEST_IMAGE: &str = "klipolo-test-image.png";
const TEST_SIZE: (u32, u32) = (1280, 720);
/// A transition's window in its draft: 1 s across the cut at 2.5 s.
const TRANSITION_US: Us = 1_000_000;
const SIDE_A: &str = "klipolo-draft-a";
const SIDE_B: &str = "klipolo-draft-b";

/// What a draft layer draws.
#[derive(Clone)]
pub enum Body {
    /// A shader, checked and hardened.
    Shader(Arc<ShaderEffect>),
    /// A scene the worker checked and compiled itself, drawn as a generator layer is.
    Scene(Arc<CompiledScene>),
}

/// One shader or scene, checked and ready to draw at its settings.
#[derive(Clone)]
pub struct DraftLayer {
    pub def: Arc<ContribDef>,
    /// How it draws: a scene draws as a generator layer.
    pub kind: ShaderKind,
    pub body: Body,
    /// `Checked::notes`: the worker's replies forward them.
    pub notes: Arc<[Diagnostic]>,
    /// SHA-256 of the hardened text, so the verified store notices a naga update that changes it under the same key.
    pub hardened: Key,
    /// The settings it draws at (its defaults under these).
    pub params: Params,
    pub format: FormatKind,
    pub seconds: f64,
    registry: Arc<Registry>,
    measured: Arc<Registry>,
    halved: Option<Arc<Registry>>,
    stills: Option<Arc<FrozenStills>>,
    image: Arc<Path>,
    /// Its audit variants' compiled objects, kept for the request.
    pub audit: Arc<AuditCache>,
}

impl DraftLayer {
    /// `body` under `def`, checked and hardened (every refusal comes back as the guard's diagnostics), at `params`.
    /// `tmp` is the worker's own folder for the test image.
    pub fn new(
        def: &ContribDef,
        body: &str,
        params: Params,
        format: FormatKind,
        tmp: &Path,
    ) -> Result<Self, Vec<Diagnostic>> {
        let kind = shader_kind(def.kind).ok_or_else(|| {
            vec![Diagnostic::error(
                "rule.kind",
                "Only filters, generators and transitions are shaders.",
            )]
        })?;
        let (shader, notes) = ShaderEffect::assemble_untrusted_with_notes(def, kind, body)?;
        let hardened = Key(Sha256::digest(shader.source.as_bytes()).into());
        let image = test_image(tmp).map_err(|e| {
            vec![Diagnostic::error(
                "worker.image",
                format!("The checker's test image couldn't be made: {e}"),
            )]
        })?;
        let measured = ShaderEffect {
            animates: true,
            fps: None,
            ..shader.clone()
        };
        let may_halve = match kind {
            ShaderKind::Generator => matches!(shader.resolution, Resolution::Auto),
            ShaderKind::Filter => shader.downsample.is_some(),
            ShaderKind::Transition => false,
        };
        let halved = may_halve.then(|| {
            let mut half = measured.clone();
            match kind {
                ShaderKind::Generator => half.resolution = Resolution::Half,
                _ => {
                    if let Some(d) = &mut half.downsample {
                        d.at_least_px = 0.0;
                    }
                }
            }
            Registry::with_checked([(def.clone(), half)])
        });
        let stills = (kind == ShaderKind::Transition).then(|| Arc::new(transition_stills()));
        Ok(Self {
            registry: Registry::with_checked([(def.clone(), shader.clone())]),
            measured: Registry::with_checked([(def.clone(), measured)]),
            halved,
            stills,
            def: Arc::new(def.clone()),
            kind,
            body: Body::Shader(Arc::new(shader)),
            notes: notes.into(),
            hardened,
            params,
            format,
            seconds: DRAFT_SECONDS,
            image,
            audit: Arc::default(),
        })
    }

    /// A built-in shader, `id`, as a draft layer at `params`: what the budget's calibration measures beside agent-made
    /// shaders. Its measured copy never caches or steps, and a filter with a `downsample` is
    /// also measured on its downsampled path, as a draft's. Only in builds with the `test-support` feature, which the
    /// engine's own tests turn on.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn builtin(
        id: &klipolo_model::contrib::ContribId,
        params: Params,
        format: FormatKind,
        tmp: &Path,
    ) -> Option<Self> {
        let registry = Registry::builtin();
        let shader = registry.shader(id)?.clone();
        let def = registry.catalog().get(id)?.clone();
        let kind = shader_kind(def.kind)?;
        let measured = ShaderEffect {
            animates: true,
            fps: None,
            ..(*shader).clone()
        };
        let halved = (kind == ShaderKind::Filter && measured.downsample.is_some()).then(|| {
            let mut half = measured.clone();
            if let Some(d) = &mut half.downsample {
                d.at_least_px = 0.0;
            }
            Registry::builtin_with_test_shader(&def, half)
        });
        Some(Self {
            measured: Registry::builtin_with_test_shader(&def, measured),
            registry,
            halved,
            stills: (kind == ShaderKind::Transition).then(|| Arc::new(transition_stills())),
            hardened: Key(Sha256::digest(shader.source.as_bytes()).into()),
            def: Arc::new(def),
            kind,
            body: Body::Shader(shader),
            notes: Vec::new().into(),
            params,
            format,
            seconds: DRAFT_SECONDS,
            image: test_image(tmp).ok()?,
            audit: Arc::default(),
        })
    }

    /// The shader it draws (None for a scene).
    pub fn shader(&self) -> Option<&Arc<ShaderEffect>> {
        match &self.body {
            Body::Shader(shader) => Some(shader),
            Body::Scene(_) => None,
        }
    }

    /// The scene it draws (None for a shader).
    pub fn scene(&self) -> Option<&Arc<CompiledScene>> {
        match &self.body {
            Body::Scene(scene) => Some(scene),
            Body::Shader(_) => None,
        }
    }

    /// Its shader's static facts (None for a scene, whose own are its check's).
    pub fn facts(&self) -> Option<&StaticFacts> {
        self.shader().and_then(|s| s.facts.as_deref())
    }

    /// The size its shader renders at (a scene: full size).
    pub fn resolution(&self) -> Resolution {
        self.shader().map_or(Resolution::Full, |s| s.resolution)
    }

    /// The same layer at `params`.
    pub fn at(&self, params: Params) -> Self {
        Self {
            params,
            ..self.clone()
        }
    }

    /// Whether it may run at half size (a generator with `resolution: half` or `auto`, a filter with a
    /// `downsample`), and so be Heavy.
    pub fn may_halve(&self) -> bool {
        self.halved.is_some() || self.half_always()
    }

    /// A `resolution: half` generator: it always draws at half size, so its preview measurement is its half one.
    pub fn half_always(&self) -> bool {
        self.kind == ShaderKind::Generator && self.resolution() == Resolution::Half
    }

    /// Points `h` at this layer: its measured copy (`measured`, at half size with `half`) or the shader as it is, and its
    /// stills.
    pub fn prepare(&self, h: &mut Headless, measured: bool, half: bool) {
        let registry = match (measured, half, &self.halved) {
            (true, true, Some(halved)) => halved,
            (true, _, _) => &self.measured,
            _ => &self.registry,
        };
        h.set_registry(registry.clone());
        h.set_stills(self.stills.clone());
        h.set_transparent(true);
    }

    /// The one-layer project at its settings.
    pub fn project(&self) -> Project {
        self.build(true)
    }

    /// The same without the pass.
    pub fn baseline(&self) -> Project {
        self.build(false)
    }

    /// `n` times spread across where the pass draws (a transition's window, else the whole layer).
    pub fn times(&self, n: usize) -> Vec<Us> {
        let (start, span) = match self.kind {
            ShaderKind::Transition => (cut() - TRANSITION_US / 2, TRANSITION_US),
            _ => (0, secs_to_us(self.seconds)),
        };
        (0..n.max(1))
            .map(|i| start + ((i as f64 + 0.5) / n.max(1) as f64 * span as f64) as Us)
            .collect()
    }

    /// The middle of where it draws (the thumbnail's time).
    pub fn middle(&self) -> Us {
        self.times(1)[0]
    }
}

/// The shader kind a contribution kind draws as.
pub fn shader_kind(kind: ContribKind) -> Option<ShaderKind> {
    match kind {
        ContribKind::Generator => Some(ShaderKind::Generator),
        ContribKind::Effect => Some(ShaderKind::Filter),
        ContribKind::Transition => Some(ShaderKind::Transition),
        _ => None,
    }
}
