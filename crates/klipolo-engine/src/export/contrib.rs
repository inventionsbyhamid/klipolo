//! An export job's contributions: the job freezes exactly what the
//! preview's snapshot draws, so an export never draws what the preview wouldn't. At job creation the app hands over the
//! contributions the project uses that its one `runnable` predicate let run (the "Agent-made effects" switch on, not
//! guarded, checked on this computer, not Over), each already loaded and verified, with its verdict; they are written
//! from memory (never a copy of the project's `contrib/`) into `<job>/contrib/<key hex>/`, and `verified.json` holds
//! exactly those verdicts. What the snapshot leaves out joins the job's catalogue only (`withheld.json`), so a frame is
//! noted "being checked" and drawn without it, never stopped as a missing plugin. The export worker builds its
//! registry from the folder (`job_registry`): runnable means in `verified.json`, its folder reading back as saved under
//! its own key. A verified scene is compiled there (`compile_verified`) against the job's catalogue, so the
//! generators its shapes are filled with resolve as the preview's did.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use klipolo_model::contrib::{ContribDef, ContribId};
use serde::{Deserialize, Serialize};

use super::shared::write_atomic;
use crate::make::draft::shader_kind;
use crate::registry::key::ContribSource;
use crate::registry::pack::{LoadedContrib, PackLimits, Verdict, hex, load_folder_as, read_capped};
use crate::registry::pack_scene::{VerifiedScene, compile_verified};
use crate::registry::{Registry, ShaderEffect};

/// The job's folder of contributions.
pub const CONTRIB_DIR: &str = "contrib";
/// The verdicts of the contributions the job draws, by key.
pub const VERIFIED_JSON: &str = "verified.json";
/// The definitions of the contributions the job leaves out.
pub const WITHHELD_JSON: &str = "withheld.json";
/// What either list may be read up to (64 contributions with their envelopes and advice fit well within it).
const MAX_LIST_BYTES: u64 = 4 << 20;

/// What a job draws besides the built-ins: each runnable contribution as loaded, with the verdict it runs with; and the
/// definitions of those it leaves out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobContribs {
    pub runnable: Vec<(LoadedContrib, Verdict)>,
    pub withheld: Vec<ContribDef>,
}

/// One entry of `verified.json`: the contribution's id (its folder is named by its key) and its verdict.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobEntry {
    pub id: ContribId,
    pub verdict: Verdict,
}

/// Writes `contribs` into `<job>/contrib/` (see the module's doc), each file atomically; nothing when there are none.
pub fn write_job_contribs(job: &Path, contribs: &JobContribs) -> std::io::Result<()> {
    if contribs.runnable.is_empty() && contribs.withheld.is_empty() {
        return Ok(());
    }
    let root = job.join(CONTRIB_DIR);
    std::fs::create_dir_all(&root)?;
    let mut verified = BTreeMap::new();
    for (c, verdict) in &contribs.runnable {
        let key = hex(&c.key);
        let dir = root.join(&key);
        std::fs::create_dir_all(&dir)?;
        for (name, bytes) in &c.bytes {
            write_atomic(&dir.join(name), bytes)?;
        }
        verified.insert(
            key,
            JobEntry {
                id: c.def.id.clone(),
                verdict: verdict.clone(),
            },
        );
    }
    write_atomic(&root.join(VERIFIED_JSON), &json(&verified)?)?;
    write_atomic(&root.join(WITHHELD_JSON), &json(&contribs.withheld)?)?;
    Ok(())
}

/// The registry the job at `job` draws with (see the module's doc). A folder that doesn't read back, or whose key isn't
/// its name, draws as nothing (logged): a damaged job never draws what wasn't checked.
pub fn job_registry(job: &Path) -> Arc<Registry> {
    let root = job.join(CONTRIB_DIR);
    let withheld: Vec<ContribDef> = read_list(&root.join(WITHHELD_JSON)).unwrap_or_default();
    let verified: BTreeMap<String, JobEntry> =
        read_list(&root.join(VERIFIED_JSON)).unwrap_or_default();
    let mut loaded = Vec::new();
    let mut verdicts = BTreeMap::new();
    for (key, entry) in verified.into_iter().take(PackLimits::MAX_CONTRIBS) {
        match load_folder_as(&root.join(&key), &entry.id, &PackLimits::DEFAULT) {
            Ok(c) if hex(&c.key) == key => {
                verdicts.insert(c.key, entry.verdict);
                loaded.push(c);
            }
            Ok(_) => {
                eprintln!("export: a contribution's folder isn't the one checked; it's left out")
            }
            Err(e) => eprintln!("export: a contribution didn't read ({e}); it's left out"),
        }
    }
    let defs = withheld.iter().chain(loaded.iter().map(|c| &c.def));
    let base = Registry::with_definitions(&Registry::builtin(), defs);
    let shader_for = |c: &LoadedContrib| -> Option<Arc<ShaderEffect>> {
        let verdict = verdicts.get(&c.key)?;
        let ContribSource::Wgsl(body) = &c.source else {
            return None;
        };
        let kind = shader_kind(c.def.kind)?;
        match ShaderEffect::assemble_untrusted(&c.def, kind, body) {
            Ok(shader) => Some(Arc::new(shader.verified(verdict))),
            Err(_) => {
                eprintln!("export: a checked contribution didn't assemble; it's left out");
                None
            }
        }
    };
    let scene_for = |c: &LoadedContrib| -> Option<VerifiedScene> {
        let verdict = verdicts.get(&c.key)?;
        match compile_verified(c, base.catalog()) {
            Ok(scene) => Some(VerifiedScene::new(scene, verdict)),
            Err(_) => {
                eprintln!("export: a checked scene didn't compile; it's left out");
                None
            }
        }
    };
    Arc::new(Registry::with_contributions(
        &base,
        &loaded,
        &shader_for,
        &scene_for,
    ))
}

/// A JSON list the job wrote, None when it isn't there or doesn't read.
fn read_list<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = read_capped(path, MAX_LIST_BYTES).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn json<T: Serialize>(value: &T) -> std::io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(std::io::Error::other)
}
