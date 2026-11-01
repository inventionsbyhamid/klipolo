//! What contributions draw and sound like. The model's `Catalog` says what an id is; the
//! registry says how the engine renders it: WGSL effects, generators and transitions on the effect ABI, look grades,
//! motion presets and audio filter chains. Built-ins, packs and shader plugins share the same tables.
//! The shader presets are built-ins read from their data folders (`presets`).
//! Pure data with no GPU code: `PlanContext` carries an `Arc<Registry>` into the pure `FramePlan`, and the export
//! worker builds the same one.

mod builtins;
pub mod guard;
pub mod key;
pub mod pack;
pub mod pack_scene;
pub mod presets;
mod scenes;
pub mod shader;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use klipolo_model::contrib::{AssetPath, Catalog, ContribId};

use builtins::build_builtin;
pub use builtins::builtin_builds;
use klipolo_model::scene::timeline::CompiledScene;
use pack_scene::VerifiedScene;
pub use scenes::compile_scene;

pub use shader::{
    Downsample, EFFECT_ABI, EFFECT_ABI_MINOR, MAX_SHADER_PARAMS, MAX_STEP_FPS, ParamLayout,
    SANITIZE, ShaderEffect, ShaderKind, ShaderTrust, Slot, fnv1a64, fnv1a64_on, validate_wgsl,
};

/// A look as Cap's parametric grade; `render::look::grade` scales it by the clip's strength.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LookGrade {
    pub temperature: f32,
    pub tint: f32,
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub fade: f32,
    pub split_tone: f32,
    pub vignette: f32,
    /// Film grain at full strength.
    pub grain: f32,
}

/// How a built-in motion preset moves a layer (`plan::motion`): the enter and exit presets, then the loops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Fade,
    Slide,
    Pop,
    Zoom,
    Drop,
    Bounce,
    BlurIn,
    Typewriter,
    Float,
    Pulse,
    Shake,
    Wiggle,
    Spin,
}

#[derive(Clone)]
pub struct Registry {
    catalog: Catalog,
    shaders: HashMap<ContribId, Arc<ShaderEffect>>,
    /// Motion graphics (`scene@1`), compiled once and shared by every layer that shows one (`with_scenes`; verified
    /// ones through `with_contributions`, with the settings their verdict measured).
    scenes: HashMap<ContribId, VerifiedScene>,
    /// The built-in blur, also run by the `blur-in` preset (found without building its id each frame).
    blur: Arc<ShaderEffect>,
    looks: HashMap<ContribId, LookGrade>,
    motion: HashMap<ContribId, Preset>,
    audio: HashMap<ContribId, &'static str>,
    /// Installed packs: `plugin@version` → folder.
    packs: HashMap<String, PathBuf>,
}

impl Registry {
    /// Everything that ships inside Klipolo, built once per process.
    pub fn builtin() -> Arc<Registry> {
        static BUILTIN: OnceLock<Arc<Registry>> = OnceLock::new();
        BUILTIN.get_or_init(|| Arc::new(build_builtin())).clone()
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn shader(&self, id: &ContribId) -> Option<&Arc<ShaderEffect>> {
        self.shaders.get(id)
    }

    /// The compiled scene a generator layer naming `id` draws: None for one this registry doesn't have (or doesn't
    /// run yet: a scene is added only once it is verified), which draws as nothing.
    pub fn scene(&self, id: &ContribId) -> Option<&Arc<CompiledScene>> {
        self.scenes.get(id).map(|s| &s.scene)
    }

    /// The settings scene `id`'s verdict measured (None: a built-in or a test's scene, drawn as asked).
    pub fn scene_envelope(&self, id: &ContribId) -> Option<&Arc<crate::make::envelope::Envelope>> {
        self.scenes.get(id)?.envelope.as_ref()
    }

    /// The built-in blur (`builtin::EFFECT_BLUR`).
    pub fn blur(&self) -> &Arc<ShaderEffect> {
        &self.blur
    }

    pub fn shaders(&self) -> impl Iterator<Item = &Arc<ShaderEffect>> {
        self.shaders.values()
    }

    pub fn look(&self, id: &ContribId) -> Option<LookGrade> {
        self.looks.get(id).copied()
    }

    /// A motion preset this build draws; None for one it doesn't (a plugin's, or a newer built-in), which draws as
    /// nothing. A hash lookup: no allocation (the plan asks per moving layer per frame).
    pub fn motion_preset(&self, id: &ContribId) -> Option<Preset> {
        self.motion.get(id).copied()
    }

    /// The FFmpeg filter chain of an audio effect (built-in presets only, for now).
    pub fn audio_filter(&self, id: &ContribId) -> Option<&'static str> {
        self.audio.get(id).copied()
    }

    /// A path the decoders can open. A `pack://` path becomes the file inside the installed pack; anything else,
    /// including a pack that isn't installed (which then reports as missing media), comes back unchanged.
    pub fn resolve_path<'a>(&self, path: &'a str) -> Cow<'a, str> {
        match AssetPath::parse(path) {
            Some(AssetPath::Pack {
                plugin,
                version,
                path: inner,
            }) => match self.packs.get(&format!("{plugin}@{version}")) {
                Some(dir) => Cow::Owned(dir.join(inner).to_string_lossy().into_owned()),
                None => Cow::Borrowed(path),
            },
            _ => Cow::Borrowed(path),
        }
    }
}
