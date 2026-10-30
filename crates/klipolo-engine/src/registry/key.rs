//! A contribution's key (whose `Compiled` reply
//! carries the key it measured, m11): defined once, used by drafts, the library, embedding, the worker and the verified
//! store. SHA-256 over, for a shader, the effect ABI it was checked under (`EFFECT_ABI` and, 
//! `EFFECT_ABI_MINOR`, so a minor ABI change checks every shader again), then the canonical JSON of the definition's
//! rendering fields (`kind`, `params`, `padding`, `resolution`, `fps`, `downsample`, `protective`, in this struct's
//! order; names, description, tags, blend and provenance don't change what is drawn), a `0x00` byte, then the source
//! exactly as stored (WGSL bytes, or a scene's `scene.json` bytes, never re-serialised).

use klipolo_model::contrib::{ContribDef, ContribKind, DownsampleSpec, PaddingSpec, Resolution};
use klipolo_model::params::ParamSpec;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{EFFECT_ABI, EFFECT_ABI_MINOR};

/// A contribution's source as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContribSource {
    Wgsl(String),
    Scene(Vec<u8>),
}

#[derive(Serialize)]
struct Rendering<'a> {
    kind: ContribKind,
    params: &'a [ParamSpec],
    padding: &'a Option<PaddingSpec>,
    resolution: &'a Option<Resolution>,
    fps: &'a Option<u32>,
    downsample: &'a Option<DownsampleSpec>,
    protective: bool,
}

/// The key of `def` with `source`.
pub fn contrib_key(def: &ContribDef, source: &ContribSource) -> [u8; 32] {
    let rendering = Rendering {
        kind: def.kind,
        params: &def.params,
        padding: &def.padding,
        resolution: &def.resolution,
        fps: &def.fps,
        downsample: &def.downsample,
        protective: def.protective,
    };
    let json = serde_json::to_vec(&rendering).unwrap_or_default();
    let mut hash = Sha256::new();
    if let ContribSource::Wgsl(_) = source {
        hash.update(format!("abi{EFFECT_ABI}.{EFFECT_ABI_MINOR}\0"));
    }
    hash.update(&json);
    hash.update([0u8]);
    match source {
        ContribSource::Wgsl(text) => hash.update(text.as_bytes()),
        ContribSource::Scene(bytes) => hash.update(bytes),
    }
    hash.finalize().into()
}
