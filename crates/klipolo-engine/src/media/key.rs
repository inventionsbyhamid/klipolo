//! Cache names for one source file's derived data (sound, thumbnails, editing copies, screen activity). A name hashes
//! the kind and its format version, the path, and the file's size and modification time: an edited or replaced file
//! gets new caches, never an old one. An editing copy's name also binds the file's number on its
//! volume (`SourceId`), so a file replaced by another of the same size and time still gets a new copy.

use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// What a file is now: its size, modification time (ns) and number on its volume (Unix inode; 0 elsewhere). An
/// overwrite in place changes the size or time; a replace (a save to a new file renamed over it) changes the number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceId {
    pub len: u64,
    pub modified_ns: u64,
    pub file_id: u64,
}

impl SourceId {
    pub fn of(meta: &std::fs::Metadata) -> Self {
        let modified_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        #[cfg(unix)]
        let file_id = std::os::unix::fs::MetadataExt::ino(meta);
        #[cfg(not(unix))]
        let file_id = 0;
        Self {
            len: meta.len(),
            modified_ns,
            file_id,
        }
    }
}

/// `path` as it is now (a stat), or None when it can't be looked at.
pub fn source_id(path: &str) -> Option<SourceId> {
    std::fs::metadata(path).ok().map(|m| SourceId::of(&m))
}

pub enum CacheKind {
    Pcm,
    Thumbs,
    Proxy {
        long_side: u32,
    },
    /// The screen activity track.
    Activity,
}

impl CacheKind {
    fn tag(&self) -> String {
        match self {
            Self::Pcm => "s16-half-native-1".into(),
            Self::Thumbs => "thumbs-1".into(),
            Self::Proxy { long_side } => format!("proxy-2-{long_side}"),
            Self::Activity => "activity-1".into(),
        }
    }
}

/// 16 hex digits for `path`'s `kind` cache, or None when the file can't be read.
pub fn stem(path: &str, kind: &CacheKind) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    Some(stem_of(path, &meta, kind))
}

/// `stem` from metadata already read.
pub fn stem_of(path: &str, meta: &std::fs::Metadata, kind: &CacheKind) -> String {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let tag = kind.tag();
    let mut parts: Vec<&[u8]> = Vec::with_capacity(5);
    let (len, mtime) = (meta.len().to_le_bytes(), mtime.to_le_bytes());
    parts.extend([tag.as_bytes(), path.as_bytes(), &len[..], &mtime[..]]);
    let id = SourceId::of(meta).file_id.to_le_bytes();
    if matches!(kind, CacheKind::Proxy { .. }) {
        parts.push(&id[..]);
    }
    hash_name(&parts)
}

/// 16 hex digits naming a cache made of `parts`: FNV-1a over them, each followed by 0xff (stable across runs, unlike
/// `DefaultHasher`).
pub fn hash_name(parts: &[&[u8]]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in part.iter().chain([0xff].iter()) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}
