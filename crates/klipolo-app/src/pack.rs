//! Klipolo's own pack of sounds, music beds and stickers (`assets/library`, listed by `scripts/gen-library.py`):
//! what its manifest lists, as the library shows it and as assets a project adds. The files ship in the app's resources
//! (`library/`); a project names them by their path there, as any file, and by their id (`klipolo-pack-<item>`). That
//! path is where the pack was when the file was added: the app moved, macOS ran it from a translocated copy, or another
//! build saved the project, and it is somewhere else now. So each opening points the project's pack files at this
//! install's pack again (`Pack::repoint`), by id; the decoders, the peaks, the page's media and export all read the
//! path as it is then. A newer pack can also hold a different recording under the same id, so the length and size come
//! from this pack too, and the timeline's pieces of a sound fit its length now (`fit_to_file`).

use std::path::{Path, PathBuf};

use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::time::{MIN_DURATION_US, Us, drifted};
use klipolo_model::timeline::{ClipSource, ItemContent};
use klipolo_model::volume;
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
    /// Words for what it is or suits ("transition", "swipe"), as the manifest lists them: the library's search and
    /// chat find it by them too.
    pub tags: Vec<String>,
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
    #[serde(default)]
    tags: Vec<String>,
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
                        tags: item.tags,
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

    /// Points `project`'s pack files (assets whose id is `klipolo-pack-<item>`) at this pack's copy of each item, with its
    /// length and size as this pack lists them; an item this pack doesn't have keeps its path (it reports as missing if it
    /// is gone). A sound whose length changed (a pack update holds another recording under its id) has its pieces on the
    /// timeline fitted to it (`fit_to_file`). How many assets it changed.
    pub(crate) fn repoint(&self, project: &mut Project) -> usize {
        let mut changed = 0;
        let mut lengths = Vec::new();
        for asset in &mut project.assets {
            let Some((item, file)) = asset.id.strip_prefix(ASSET_PREFIX).and_then(|id| {
                self.items
                    .iter()
                    .find(|(item, _)| item.id == id)
                    .map(|(item, file)| (item, file))
            }) else {
                continue;
            };
            let path = file.to_string_lossy();
            let listed = (item.duration_us, item.width, item.height);
            if asset.path == path && (asset.duration_us, asset.width, asset.height) == listed {
                continue;
            }
            if asset.kind == AssetKind::Audio
                && item.duration_us > 0
                && asset.duration_us != item.duration_us
            {
                lengths.push((asset.id.clone(), asset.duration_us, item.duration_us));
            }
            asset.path = path.into_owned();
            (asset.duration_us, asset.width, asset.height) = listed;
            changed += 1;
        }
        for (id, was, now) in lengths {
            fit_to_file(project, &id, was, now);
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

/// Fits the timeline's pieces of sound asset `id` to its file, now `now` long where it was `was`. A piece that would play
/// past the file's end ends at it (from the file's start, if it started past it), its fades and volume keys fitted too,
/// so the project stays valid. A layer that played the whole of the file to its end plays the whole of the longer one,
/// as far as the next layer on its track lets it, unless its track is locked. Main-track clips only shorten: lengthening
/// one would move everything after it.
fn fit_to_file(project: &mut Project, id: &str, was: Us, now: Us) {
    for track in &mut project.timeline.tracks {
        let starts: Vec<Us> = track.items.iter().map(|item| item.start_us).collect();
        let locked = track.locked;
        for (k, item) in track.items.iter_mut().enumerate() {
            let ItemContent::Audio {
                asset_id,
                source_in_us,
                fade_in_us,
                fade_out_us,
                drift_ppm,
                volume_keys,
                ..
            } = &mut item.content
            else {
                continue;
            };
            if asset_id != id {
                continue;
            }
            // The timeline length that plays `footage` of the file.
            let timeline = |footage: Us| {
                let mut length = (footage as f64 / (1.0 + *drift_ppm * 1e-6)).floor() as Us;
                while length > 0 && drifted(length, *drift_ppm) > footage {
                    length -= 1;
                }
                length
            };
            let end = source_in_us.saturating_add(drifted(item.duration_us, *drift_ppm));
            let length = if end > now {
                if *source_in_us > now - MIN_DURATION_US {
                    *source_in_us = 0;
                }
                item.duration_us.min(timeline(now - *source_in_us))
            } else if !locked && *source_in_us == 0 && end + 1_000 >= was && now > was {
                let room = starts
                    .get(k + 1)
                    .map_or(Us::MAX, |next| next - item.start_us);
                timeline(now).min(room).max(item.duration_us)
            } else {
                continue;
            };
            if length < item.duration_us {
                volume::fit(volume_keys, length);
            }
            item.duration_us = length;
            fit_fades(fade_in_us, fade_out_us, length);
        }
    }
    for clip in &mut project.timeline.main {
        if !matches!(&clip.source, ClipSource::Asset { id: asset } if asset == id)
            || clip.source_out_us <= now
        {
            continue;
        }
        if clip.source_in_us > now - MIN_DURATION_US {
            clip.source_in_us = 0;
        }
        clip.source_out_us = now;
        let length = clip.duration_us();
        volume::fit(&mut clip.volume_keys, length);
        fit_fades(&mut clip.fade_in_us, &mut clip.fade_out_us, length);
    }
}

/// Fades that together take no more than `length`: the fade out keeps what it can, the fade in the rest.
fn fit_fades(fade_in: &mut Us, fade_out: &mut Us, length: Us) {
    *fade_out = (*fade_out).min(length);
    *fade_in = (*fade_in).min(length - *fade_out);
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

    /// The repository's pack lists its 35 sounds, four music beds and 51 pictures (stickers and backgrounds),
    /// each with its file.
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
            (35, 4, 51)
        );
        let whoosh = pack.asset("whoosh").unwrap();
        assert_eq!(
            (whoosh.id.as_str(), whoosh.kind, whoosh.name.as_str()),
            ("klipolo-pack-whoosh", AssetKind::Audio, "Whoosh")
        );
        assert_eq!(whoosh.duration_us, 426_000);
        assert!(Path::new(&whoosh.path).is_file());
        let star = pack.asset("star").unwrap();
        assert_eq!(
            (star.kind, star.width, star.height),
            (AssetKind::Image, 1024, 1024)
        );
        assert!(pack.asset("nope").is_none());
    }

    /// Each item keeps its manifest's tags, for the library's search and chat; an item with none has none.
    #[test]
    fn the_pack_keeps_each_items_tags() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pop.wav"), b"RIFF").unwrap();
        std::fs::write(dir.path().join("tap.wav"), b"RIFF").unwrap();
        std::fs::write(
            dir.path().join("manifest.json"),
            r#"{"items": [
                {"id": "pop", "name": "Pop", "category": "sound", "file": "pop.wav", "duration_ms": 160,
                 "tags": ["boom", "impact"]},
                {"id": "tap", "name": "Tap", "category": "sound", "file": "tap.wav", "duration_ms": 90}
            ]}"#,
        )
        .unwrap();
        let tags: Vec<Vec<String>> = Pack::read(dir.path())
            .items()
            .into_iter()
            .map(|item| item.tags)
            .collect();
        assert_eq!(tags, [vec!["boom", "impact"], vec![]]);
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
