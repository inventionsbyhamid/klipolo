//! What fonts can draw here, as a value (no global state; the app and the engine each hold one, built from
//! the same sources; tests build their own): the bundled set, the families installed on this computer (the engine's
//! one scan), downloaded families as their manifests describe them, and Google's catalogue (the
//! app's index holds the shipped snapshot; the engine's needs none, since it draws only what is here).
//!
//! Downloaded families reach every index the same way: the app builds its index from
//! the download store's manifests, and the engine from the families registered with it (`render::fonts::register`), so
//! the engine resolves a download, and reads the CJK it covers, exactly as the app's preflight does.
//!
//! Every face the index can resolve to has an id (`face_id`: its file and its index in a collection), which the webview
//! names a face by when it asks for its bytes. `face_by_id` is one map lookup. The map is built
//! on its first use, from the fields as they are then; the fields are private and every builder starts a new map, so
//! the two never get out of step, and the engine, which never asks, never builds it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use klipolo_model::font::{FontRef, FontSource};

use crate::bundled::{BUNDLED, Category};
use crate::cjk::CjkCover;
use crate::coverage::{Coverage, HAN, HANGUL, KANA};
use crate::downloaded::version_number;
pub use crate::downloaded::{Downloaded, DownloadedFace};
use crate::google::Catalogue;
use crate::installed::NORMAL_WIDTH;
pub use crate::installed::{InstalledFace, InstalledFamily, hidden};
use crate::resolve::{Face, FaceFile, bundled_row_face};

/// Change it only through its builders (`shipped`, `with_*`), which keep `face_by_id` in step.
#[derive(Clone, Debug, Default)]
pub struct FontIndex {
    bundled_dir: Option<PathBuf>,
    /// This computer's families, sorted by name (case-insensitively), hidden ones left out.
    installed: Arc<[InstalledFamily]>,
    /// Google families downloaded here, each at its exact version.
    downloaded: Vec<Downloaded>,
    /// Google's catalogue, shared, never copied: what can be downloaded, offline.
    catalogue: Option<Arc<Catalogue>>,
    /// Every face of `faces()`, by `face_id`: built on first use.
    by_id: OnceLock<HashMap<String, Face>>,
}

/// The id a face is named by over IPC: `<path>#<index in its collection>`. Opaque to the webview, which only hands it
/// back; `FontIndex::face_by_id` answers only ids of faces the index holds.
pub fn face_id(file: &FaceFile) -> String {
    format!("{}#{}", file.path.to_string_lossy(), file.index)
}

/// The CJK an installed family's coverage bits say it draws itself.
fn cover_of(coverage: Coverage) -> CjkCover {
    CjkCover {
        han: coverage & HAN != 0,
        kana: coverage & KANA != 0,
        hangul: coverage & HANGUL != 0,
    }
}

impl FontIndex {
    /// The fonts that ship with Klipolo, from `bundled_dir` (None: a broken install, drawn in the system sans-serif).
    pub fn shipped(bundled_dir: Option<PathBuf>) -> Self {
        Self {
            bundled_dir,
            ..Self::default()
        }
    }

    /// This index with `families` downloaded (replacing any it had).
    pub fn with_downloaded(mut self, families: Vec<Downloaded>) -> Self {
        self.downloaded = families;
        self.by_id = OnceLock::new();
        self
    }

    /// This index with this computer's `families` (replacing any it had): hidden families dropped, sorted by name.
    pub fn with_installed(mut self, mut families: Vec<InstalledFamily>) -> Self {
        families.retain(|f| !hidden(&f.family));
        families.sort_by_cached_key(|f| f.family.to_lowercase());
        self.installed = families.into();
        self.by_id = OnceLock::new();
        self
    }

    /// This index with the installed families `installed` (an index's own list, shared, never copied).
    pub fn with_installed_list(mut self, installed: Arc<[InstalledFamily]>) -> Self {
        self.installed = installed;
        self.by_id = OnceLock::new();
        self
    }

    /// This index with Google's catalogue (replacing any it had): `google::shipped()` in the app, a fixture in tests.
    /// The catalogue has no faces here, so `face_by_id`'s map is unchanged.
    pub fn with_catalogue(mut self, catalogue: Arc<Catalogue>) -> Self {
        self.catalogue = Some(catalogue);
        self
    }

    /// Google's catalogue, if this index has one.
    pub fn catalogue(&self) -> Option<&Arc<Catalogue>> {
        self.catalogue.as_ref()
    }

    /// The bundled folder, None for a broken install.
    pub fn bundled_dir(&self) -> Option<&Path> {
        self.bundled_dir.as_deref()
    }

    /// This computer's families, sorted by name.
    pub fn installed(&self) -> &[InstalledFamily] {
        &self.installed
    }

    /// The same list, shared (`with_installed_list`).
    pub fn installed_list(&self) -> Arc<[InstalledFamily]> {
        self.installed.clone()
    }

    /// The installed family named exactly `family`.
    pub fn installed_family(&self, family: &str) -> Option<&InstalledFamily> {
        self.installed.iter().find(|f| f.family == family)
    }

    /// The downloaded families.
    pub fn downloads(&self) -> &[Downloaded] {
        &self.downloaded
    }

    /// Every face the index can resolve to, fallbacks' files included: the bundled faces (Adobe NotDef too, the file
    /// behind `kl:fallback:notdef`), the installed ones (registered under their own family names), then the
    /// downloaded ones.
    pub fn faces(&self) -> impl Iterator<Item = Face> + '_ {
        let bundled = self
            .bundled_dir
            .iter()
            .flat_map(|dir| BUNDLED.iter().map(move |b| bundled_row_face(dir, b)));
        let installed = self.installed.iter().flat_map(|f| {
            f.faces.iter().map(move |x| Face {
                alias: f.alias.clone(),
                weight: x.weight,
                italic: x.italic,
                stretch: x.stretch,
                file: Some(FaceFile {
                    path: x.path.clone(),
                    index: x.index,
                }),
            })
        });
        let downloaded = self.downloaded.iter().flat_map(|d| {
            let alias = d.alias();
            d.faces.iter().map(move |f| Face {
                alias: alias.clone(),
                weight: f.weight,
                italic: f.italic,
                stretch: NORMAL_WIDTH,
                file: Some(f.file.clone()),
            })
        });
        bundled.chain(installed).chain(downloaded)
    }

    /// The face whose `face_id` is `id`, if the index holds one; None for any other id (a path IPC made up).
    pub fn face_by_id(&self, id: &str) -> Option<&Face> {
        self.by_id
            .get_or_init(|| {
                self.faces()
                    .filter_map(|face| Some((face_id(face.file.as_ref()?), face)))
                    .collect()
            })
            .get(id)
    }

    /// The downloaded family `family` at exactly `version`: another version never stands in.
    pub fn downloaded(&self, family: &str, version: &str) -> Option<&Downloaded> {
        self.downloaded
            .iter()
            .find(|d| d.family == family && d.version == version)
    }

    /// The CJK a face draws itself: a downloaded family's coverage, an installed family's coverage bits (a picked
    /// Hiragino draws its own kanji and kana), none for a bundled face (none of the twelve has CJK).
    pub fn cjk_cover(&self, face: &Face) -> CjkCover {
        if let Some(d) = self.downloaded.iter().find(|d| d.alias() == face.alias) {
            return d.cover;
        }
        self.installed_family_of(face)
            .map_or(CjkCover::NONE, |f| cover_of(f.coverage))
    }

    /// The installed family (or width of one) `face` is in: registered under its alias, at its width.
    fn installed_family_of(&self, face: &Face) -> Option<&InstalledFamily> {
        self.installed
            .iter()
            .find(|f| f.alias == face.alias && f.faces.iter().any(|x| x.stretch == face.stretch))
    }

    /// Whether `face` is an installed face over the cap (`sfnt::MAX_FACE_BYTES`): never read or sent to the webview,
    /// which draws it from the system by name.
    pub fn too_large(&self, face: &Face) -> bool {
        let Some(file) = &face.file else {
            return false;
        };
        self.installed_family_of(face).is_some_and(|f| {
            f.faces.iter().any(|x| {
                x.path == file.path
                    && x.index == file.index
                    && x.bytes > crate::sfnt::MAX_FACE_BYTES
            })
        })
    }

    /// A family name as people type it, case-insensitively. A name never resolves to a Google family not downloaded
    /// while one of that name is here: bundled first, then a Google
    /// family downloaded here, then this computer's, then Google's catalogue at its version (to be downloaded).
    /// Adobe NotDef is never one.
    pub fn resolve_family(&self, name: &str) -> Option<FontRef> {
        let want = name.trim().to_lowercase();
        if let Some(b) = BUNDLED
            .iter()
            .find(|b| b.category != Category::Fallback && b.family.to_lowercase() == want)
        {
            return Some(FontRef::bundled(b.family));
        }
        let google = self.catalogue.as_ref().and_then(|c| c.find(&want));
        if let Some(d) = self.downloaded_named(&want, google.map(|g| g.version.as_str())) {
            return Some(d.font());
        }
        if let Some(f) = self
            .installed
            .iter()
            .find(|f| f.family.to_lowercase() == want)
        {
            return Some(FontRef {
                family: f.family.clone(),
                source: FontSource::Installed,
            });
        }
        google.map(|g| g.font())
    }

    /// The Google family named `want` (lower-cased) downloaded here: at the catalogue's `current` version when that
    /// one is, else the newest downloaded.
    fn downloaded_named(&self, want: &str, current: Option<&str>) -> Option<&Downloaded> {
        let number = |d: &&Downloaded| version_number(&d.version);
        let named = || {
            self.downloaded
                .iter()
                .filter(|d| d.family.to_lowercase() == want)
        };
        named()
            .find(|d| Some(d.version.as_str()) == current)
            .or_else(|| named().max_by_key(number))
    }

    /// The (weight, italic) of every face `font` has here, or can have: a Google family's downloaded faces, else its
    /// catalogue files at that exact version (so a pick snaps to a weight it has). Empty for a font that isn't here.
    pub fn weights(&self, font: &FontRef) -> Vec<(u16, bool)> {
        match &font.source {
            FontSource::Bundled if crate::bundled::fallback_only(&font.family) => Vec::new(),
            FontSource::Bundled => BUNDLED
                .iter()
                .filter(|b| b.family == font.family)
                .map(|b| (b.weight, b.italic))
                .collect(),
            FontSource::Installed => self
                .installed_family(&font.family)
                .map(|f| f.faces.iter().map(|x| (x.weight, x.italic)).collect())
                .unwrap_or_default(),
            FontSource::Google { version } => match self.downloaded(&font.family, version) {
                Some(d) => d.faces.iter().map(|f| (f.weight, f.italic)).collect(),
                None => self
                    .catalogue
                    .as_ref()
                    .and_then(|c| c.exact(&font.family, version))
                    .map(|g| g.faces())
                    .unwrap_or_default(),
            },
        }
    }

    /// The weight and style a pick of `font` draws at, so a picked family never asks for a face it lacks: the
    /// face `nearest` picks, or the asked ones for a font that isn't here.
    pub fn snap_weight(&self, font: &FontRef, weight: u16, italic: bool) -> (u16, bool) {
        let w = self.weights(font);
        crate::matching::nearest(&w, weight, italic).map_or((weight, italic), |i| w[i])
    }

    /// Whether `font` has faces here and none of them is upright (Rubik ships only Black Italic). The engine registers
    /// such a family's faces upright too and shapes it upright, so the system's script fallbacks, which are upright,
    /// still answer for letters it lacks.
    pub fn italic_only(&self, font: &FontRef) -> bool {
        let w = self.weights(font);
        !w.is_empty() && w.iter().all(|&(_, italic)| italic)
    }
}
