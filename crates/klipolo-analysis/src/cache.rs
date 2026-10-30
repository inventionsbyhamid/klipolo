//! What a clip's analysis found, kept per file so the same clip (re-imported, renamed, moved, or its project reopened)
//! never costs a second Meta call.
//!
//! A file's key is its content: SHA-256 of its size (8 bytes, little-endian), its first 4 MiB and its last 4 MiB (a file
//! under 4 MiB is read whole, twice), as lowercase hex. Two reads at most, however long the clip; a change only in the
//! middle of a file over 8 MiB keeps its key (an edit that keeps the length and both ends is not expected of a video).
//!
//! Each key has a folder, `<root>/<key>/`, under a root the caller passes (the app: `Klipolo/analysis` in Application
//! Support): one `<name>.json` per result (`transcript.json`, `scenes.json`) beside `meta.json`, which holds the
//! folder's schema, when it was made and the model each result came from. Files are written atomically (a temporary
//! file renamed into place). A folder of another schema loads nothing (a newer app's results are never misread), and
//! storing into it starts it again.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use klipolo_platform::durable::{self, Durability};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The shape of what this version stores.
pub const SCHEMA: u32 = 1;

/// How much of each end of a file the key reads.
const END_BYTES: u64 = 4 * 1024 * 1024;

/// `path`'s content key (see the module notes).
pub fn content_key(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hash = Sha256::new();
    hash.update(size.to_le_bytes());
    let mut buf = vec![0u8; END_BYTES.min(size) as usize];
    file.read_exact(&mut buf)?;
    hash.update(&buf);
    file.seek(SeekFrom::Start(size - buf.len() as u64))?;
    file.read_exact(&mut buf)?;
    hash.update(&buf);
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// A key folder's `meta.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub schema: u32,
    /// When the folder was started, UTC (`2026-10-30T10:47:32Z`).
    #[serde(default)]
    pub created_at: String,
    /// The model each result came from, by result name (`transcript` → `muse-voice-transcribe-1.0`).
    #[serde(default)]
    pub models: BTreeMap<String, String>,
}

/// Just the schema, which every version's `meta.json` has.
#[derive(Deserialize)]
struct Schema {
    schema: u32,
}

/// One file's cache folder.
#[derive(Clone, Debug)]
pub struct Dir {
    path: PathBuf,
}

impl Dir {
    /// `<root>/<key>/` (nothing is read or made until a load or a store).
    pub fn new(root: &Path, key: &str) -> Self {
        Self {
            path: root.join(key),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `<name>.json`, if this folder is of this version's schema and the file reads as a `T`. Anything else (no such
    /// file, another schema, a damaged file) is None: analyse again.
    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        let file = self.file(name).ok()?;
        let schema: Schema = read_json(&self.meta_path())?;
        if schema.schema != SCHEMA {
            return None;
        }
        read_json(&file)
    }

    /// Replaces `<name>.json` with `value`, starting the folder (its `meta.json`) first if it has none or is of
    /// another schema (whose files are dropped).
    pub fn store<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        let file = self.file(name)?;
        self.ready()?;
        write_json(&file, value)
    }

    /// The folder's `meta.json`, if it has one that reads.
    pub fn meta(&self) -> Option<Meta> {
        read_json(&self.meta_path())
    }

    /// Notes in `meta.json` that `<name>.json` came from `model`.
    pub fn set_model(&self, name: &str, model: &str) -> io::Result<()> {
        self.file(name)?;
        let mut meta = self.ready()?;
        meta.models.insert(name.to_owned(), model.to_owned());
        write_json(&self.meta_path(), &meta)
    }

    /// The folder with a `meta.json` of this schema, made now if needed.
    fn ready(&self) -> io::Result<Meta> {
        std::fs::create_dir_all(&self.path)?;
        let meta_path = self.meta_path();
        if let Some(meta) = read_json::<Meta>(&meta_path).filter(|m| m.schema == SCHEMA) {
            return Ok(meta);
        }
        // Another version's folder (or a damaged one): its results are of another shape.
        if meta_path.exists() {
            for entry in std::fs::read_dir(&self.path)? {
                let path = entry?.path();
                if path.extension().is_some_and(|e| e == "json") {
                    std::fs::remove_file(path)?;
                }
            }
        }
        let meta = Meta {
            schema: SCHEMA,
            created_at: utc(SystemTime::now()),
            models: BTreeMap::new(),
        };
        write_json(&meta_path, &meta)?;
        Ok(meta)
    }

    /// `<name>.json` in the folder. Names are lowercase letters, digits, `-` and `_` (nothing that leaves the folder),
    /// and not `meta` (the folder's own file).
    fn file(&self, name: &str) -> io::Result<PathBuf> {
        let plain = !name.is_empty()
            && name != "meta"
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !plain {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Not a cache file name",
            ));
        }
        Ok(self.path.join(format!("{name}.json")))
    }

    fn meta_path(&self) -> PathBuf {
        self.path.join("meta.json")
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    durable::write_atomic(path, &bytes, Durability::Ordered)
}

/// `2026-10-30T10:47:32Z`.
fn utc(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Howard Hinnant's `civil_from_days`, for dates after 1970.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn utc_formats_known_instants() {
        let at = |s| utc(UNIX_EPOCH + Duration::from_secs(s));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_793_357_252), "2026-10-30T10:47:32Z");
    }
}
