//! Klipolo's own pack of sounds, music beds and stickers (`assets/library`, CC0, made by `scripts/gen-library.py`):
//! what its manifest lists, as the library shows it and as assets a project adds. The files ship in the app's resources
//! (`library/`); a project names them by their path there, as any file, and by their id (`klipolo-pack-<item>`). That
//! path is where the pack was when the file was added: the app moved, macOS ran it from a translocated copy, or another
//! build saved the project, and it is somewhere else now. So each opening points the project's pack files at this
//! install's pack again (`Pack::repoint`), by id; the decoders, the peaks, the page's media and export all read the
//! path as it is then.

use std::path::{Path, PathBuf};

use klipolo_model::project::{Asset, AssetKind, Project};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The ids of the pack's assets in a project start with this (`klipolo-pack-whoosh`): the same item added twice is
/// one file of the project.
pub(crate) const ASSET_PREFIX: &str = "klipolo-pack-";

/// What a pack item is for: the library lists each under its tab (Sounds, Music, Images).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PackCategory {
    Sound,
    Music,
    Sticker,
}

/// One item of the pack, as the library lists it.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PackItem {
    pub id: String,
    pub name: String,
    pub category: PackCategory,
    /// Sounds and music; 0 for stickers.
    pub duration_us: i64,
    /// Stickers; 0 for sounds and music.
    pub width: u32,
    pub height: u32,
}

#[derive(Deserialize)]
struct Manifest {
    items: Vec<ManifestItem>,
}

#[derive(Deserialize)]
struct ManifestItem {
    id: String,
    name: String,
    category: PackCategory,
    file: String,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

/// The pack as read from its folder: each item, and its file there.
#[derive(Clone, Debug, Default)]
pub(crate) struct Pack {
    items: Vec<(PackItem, PathBuf)>,
}

impl Pack {
    /// The pack in `dir` (its `manifest.json`), or an empty one when it can't be read: the library shows the person's
    /// own files either way. Items whose file isn't there, or whose path would leave the folder, are left out.
    pub(crate) fn read(dir: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else {
            return Self::default();
        };
        let manifest: Manifest = match serde_json::from_str(&text) {
            Ok(manifest) => manifest,
            Err(error) => {
                tracing::warn!(%error, "the pack's manifest couldn't be read");
                return Self::default();
            }
        };
        let items = manifest
            .items
            .into_iter()
            .filter(|item| {
                Path::new(&item.file)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            })
            .filter_map(|item| {
                let file = dir.join(&item.file);
                file.is_file().then(|| {
                    let listed = PackItem {
                        id: item.id,
                        name: item.name,
                        category: item.category,
                        duration_us: item.duration_ms.unwrap_or(0).saturating_mul(1000),
                        width: item.width.unwrap_or(0),
                        height: item.height.unwrap_or(0),
                    };
                    (listed, file)
                })
            })
            .collect();
        Self { items }
    }

    /// Every item, in the manifest's order.
    pub(crate) fn items(&self) -> Vec<PackItem> {
        self.items.iter().map(|(item, _)| item.clone()).collect()
    }

    /// Item `id`'s file.
    pub(crate) fn file(&self, id: &str) -> Option<&Path> {
        self.items
            .iter()
            .find(|(item, _)| item.id == id)
            .map(|(_, file)| file.as_path())
    }

    /// Points `project`'s pack files (assets whose id is `klipolo-pack-<item>`) at this pack's copy of each item; an item
    /// this pack doesn't have keeps its path (it reports as missing if it is gone). How many it changed.
    pub(crate) fn repoint(&self, project: &mut Project) -> usize {
        let mut changed = 0;
        for asset in &mut project.assets {
            let Some(file) = asset
                .id
                .strip_prefix(ASSET_PREFIX)
                .and_then(|item| self.file(item))
            else {
                continue;
            };
            let path = file.to_string_lossy();
            if asset.path != path {
                asset.path = path.into_owned();
                changed += 1;
            }
        }
        changed
    }

    /// Item `id` as a project's asset: always the same id for the same item, named as the library names it.
    pub(crate) fn asset(&self, id: &str) -> Option<Asset> {
        let (item, file) = self.items.iter().find(|(item, _)| item.id == id)?;
        let kind = match item.category {
            PackCategory::Sound | PackCategory::Music => AssetKind::Audio,
            PackCategory::Sticker => AssetKind::Image,
        };
        Some(Asset {
            id: format!("{ASSET_PREFIX}{}", item.id),
            kind,
            path: file.to_string_lossy().into_owned(),
            name: item.name.clone(),
            duration_us: item.duration_us,
            width: item.width,
            height: item.height,
            fps: 0.0,
            rotation: None,
            has_audio: kind == AssetKind::Audio,
            audio_lead_us: None,
            made_with: None,
        })
    }
}

/// Where the pack is: `KLIPOLO_LIBRARY_DIR`; else `library` in the app's resources; else, in debug builds only, the
/// repository's `assets/library`.
pub(crate) fn pack_dir(resources: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("KLIPOLO_LIBRARY_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = resources.map(|r| r.join("library"))
        && dir.join("manifest.json").is_file()
    {
        return Some(dir);
    }
    #[cfg(debug_assertions)]
    {
        let repo = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/library"));
        if repo.join("manifest.json").is_file() {
            return Some(repo);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_pack() -> Pack {
        Pack::read(&PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/library"
        )))
    }

    /// The repository's pack lists its 25 sounds, two music beds and six stickers, each with its file.
    #[test]
    fn the_bundled_pack_lists_its_sounds_music_and_stickers() {
        let pack = repo_pack();
        let count = |category| {
            pack.items()
                .iter()
                .filter(|item| item.category == category)
                .count()
        };
        assert_eq!(
            (
                count(PackCategory::Sound),
                count(PackCategory::Music),
                count(PackCategory::Sticker)
            ),
            (25, 2, 6)
        );
        let whoosh = pack.asset("whoosh").unwrap();
        assert_eq!(
            (whoosh.id.as_str(), whoosh.kind, whoosh.name.as_str()),
            ("klipolo-pack-whoosh", AssetKind::Audio, "Whoosh")
        );
        assert_eq!(whoosh.duration_us, 900_000);
        assert!(Path::new(&whoosh.path).is_file());
        let star = pack.asset("star").unwrap();
        assert_eq!(
            (star.kind, star.width, star.height),
            (AssetKind::Image, 1024, 1024)
        );
        assert!(pack.asset("nope").is_none());
    }

    /// A manifest naming a file outside its folder, or one that isn't there, lists nothing for it.
    #[test]
    fn a_pack_item_outside_its_folder_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pop.wav"), b"RIFF").unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{"items": [
                {"id": "pop", "name": "Pop", "category": "sound", "file": "pop.wav", "duration_ms": 160},
                {"id": "out", "name": "Out", "category": "sound", "file": "../secret.wav", "duration_ms": 160},
                {"id": "gone", "name": "Gone", "category": "sound", "file": "gone.wav", "duration_ms": 160}
            ]}"#,
        )
        .unwrap();
        let ids: Vec<String> = Pack::read(dir.path())
            .items()
            .into_iter()
            .map(|item| item.id)
            .collect();
        assert_eq!(ids, ["pop"]);
    }
}
