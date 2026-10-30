//! Google's font catalogue: a compact snapshot of the Web
//! Fonts Developer API list, built at release time by `scripts/build-font-catalogue.mjs` (joined with licence folders
//! and file sizes), committed gzipped and compiled in. Browsing it is offline and sends nothing. Only a download
//! (`download`) reaches Google, at `GSTATIC` and a file's tail: the host is fixed there, and the snapshot holds tails
//! only.
//!
//! The catalogue is a value in a `FontIndex` (`FontIndex::with_catalogue`): nothing reads a global, and
//! tests pass their own. `shipped()` decodes the compiled-in snapshot once, on first use, and shares it; the app asks
//! for it off the main thread. Held, 1,900 families and 7,500 files take about 1.5 MB (each file about 90 bytes, each
//! family about 350 with its names and subsets); the unpacked JSON (about 740 KB) is dropped once it is read.
//!
//! The snapshot is checked as it is read: a family's name and version must make a `FontRef` the model accepts, its
//! licence must be one of Google's three folders and its tails plain paths. A snapshot that breaks a rule isn't used
//! (`the_shipped_snapshot_parses` holds the compiled one to it), so nothing odd reaches a project or a download URL.

use std::io::Read;
use std::sync::{Arc, OnceLock};

use klipolo_model::font::{FontRef, FontSource};
use serde::Serialize;
use ts_rs::TS;

use crate::bundled::Category;
use crate::coverage::Coverage;
use crate::matching::nearest;

mod snapshot;

pub use snapshot::subsets_coverage;

/// Where a file's tail is fetched from.
pub const GSTATIC: &str = "https://fonts.gstatic.com/s/";
/// A family whose files total more than this is downloaded a weight at a time (Chinese, Japanese, Korean and a few
/// Noto families); any other downloads whole (the fonts research, "Downloading a family").
pub const WHOLE_FAMILY_MAX: u64 = 15_000_000;
/// The most a snapshot may unpack to: the real one is about 740 KB.
const MAX_RAW: u64 = 16 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Catalogue {
    /// The day the snapshot was built (`2026-10-22`).
    pub date: String,
    /// The labelled sample (`google-fonts.fixture.json`), until an API key builds the real one: Settings says so,
    /// and the release scanner refuses it outside a test build.
    pub sample: bool,
    /// By popularity, most popular first.
    pub families: Vec<GoogleFamily>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GoogleFamily {
    pub family: String,
    /// Its folder in `google/fonts` (`playfairdisplay`), where its licence file is.
    pub slug: String,
    pub category: Category,
    /// Its rank by popularity (0 is the most popular), and by trending.
    pub popularity: u32,
    pub trending: u32,
    /// Google's version (`v31`): a project names it exactly.
    pub version: String,
    /// Google's subsets (`latin`, `cyrillic`, `japanese`…).
    pub subsets: Vec<String>,
    /// `ofl`, `apache` or `ufl`: its folder in `google/fonts`.
    pub licence: String,
    pub files: Vec<GoogleFile>,
    /// The tail of Google's small file that draws the family's own name, for the online preview after consent;
    /// empty when Google gave none, or not a plain tail.
    pub menu: String,
    /// The scripts its subsets cover, as `coverage` bits: "Covers my text" reads these.
    pub(crate) coverage: Coverage,
    /// Bundled with Klipolo too: listed and resolved as the bundled family only.
    pub(crate) bundled: bool,
    /// Tests only (`test-hosts`): the local server its files and licence come from, in place of Google's two hosts.
    #[cfg(feature = "test-hosts")]
    pub(crate) test_base: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoogleFile {
    pub weight: u16,
    pub italic: bool,
    /// The path after `GSTATIC` (`montserrat/v31/JTUHjIg1….ttf`).
    pub url_tail: String,
    pub bytes: u64,
}

/// What Settings says of the font list: "Font list from {date}", or "Font list: sample (test build)".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CatalogueInfo {
    pub date: String,
    pub sample: bool,
    pub families: u32,
}

impl Catalogue {
    /// The snapshot's JSON (unpacked), checked; an error names the first family that breaks a rule.
    pub fn from_json(bytes: &[u8]) -> Result<Catalogue, String> {
        let snapshot: snapshot::Snapshot = serde_json::from_slice(bytes)
            .map_err(|e| format!("the font catalogue can't be read: {e}"))?;
        let mut families = snapshot
            .families
            .into_iter()
            .map(snapshot::family)
            .collect::<Result<Vec<_>, _>>()?;
        families.sort_by_key(|f| f.popularity);
        Ok(Catalogue {
            date: snapshot.date,
            sample: snapshot.sample,
            families,
        })
    }

    /// The gzipped snapshot, unpacked (at most 16 MB) and checked.
    pub fn from_gz(gz: &[u8]) -> Result<Catalogue, String> {
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(gz)
            .take(MAX_RAW)
            .read_to_end(&mut raw)
            .map_err(|e| format!("the font catalogue can't be unpacked: {e}"))?;
        Self::from_json(&raw)
    }

    /// The family named `family_lower` (already lower-cased), compared case-insensitively without allocating.
    pub fn find(&self, family_lower: &str) -> Option<&GoogleFamily> {
        self.families.iter().find(|f| {
            f.family
                .chars()
                .flat_map(char::to_lowercase)
                .eq(family_lower.chars())
        })
    }

    /// The family named exactly `family` at exactly `version` (what a project's `FontRef` names).
    pub fn exact(&self, family: &str, version: &str) -> Option<&GoogleFamily> {
        self.families
            .iter()
            .find(|f| f.family == family && f.version == version)
    }

    /// Every file of `f` together.
    pub fn family_bytes(&self, f: &GoogleFamily) -> u64 {
        f.bytes()
    }

    /// What a pick of `f` downloads (`GoogleFamily::download_bytes`).
    pub fn download_bytes(&self, f: &GoogleFamily) -> u64 {
        f.download_bytes()
    }

    pub fn info(&self) -> CatalogueInfo {
        CatalogueInfo {
            date: self.date.clone(),
            sample: self.sample,
            families: u32::try_from(self.families.len()).unwrap_or(u32::MAX),
        }
    }

    /// No families: what the app holds if the compiled-in snapshot couldn't be read (a test stops that build).
    fn none() -> Catalogue {
        Catalogue {
            date: String::new(),
            sample: true,
            families: Vec::new(),
        }
    }
}

impl GoogleFamily {
    /// The `FontRef` a pick of it stores: Google, at the catalogue's version.
    pub fn font(&self) -> FontRef {
        FontRef {
            family: self.family.clone(),
            source: FontSource::Google {
                version: self.version.clone(),
            },
        }
    }

    /// Every file together.
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|x| x.bytes).sum()
    }

    /// What a pick downloads: the whole family up to `WHOLE_FAMILY_MAX`, else its regular face (the nearest to 400
    /// upright), since such a family downloads a weight at a time.
    pub fn download_bytes(&self) -> u64 {
        let all = self.bytes();
        if all <= WHOLE_FAMILY_MAX {
            return all;
        }
        nearest(&self.faces(), 400, false).map_or(all, |i| self.files[i].bytes)
    }

    /// Its files' (weight, italic), in the files' order.
    pub fn faces(&self) -> Vec<(u16, bool)> {
        self.files.iter().map(|f| (f.weight, f.italic)).collect()
    }

    /// The scripts its subsets cover.
    pub fn coverage(&self) -> Coverage {
        self.coverage
    }

    /// Whether Klipolo bundles a family of the same name (then it is listed and resolved as the bundled one only).
    pub fn bundled(&self) -> bool {
        self.bundled
    }

    /// Tests only (`test-hosts`): a sans family under the OFL with `files`, its slug its lower-cased name, downloaded
    /// from the local server at `base` (`test_server::family_on`), or from nowhere (a closed local port) for None.
    #[cfg(feature = "test-hosts")]
    #[doc(hidden)]
    pub fn for_tests(
        family: &str,
        version: &str,
        files: Vec<GoogleFile>,
        base: Option<String>,
    ) -> GoogleFamily {
        let subsets = vec!["latin".to_owned()];
        GoogleFamily {
            family: family.to_owned(),
            slug: family.to_lowercase().replace(' ', ""),
            category: Category::Sans,
            popularity: 0,
            trending: 0,
            version: version.to_owned(),
            coverage: snapshot::subsets_coverage(&subsets),
            subsets,
            licence: "ofl".into(),
            menu: files
                .first()
                .map(|f| f.url_tail.clone())
                .unwrap_or_default(),
            files,
            bundled: false,
            test_base: base,
        }
    }

    /// Tests only: a one-file family (400 upright) whose file's tail is `tail`, as a snapshot couldn't give.
    #[cfg(feature = "test-hosts")]
    #[doc(hidden)]
    pub fn for_tests_with_tail(family: &str, tail: &str) -> GoogleFamily {
        let file = GoogleFile {
            weight: 400,
            italic: false,
            url_tail: tail.to_owned(),
            bytes: 1_000,
        };
        GoogleFamily::for_tests(family, "v1", vec![file], None)
    }
}

/// The compiled-in snapshot, decoded on first use.
static SHIPPED: OnceLock<Arc<Catalogue>> = OnceLock::new();

fn shipped_ref() -> &'static Arc<Catalogue> {
    SHIPPED.get_or_init(|| {
        let gz = include_bytes!("../catalogue/google-fonts.json.gz");
        Arc::new(Catalogue::from_gz(gz).unwrap_or_else(|e| {
            // The gate's `the_shipped_snapshot_parses` and the release scanner stop such a build; never silent.
            tracing::error!(error = %e, "fonts: the Google Fonts snapshot compiled in can't be read; no Google families");
            Catalogue::none()
        }))
    })
}

/// The compiled-in snapshot, decoded on first use and shared: the app's index holds it through this one `Arc`.
pub fn shipped() -> Arc<Catalogue> {
    shipped_ref().clone()
}

/// The compiled-in snapshot (`shipped()`), borrowed for the process's life.
pub fn catalogue() -> &'static Catalogue {
    shipped_ref()
}
