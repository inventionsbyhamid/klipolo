//! Strings the plans of one project share. Every id, path, text and text style a plan carries is an `Arc` made the
//! first time it's seen, so planning a frame copies no strings: a lookup by `&str`, then a reference-count bump.
//! Contexts of later revisions keep the table; a new project starts a new one.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_model::params::{ParamKind, ParamSpec, ParamValue};
use klipolo_model::project::{AssetKind, Project};
use klipolo_model::timeline::TextStyle;

use super::effects::clamped;
use super::{backdrop_layer_id, camera_layer_id, cursor_layer_id};
use crate::make::envelope::Envelope;

/// Entries kept before the table starts over (text being typed makes a new string per keystroke).
const MAX_ENTRIES: usize = 16_384;

#[derive(Default)]
pub struct Strings(Mutex<Table>);

#[derive(Default)]
pub struct Table {
    strs: HashMap<Box<str>, Arc<str>>,
    cameras: HashMap<Box<str>, Arc<str>>,
    backdrops: HashMap<Box<str>, Arc<str>>,
    cursors: HashMap<Box<str>, Arc<str>>,
    /// The last style of each text item.
    styles: HashMap<Box<str>, Arc<TextStyle>>,
    /// The last settings of each scene layer.
    scenes: HashMap<Box<str>, KeptScene>,
}

/// A scene layer's settings as drawn (held inside its scene's envelope), their hash and the files its image
/// settings name, shared while none of what they come from changes; and whether they were held.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneInputs {
    pub params: Arc<BTreeMap<String, ParamValue>>,
    pub hash: u64,
    pub images: Arc<[(Arc<str>, Arc<str>)]>,
    pub clamped: bool,
}

/// A scene layer's inputs and what they were made from: the layer's own settings (the same map as `inputs.params`
/// unless they were held) and the envelope they were held to.
struct KeptScene {
    raw: Arc<BTreeMap<String, ParamValue>>,
    envelope: Option<Arc<Envelope>>,
    inputs: SceneInputs,
}

impl Strings {
    pub fn lock(&self) -> MutexGuard<'_, Table> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Table {
    pub fn str(&mut self, s: &str) -> Arc<str> {
        if let Some(shared) = self.strs.get(s) {
            return shared.clone();
        }
        if self.strs.len() >= MAX_ENTRIES {
            self.strs.clear();
        }
        let shared: Arc<str> = s.into();
        self.strs.insert(s.into(), shared.clone());
        shared
    }

    /// A file path as the decoders will open it (`Registry::resolve_path`), shared like `str`.
    pub fn path(&mut self, path: &str, registry: &crate::registry::Registry) -> Arc<str> {
        if path.starts_with(klipolo_model::contrib::PACK_SCHEME) {
            let resolved = registry.resolve_path(path);
            self.str(&resolved)
        } else {
            self.str(path)
        }
    }

    /// `camera:<clip id>` (`camera_layer_id`).
    pub fn camera(&mut self, clip_id: &str) -> Arc<str> {
        per_clip(&mut self.cameras, clip_id, camera_layer_id)
    }

    /// `backdrop:<clip id>` (`backdrop_layer_id`).
    pub fn backdrop(&mut self, clip_id: &str) -> Arc<str> {
        per_clip(&mut self.backdrops, clip_id, backdrop_layer_id)
    }

    /// `cursor:<clip id>` (`cursor_layer_id`).
    pub fn cursor(&mut self, clip_id: &str) -> Arc<str> {
        per_clip(&mut self.cursors, clip_id, cursor_layer_id)
    }

    /// Scene layer `item_id`'s settings (`params`, under `specs`), held inside `envelope` (what its verdict measured:
    /// `effects::clamped`), shared while they, the envelope and the files of its image settings stay the
    /// same: a comparison per frame, a copy, the clamp and a hash only when one changes, so a held layer copies nothing
    /// a frame either.
    pub fn scene_inputs(
        &mut self,
        item_id: &str,
        specs: &[ParamSpec],
        params: &BTreeMap<String, ParamValue>,
        envelope: Option<&Arc<Envelope>>,
        project: &Project,
        registry: &crate::registry::Registry,
    ) -> SceneInputs {
        let image = |key: &str| match params.get(key) {
            Some(ParamValue::Text(id)) => project
                .asset(id)
                .filter(|a| a.kind == AssetKind::Image)
                .map(|a| (id.as_str(), a.path.as_str())),
            _ => None,
        };
        let images = || {
            specs
                .iter()
                .filter(|s| matches!(s.kind, ParamKind::Asset { .. }))
                .filter_map(|s| image(&s.key))
        };
        if let Some(kept) = self.scenes.get(item_id) {
            let same_envelope = match (&kept.envelope, envelope) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            let images_kept = &kept.inputs.images;
            let same_files = images_kept.len() == images().count()
                && images_kept
                    .iter()
                    .zip(images())
                    .all(|((a, p), (b, q))| **a == *b && registry.resolve_path(q) == **p);
            if *kept.raw == *params && same_envelope && same_files {
                return kept.inputs.clone();
            }
        }
        if self.scenes.len() >= MAX_ENTRIES {
            self.scenes.clear();
        }
        let files: Vec<(Arc<str>, Arc<str>)> = images()
            .map(|(id, path)| (self.str(id), self.path(path, registry)))
            .collect();
        let drawn = clamped(params, specs, envelope.map(|e| &**e));
        let held = matches!(drawn, Cow::Owned(_));
        let drawn = Arc::new(drawn.into_owned());
        let raw = if held {
            Arc::new(params.clone())
        } else {
            drawn.clone()
        };
        let inputs = SceneInputs {
            hash: super::scene::params_hash(&drawn),
            params: drawn,
            images: files.into(),
            clamped: held,
        };
        let kept = KeptScene {
            raw,
            envelope: envelope.cloned(),
            inputs: inputs.clone(),
        };
        self.scenes.insert(item_id.into(), kept);
        inputs
    }

    /// The text item's style, shared while it doesn't change.
    pub fn style(&mut self, item_id: &str, style: &TextStyle) -> Arc<TextStyle> {
        match self.styles.get(item_id) {
            Some(shared) if **shared == *style => shared.clone(),
            _ => {
                if self.styles.len() >= MAX_ENTRIES {
                    self.styles.clear();
                }
                let shared = Arc::new(style.clone());
                self.styles.insert(item_id.into(), shared.clone());
                shared
            }
        }
    }
}

/// A clip's `id(clip_id)`, made the first time it's asked for.
fn per_clip(
    ids: &mut HashMap<Box<str>, Arc<str>>,
    clip_id: &str,
    id: fn(&str) -> String,
) -> Arc<str> {
    if let Some(shared) = ids.get(clip_id) {
        return shared.clone();
    }
    if ids.len() >= MAX_ENTRIES {
        ids.clear();
    }
    let shared: Arc<str> = id(clip_id).into();
    ids.insert(clip_id.into(), shared.clone());
    shared
}
