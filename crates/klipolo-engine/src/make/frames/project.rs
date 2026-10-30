//! A project's frames in the worker: the snapshot, the contributions and the stills the app wrote into
//! `in/`, and nothing else. Every asset and LUT path is rewritten into the stills folder (an image or LUT the app
//! copied) or onto a name that never exists in `in/` (anything else: it draws as missing), so the worker opens no file
//! of the person's; video frames come from `FrozenStills`. The contributions are checked and hardened again here from
//! their definition and source, drawn as their verified `runsAt` says, and every setting the project gives
//! them is clamped to their envelope, as the preview draws them. Scenes are compiled here from their bytes: a library
//! K4): a library or project one the app verified, and the draft being previewed only when this worker checked its key
//! this session (else it draws nothing, noted as being checked).

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use klipolo_model::contrib::{Catalog, ContribDef, ContribId, ContribKind, Resolution};
use klipolo_model::project::{AssetKind, Project};
use klipolo_model::timeline::ItemContent;
use serde::{Deserialize, Serialize};

use crate::api::FRAME_AGENT_EFFECT_CHECKING;
use crate::make::budget::RunsAt;
use crate::make::draft::shader_kind;
use crate::make::envelope::{Envelope, clamp};
use crate::make::protocol::{InputName, Key};
use crate::media::frozen::{FrozenStills, read_index};
use crate::registry::guard::shown;
use crate::registry::key::{ContribSource, contrib_key};
use crate::registry::{Registry, ShaderEffect, ShaderKind, compile_scene};

/// The snapshot's and the contributions' files, at most.
const MAX_SNAPSHOT_BYTES: u64 = 16 << 20;
const MAX_CONTRIBS_BYTES: u64 = 4 << 20;
/// Contributions one project hands over, at most.
const MAX_CONTRIBS: usize = 64;

/// One non-built-in contribution of the snapshot, as the app hands it over: its definition and source as stored,
/// and what this computer verified, its `runsAt` and envelope. Never hardened text: it is checked again here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContribEntry {
    pub def: ContribDef,
    pub body: String,
    #[serde(default)]
    pub runs_at: RunsAt,
    #[serde(default)]
    pub envelope: Envelope,
    /// A scene draft the agent is previewing: drawn only if this worker checked its key this session.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub draft: bool,
}

/// What a project's frames draw with.
pub struct ProjectInputs {
    pub project: Arc<Project>,
    pub registry: Arc<Registry>,
    pub stills: Arc<FrozenStills>,
    /// Contributions left out (refused here) and media the app didn't hand over, for the reply's notes.
    pub notes: Vec<String>,
    /// The project's media by the paths it is drawn from (for notes naming what's missing).
    pub names: HashMap<String, String>,
}

/// The inputs `snapshot`, `contribs` and `stills` from `inputs` (the session's `in/`); `checked`, whether this worker
/// checked a scene draft's key this session.
pub fn load(
    inputs: &Path,
    (snapshot, contribs, stills): (&InputName, &InputName, &InputName),
    checked: &dyn Fn(&Key) -> bool,
) -> Result<ProjectInputs, String> {
    let mut project: Project =
        serde_json::from_slice(&read(&inputs.join(snapshot.as_str()), MAX_SNAPSHOT_BYTES)?)
            .map_err(|e| format!("the project snapshot isn't valid: {e}"))?;
    let entries: Vec<ContribEntry> =
        serde_json::from_slice(&read(&inputs.join(contribs.as_str()), MAX_CONTRIBS_BYTES)?)
            .map_err(|e| format!("the contributions aren't valid: {e}"))?;
    if entries.len() > MAX_CONTRIBS {
        return Err(format!("more than {MAX_CONTRIBS} contributions"));
    }
    let folder = inputs.join(stills.as_str());
    let index = read_index(&folder)?;
    let frozen = FrozenStills::load(&folder, &index)?;
    let mut notes = Vec::new();
    let mut shaders = Vec::new();
    let mut envelopes: HashMap<ContribId, (ContribDef, Envelope)> = HashMap::new();
    let mut scenes = Vec::new();
    for e in entries {
        if e.def.kind == ContribKind::Scene {
            scenes.push(e);
            continue;
        }
        let Some(kind) = shader_kind(e.def.kind) else {
            notes.push(format!("make.contribLeftOut: {}", short(e.def.id.as_str())));
            continue;
        };
        match ShaderEffect::assemble_untrusted(&e.def, kind, &e.body) {
            Ok(mut shader) => {
                if kind == ShaderKind::Generator && e.runs_at.half {
                    shader.resolution = Resolution::Half;
                }
                envelopes.insert(e.def.id.clone(), (e.def.clone(), e.envelope));
                shaders.push((e.def, shader));
            }
            Err(_) => notes.push(format!("make.contribLeftOut: {}", short(e.def.id.as_str()))),
        }
    }
    // After the generators: a scene's fills may name them.
    let catalog = shaders
        .iter()
        .fold(Catalog::builtin().clone(), |c, (def, _)| {
            c.with(def.clone())
        });
    let mut compiled = Vec::new();
    for e in scenes {
        let bytes = e.body.into_bytes();
        let key = Key(contrib_key(&e.def, &ContribSource::Scene(bytes.clone())));
        if e.draft && !checked(&key) {
            notes.push(draft_not_checked(e.def.id.as_str()));
            continue;
        }
        match compile_scene(&bytes, &catalog) {
            Ok(scene) => {
                envelopes.insert(e.def.id.clone(), (e.def.clone(), e.envelope));
                compiled.push((e.def, scene));
            }
            Err(_) => notes.push(format!("make.contribLeftOut: {}", short(e.def.id.as_str()))),
        }
    }
    clamp_project(&mut project, &envelopes);
    let names = rewrite(&mut project, inputs, &folder, &index);
    Ok(ProjectInputs {
        project: Arc::new(project),
        registry: Arc::new(Registry::with_checked(shaders).with_scenes(compiled)),
        stills: Arc::new(frozen),
        notes,
        names,
    })
}

/// Every setting the project gives a contribution in `envelopes`, clamped into its envelope.
fn clamp_project(p: &mut Project, envelopes: &HashMap<ContribId, (ContribDef, Envelope)>) {
    let fit = |id: &ContribId, params: &mut crate::make::envelope::Params| {
        if let Some((def, env)) = envelopes.get(id) {
            clamp(def, env, params);
        }
    };
    for track in &mut p.timeline.tracks {
        for item in &mut track.items {
            if let ItemContent::Generator { source, params, .. } = &mut item.content {
                fit(source, params);
            }
            for e in &mut item.effects {
                fit(&e.effect, &mut e.params);
            }
        }
    }
    for clip in &mut p.timeline.main {
        if let Some(t) = &mut clip.transition_out {
            fit(&t.effect, &mut t.params);
        }
    }
}

/// Points every asset and LUT at the app's copy in the stills folder, or at a name that never exists in `in/` (no
/// input starts with `.`). Videos are never opened (`FrozenStills` serves them by id). The drawn paths' names.
fn rewrite(
    p: &mut Project,
    inputs: &Path,
    folder: &Path,
    index: &crate::media::frozen::StillsIndex,
) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut n = 0;
    let nowhere = |n: &mut usize| {
        *n += 1;
        inputs.join(format!(".missing-{n}"))
    };
    for asset in &mut p.assets {
        let copy = index
            .images
            .iter()
            .find(|i| i.id == asset.id)
            .map(|i| folder.join(i.file.as_str()));
        let path = match (asset.kind, copy) {
            (AssetKind::Image, Some(copy)) => copy,
            _ => nowhere(&mut n),
        };
        asset.path = path.to_string_lossy().into_owned();
        names.insert(asset.path.clone(), asset.name.clone());
    }
    for lut in &mut p.luts {
        let path = index
            .luts
            .iter()
            .find(|l| l.id == lut.id)
            .map_or_else(|| nowhere(&mut n), |l| folder.join(l.file.as_str()));
        lut.path = path.to_string_lossy().into_owned();
        names.insert(lut.path.clone(), lut.name.clone());
    }
    names
}

pub(crate) fn read(path: &Path, cap: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(cap + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("an input can't be read: {e}"))?;
    if bytes.len() as u64 > cap {
        return Err("an input is too large".into());
    }
    Ok(bytes)
}

/// The note frames of a project say for a placed draft `id` this worker never checked: the app matches it, for its
/// own placed draft, to check that draft again, never any text that merely says "checking".
pub fn draft_not_checked(id: &str) -> String {
    format!("{FRAME_AGENT_EFFECT_CHECKING}: {}", short(id))
}

/// An id or name for a note: escaped, at most 64 characters.
pub(super) fn short(text: &str) -> String {
    shown(&text.chars().take(64).collect::<String>())
}
