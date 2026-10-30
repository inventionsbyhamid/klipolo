//! Reading a contribution's version folder: the person's library
//! (`load_version_dir`), a project's copies (`embedded`: through folder handles), the render worker's copy of what the
//! app sent and an export job's folders (`load_folder_as`). Every reader ends in [`load_files`]: only `contrib.json`,
//! `effect.wgsl` or `scene.json`, `thumb.jpg` and `hashes.json`, each within its size, never through a symbolic link at
//! any level, and only when every file's SHA-256 matches `hashes.json`. The folder's names and the definition's id are
//! well formed and agree; the names in the definition are already clean (they are cleaned when saved, never here) and
//! its settings' text holds no hidden characters (a setting's key not even one the worker's taming would change); a
//! protective definition or a built-in id is refused, and so is a definition past the caps (`def`). A scene's bytes
//! are read here and parsed only once this computer verified them. The registry built from what was
//! loaded is `with_contributions`, its shaders from `ShaderCache`, each with its verdict on it (`verdict`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use klipolo_model::contrib::{ContribDef, ContribId, ContribKind};
use sha2::{Digest, Sha256};

use super::key::{ContribSource, contrib_key};

mod cache;
mod def;
mod embedded;
mod read;
mod registry;
mod verdict;
pub use cache::{MAX_CACHED_SHADERS, ShaderCache};
pub use def::{check_definition, names_clean, setting_text_clean};
pub use embedded::{Embedded, MAX_EMBEDDED_BYTES, Refused, read_embedded_dir};
pub use read::read_capped;
pub use verdict::Verdict;

pub const CONTRIB_JSON: &str = "contrib.json";
pub const EFFECT_WGSL: &str = "effect.wgsl";
pub const SCENE_JSON: &str = "scene.json";
pub const THUMB_JPG: &str = "thumb.jpg";
pub const HASHES_JSON: &str = "hashes.json";

/// The largest each file may be, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackLimits {
    pub contrib_json: u64,
    pub wgsl: u64,
    pub scene: u64,
    pub thumb: u64,
    pub hashes: u64,
}

impl PackLimits {
    /// The one cap on how many contributions any reader takes from a folder of them.
    pub const MAX_CONTRIBS: usize = 64;
    /// The size caps: 64 KiB of definition, 32 KiB of WGSL, 256 KiB of scene, a 256 KiB thumbnail.
    pub const DEFAULT: PackLimits = PackLimits {
        contrib_json: 64 << 10,
        wgsl: 32 << 10,
        scene: 256 << 10,
        thumb: 256 << 10,
        hashes: 4 << 10,
    };

    /// The cap of `name`, a file a version folder may hold; None for any other name.
    pub fn of(&self, name: &str) -> Option<u64> {
        Some(match name {
            CONTRIB_JSON => self.contrib_json,
            EFFECT_WGSL => self.wgsl,
            SCENE_JSON => self.scene,
            THUMB_JPG => self.thumb,
            HASHES_JSON => self.hashes,
            _ => return None,
        })
    }
}

/// A version folder as loaded: its definition and source, its key over both (`contrib_key`), where it was, and
/// its files exactly as read, so it is copied from memory and never read twice.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedContrib {
    pub def: ContribDef,
    pub source: ContribSource,
    pub key: [u8; 32],
    pub folder: PathBuf,
    pub bytes: Vec<(String, Vec<u8>)>,
}

/// Why a folder wasn't loaded. Names from the folder are quoted, escaped and cut at 64 characters.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PackError {
    #[error("{0} isn't a well-formed contribution id")]
    BadId(String),
    #[error("the folder doesn't match the id {0}")]
    FolderMismatch(String),
    #[error("{0} is a built-in id")]
    Builtin(String),
    #[error("{0} is a symbolic link or not a plain file or folder")]
    Link(String),
    #[error("{0} isn't a file a version folder holds")]
    Extra(String),
    #[error("{0} is missing")]
    Missing(String),
    #[error("{file} is larger than {limit} bytes")]
    TooLarge { file: String, limit: u64 },
    #[error("{0} doesn't match its hash in hashes.json")]
    Hash(String),
    #[error("the definition's id {found} isn't the folder's {expected}")]
    IdMismatch { expected: String, found: String },
    #[error("only built-in effects can be protective")]
    Protective,
    #[error("the definition's {0} isn't cleaned")]
    NotClean(&'static str),
    #[error("the setting {0} holds a control, invisible, direction or line-separator character")]
    Hidden(String),
    #[error("{0}")]
    Definition(String),
    #[error("{0}")]
    Io(String),
}

/// `text` quoted, its invisible and control characters escaped, cut at 64 characters.
pub(crate) fn quoted(text: &str) -> String {
    let cut: String = text.chars().take(64).collect();
    format!("{:?}", super::guard::shown(&cut))
}

/// The version folder `dir` (`<…>/<plugin>/<local>`) of contribution `id`, read and checked (see the module's doc).
pub fn load_version_dir(
    dir: &Path,
    id: &ContribId,
    limits: &PackLimits,
) -> Result<LoadedContrib, PackError> {
    check_id(id)?;
    let (plugin, local) = id.as_str().split_once('/').expect("well formed");
    let named = |p: Option<&Path>, name: &str| {
        p.and_then(Path::file_name)
            .is_some_and(|n| n.to_str() == Some(name))
    };
    if !named(Some(dir), local) || !named(dir.parent(), plugin) {
        return Err(PackError::FolderMismatch(quoted(id.as_str())));
    }
    read::plain_folder(dir.parent().expect("named"), plugin)?;
    read::plain_folder(dir, local)?;
    let files = read::version_files(dir, limits)?;
    load_files(id, files, dir.to_path_buf())
}

/// The folder `dir` read as version `id` whatever it is named: the render worker's copy of what the app sent
/// in its `in/`, an export job's `<key>/` folder. Every check `load_version_dir` makes but the folder's names.
pub fn load_folder_as(
    dir: &Path,
    id: &ContribId,
    limits: &PackLimits,
) -> Result<LoadedContrib, PackError> {
    check_id(id)?;
    read::plain_folder(dir, "the version folder")?;
    let files = read::version_files(dir, limits)?;
    load_files(id, files, dir.to_path_buf())
}

/// A well-formed id that isn't built in.
fn check_id(id: &ContribId) -> Result<(), PackError> {
    if !id.is_well_formed() {
        return Err(PackError::BadId(quoted(id.as_str())));
    }
    if id.is_builtin() {
        return Err(PackError::Builtin(quoted(id.as_str())));
    }
    Ok(())
}

/// Version `id` from its files exactly as read (each already within its cap), found in `folder`: every file but
/// `hashes.json` matching its hash there, then the definition parsed and checked (see the module's doc) and the source
/// the kind takes. The one check every reader ends in: the library's, a project's embedded folder's (read through
/// folder handles, `embedded`), the worker's and an export job's.
pub fn load_files(
    id: &ContribId,
    files: BTreeMap<String, Vec<u8>>,
    folder: PathBuf,
) -> Result<LoadedContrib, PackError> {
    check_id(id)?;
    verify_hashes(&files)?;
    let file = |name: &str| files.get(name);
    let def: ContribDef = serde_json::from_slice(
        file(CONTRIB_JSON).ok_or_else(|| PackError::Missing(CONTRIB_JSON.into()))?,
    )
    .map_err(|e| PackError::Definition(format!("contrib.json doesn't read: {}", short(&e))))?;
    if def.id != *id {
        return Err(PackError::IdMismatch {
            expected: quoted(id.as_str()),
            found: quoted(def.id.as_str()),
        });
    }
    if def.protective {
        return Err(PackError::Protective);
    }
    names_clean(&def).map_err(PackError::NotClean)?;
    setting_text_clean(&def).map_err(PackError::Hidden)?;
    check_definition(&def).map_err(PackError::Definition)?;
    let source = source_of(&def, file(EFFECT_WGSL), file(SCENE_JSON))?;
    Ok(LoadedContrib {
        key: contrib_key(&def, &source),
        def,
        source,
        folder,
        bytes: files.into_iter().collect(),
    })
}

/// Every file but `hashes.json` is listed there with its SHA-256 (lowercase hex), and everything listed is there.
fn verify_hashes(files: &BTreeMap<String, Vec<u8>>) -> Result<(), PackError> {
    let listed: BTreeMap<String, String> = serde_json::from_slice(
        files
            .get(HASHES_JSON)
            .ok_or_else(|| PackError::Missing(HASHES_JSON.into()))?,
    )
    .map_err(|e| PackError::Definition(format!("hashes.json doesn't read: {}", short(&e))))?;
    for (name, bytes) in files.iter().filter(|(n, _)| *n != HASHES_JSON) {
        if listed.get(name) != Some(&hex(&Sha256::digest(bytes))) {
            return Err(PackError::Hash(quoted(name)));
        }
    }
    match listed.keys().find(|n| !files.contains_key(*n)) {
        Some(name) => Err(PackError::Missing(quoted(name))),
        None => Ok(()),
    }
}

/// A shader's WGSL (UTF-8) or a scene's bytes, whichever the kind takes, and never both.
fn source_of(
    def: &ContribDef,
    wgsl: Option<&Vec<u8>>,
    scene: Option<&Vec<u8>>,
) -> Result<ContribSource, PackError> {
    match def.kind {
        ContribKind::Scene => {
            if wgsl.is_some() {
                return Err(PackError::Extra(quoted(EFFECT_WGSL)));
            }
            let bytes = scene.ok_or_else(|| PackError::Missing(SCENE_JSON.into()))?;
            Ok(ContribSource::Scene(bytes.clone()))
        }
        ContribKind::Generator | ContribKind::Effect | ContribKind::Transition => {
            if scene.is_some() {
                return Err(PackError::Extra(quoted(SCENE_JSON)));
            }
            let text = wgsl.ok_or_else(|| PackError::Missing(EFFECT_WGSL.into()))?;
            String::from_utf8(text.clone())
                .map(ContribSource::Wgsl)
                .map_err(|_| PackError::Definition("effect.wgsl isn't UTF-8 text".into()))
        }
        _ => Err(PackError::Definition(
            "a version folder holds a generator, a filter, a transition or a scene".into(),
        )),
    }
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A parse error's text, escaped and cut (it can quote what the folder holds).
fn short(e: &serde_json::Error) -> String {
    let text = super::guard::shown(&e.to_string());
    text.chars().take(160).collect()
}
