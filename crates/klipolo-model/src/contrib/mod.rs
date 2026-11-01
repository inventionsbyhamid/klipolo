//! Contributions: looks, motion presets, audio effects, visual effects, generators and
//! transitions. They are built in now and come from plugins later, addressed by namespaced ids
//! `<plugin>/<contribution>` such as `klipolo.builtin/look.warm`. Projects store ids and parameter values; the
//! [`Catalog`] says what an id is. An id the catalog doesn't know (a plugin that isn't installed, a built-in from a
//! newer Klipolo) passes validation, stays in the project untouched and draws as nothing, with a note from the engine.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::motion::Blend;
use crate::params::{ParamKind, ParamSpec, VisibleWhen};
use crate::project::Project;

mod meta;
mod versions;
pub use meta::{
    PaddingSpec, Provenance, Resolution, GENERATOR_SOURCES, LIBRARY_PREFIX, MAX_PADDING_PX,
};

/// The plugin id of everything that ships inside Klipolo.
pub const BUILTIN_PLUGIN: &str = "klipolo.builtin";
/// `pack://<plugin>@<version>/<path>`: a file inside an installed pack.
pub const PACK_SCHEME: &str = "pack://";
/// Project features (`Project::requires`) this build reads and writes without losing anything.
pub const FEATURES: &[&str] = &["effects@1", "transitions@1", "scenes@1"];

/// `<plugin>/<contribution>`.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    TS,
    schemars::JsonSchema,
)]
#[ts(export)]
pub struct ContribId(pub String);

impl ContribId {
    pub fn new(plugin: &str, contribution: &str) -> Self {
        Self(format!("{plugin}/{contribution}"))
    }

    pub fn builtin(contribution: &str) -> Self {
        Self::new(BUILTIN_PLUGIN, contribution)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The plugin part: `klipolo.builtin`, `com.acme.neon`.
    pub fn plugin(&self) -> &str {
        self.0.split_once('/').map_or("", |(plugin, _)| plugin)
    }

    pub fn is_builtin(&self) -> bool {
        self.plugin() == BUILTIN_PLUGIN
    }

    /// A reverse-DNS plugin id (two or more dot-separated parts of a–z, 0–9 and `-`), a slash, then a contribution
    /// id of a–z, 0–9, `.` and `-` starting with a letter or digit; 128 characters at most.
    pub fn is_well_formed(&self) -> bool {
        let Some((plugin, local)) = self.0.split_once('/') else {
            return false;
        };
        let part = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        self.0.len() <= 128
            && plugin.split('.').count() >= 2
            && plugin.split('.').all(part)
            && local
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && local
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
    }
}

impl fmt::Display for ContribId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ContribId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ContribKind {
    Look,
    /// Layer motion presets: enter, exit and loop animations.
    Motion,
    AudioEffect,
    Effect,
    Generator,
    Transition,
    /// Caption styles: their definitions are style files (`captions::styles`), the catalogue names
    /// them.
    CaptionStyle,
    /// Motion graphics (`scenes@1`): data drawn by Klipolo's own code on a generator layer
    /// (`GENERATOR_SOURCES`).
    Scene,
}

/// What one contribution is: its kind, its name and its parameters, and what an agent-made one says about
/// itself. `ContribDef::new` and struct update build one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContribDef {
    pub id: ContribId,
    pub kind: ContribKind,
    pub name: String,
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub downsample: Option<DownsampleSpec>,
    /// Set on a contribution that hides what it covers (a blur, a pixelation, a shape mask): another app's changes to
    /// parts of the video under one always wait for the person, whatever that app may do. Plugins declare
    /// it the same way. Left out of the JSON when unset.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub protective: bool,
    /// What it does, in the maker's words (the person's content).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// How far a filter draws past its layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub padding: Option<PaddingSpec>,
    /// The size a generator renders at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resolution: Option<Resolution>,
    /// 1 to 60: the pass renders again only when `floor(time × fps)` changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fps: Option<u32>,
    /// The blend a new layer of it starts with (Film grain `overlay`, Light leak `screen`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub blend: Option<Blend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provenance: Option<Provenance>,
}

/// A filter that may run at half size once one of its settings makes the result smooth anyway:
/// from `at_least_px` output pixels of the number setting `param`, the host halves the pass and the rest of the
/// stack. Data on the contribution, so built-ins take the path packs will; in effect ABI v1 only built-ins may
/// declare it; whether pack manifests may is still open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DownsampleSpec {
    pub param: String,
    pub at_least_px: f32,
}

impl ContribDef {
    pub fn param(&self, key: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.key == key)
    }

    /// Checks every setting's `visible_when` when the definition loads: it names another choice setting of this
    /// contribution, lists only that setting's options (at least one), and no setting depends on itself through a
    /// chain of conditions. O(n) in the settings, once per definition.
    pub fn check_conditions(&self) -> Result<(), String> {
        let id = &self.id;
        // Key → index, first wins like `param`.
        let mut index: HashMap<&str, usize> = HashMap::with_capacity(self.params.len());
        for (i, spec) in self.params.iter().enumerate() {
            index.entry(spec.key.as_str()).or_insert(i);
        }
        // What each setting depends on, checked.
        let mut parent: Vec<Option<usize>> = Vec::with_capacity(self.params.len());
        for spec in &self.params {
            let Some(VisibleWhen { key, values }) = &spec.visible_when else {
                parent.push(None);
                continue;
            };
            let param = &spec.key;
            let Some((i, ParamKind::Choice { options, .. })) =
                index.get(key.as_str()).map(|&i| (i, &self.params[i].kind))
            else {
                return Err(format!(
                    "{id}: \"{param}\" depends on \"{key}\", which isn't a choice setting of this contribution."
                ));
            };
            if values.is_empty() {
                return Err(format!(
                    "{id}: \"{param}\" lists no value of \"{key}\" that shows it."
                ));
            }
            if let Some(value) = values.iter().find(|v| !options.contains(v)) {
                return Err(format!(
                    "{id}: \"{param}\" depends on \"{key}\" being \"{value}\", which isn't one of its options."
                ));
            }
            parent.push(Some(i));
        }
        // Each setting depends on at most one other, so following the chain from every setting once finds a loop:
        // reaching a setting still on the current chain.
        #[derive(Clone, Copy, PartialEq)]
        enum Seen {
            No,
            OnChain,
            Done,
        }
        let mut seen = vec![Seen::No; self.params.len()];
        let mut chain = Vec::new();
        for start in 0..self.params.len() {
            let mut at = Some(start);
            while let Some(i) = at {
                match seen[i] {
                    Seen::Done => break,
                    Seen::OnChain => {
                        return Err(format!(
                            "{id}: \"{}\" depends on itself through its settings' conditions.",
                            self.params[i].key
                        ));
                    }
                    Seen::No => {
                        seen[i] = Seen::OnChain;
                        chain.push(i);
                        at = parent[i];
                    }
                }
            }
            for i in chain.drain(..) {
                seen[i] = Seen::Done;
            }
        }
        Ok(())
    }
}

/// Every installed contribution. Built-ins now; plugin definitions come later, through [`Catalog::with`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    defs: BTreeMap<ContribId, ContribDef>,
}

impl Catalog {
    pub fn builtin() -> &'static Catalog {
        static BUILTIN: OnceLock<Catalog> = OnceLock::new();
        BUILTIN.get_or_init(builtin::catalog)
    }

    pub fn with(mut self, def: ContribDef) -> Self {
        self.defs.insert(def.id.clone(), def);
        self
    }

    pub fn get(&self, id: &ContribId) -> Option<&ContribDef> {
        self.defs.get(id)
    }

    /// Every definition, by id.
    pub fn defs(&self) -> impl Iterator<Item = &ContribDef> {
        self.defs.values()
    }

    pub fn of_kind(&self, kind: ContribKind) -> impl Iterator<Item = &ContribDef> {
        self.defs.values().filter(move |d| d.kind == kind)
    }
}

/// What the app says once when it opens a project from a newer Klipolo (`read_only_notice`), and when an edit still
/// arrives: a code the UI words.
pub const READ_ONLY_NOTICE: &str = crate::messages::code!("model.readOnly");

/// `READ_ONLY_NOTICE` when `p` needs features this build doesn't have: it opens, but edits are refused so that
/// saving can't drop what this build doesn't understand.
pub fn read_only_notice(p: &Project) -> Option<&'static str> {
    (!unsupported_requirements(p).is_empty()).then_some(READ_ONLY_NOTICE)
}

/// Requirements in `p.requires` this build doesn't know. Such a project came from a newer Klipolo and may hold data
/// this build would drop when saving, so the app opens it read-only.
pub fn unsupported_requirements(p: &Project) -> Vec<String> {
    p.requires
        .iter()
        .filter(|r| !FEATURES.contains(&r.as_str()))
        .cloned()
        .collect()
}

/// Where an asset's (or LUT's) file is: a path on this Mac, or a file inside an installed pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetPath<'a> {
    File(&'a str),
    Pack {
        plugin: &'a str,
        version: &'a str,
        path: &'a str,
    },
}

impl<'a> AssetPath<'a> {
    /// None for an empty path, or a pack reference without a plugin, version or file, or whose file path is
    /// absolute or climbs out of the pack (`.`, `..`, empty segments). A pack reference never holds a backslash, a
    /// colon (so no drive letter like `C:`) or NUL: Windows would read them as separators or a drive.
    pub fn parse(s: &'a str) -> Option<Self> {
        let Some(rest) = s.strip_prefix(PACK_SCHEME) else {
            return (!s.trim().is_empty()).then_some(Self::File(s));
        };
        if rest.contains(['\\', ':', '\0']) {
            return None;
        }
        let (pack, path) = rest.split_once('/')?;
        let (plugin, version) = pack.split_once('@')?;
        let inside = path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..");
        (!plugin.is_empty() && !version.is_empty() && inside).then_some(Self::Pack {
            plugin,
            version,
            path,
        })
    }
}

pub mod builtin;
