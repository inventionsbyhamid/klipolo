//! One ranked search for the font menu and chat: exact, then prefix, then word-prefix, then substring, then in-order
//! letters. Within a band, a tie breaks as a name resolves: the bundled set, then Google families downloaded here, then
//! this computer's, then Google's not downloaded; each in the index's order (the bundled order, Google's popularity,
//! this computer's names). The query is lower-cased once; names are compared case-insensitively through char iterators,
//! so nothing is allocated per name. "Covers my text" reads coverage bits: computed at scan time for installed
//! families, from the subsets when the catalogue is read for Google's. No file is read here, except the bundled files'
//! coverage, once per process.
//!
//! Cost: O(families × name length) per query, about 1,900 Google names and 300 to 2,300 installed ones (the release
//! test's budget is 5 ms a query). The rows it returns are capped by `limit` for each source on its own (bundled,
//! Google's, this computer's), so 200 Google families matching "sans" never crowd this computer's out.

use klipolo_model::font::FontSource;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::bundled::{BUNDLED, Category};
use crate::coverage::{bundled_coverage, needs};
use crate::google::GoogleFamily;
use crate::index::FontIndex;

/// Where a row's family comes from, as the menu shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RowState {
    Bundled,
    Installed,
    Downloaded,
    /// A Google family not downloaded yet: the menu shows "Download ({size})".
    Download,
}

/// One family in the menu.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FontRow {
    pub family: String,
    pub source: FontSource,
    /// None for an installed family (no catalogue says what it is): listed under All only.
    pub category: Option<Category>,
    /// Its faces' (weight, italic).
    pub faces: Vec<(u16, bool)>,
    /// The face a pick draws at, for the filter's asked weight and style: Rust's `nearest`, never a TS copy.
    pub snapped: (u16, bool),
    pub state: RowState,
    /// What a pick of a Google family not downloaded yet downloads: the whole family up to 15 MB, else its
    /// regular face (`Catalogue::download_bytes`).
    pub size_bytes: Option<u64>,
    /// A family downloaded a weight at a time (over 15 MB): the catalogue's faces at that version not downloaded yet,
    /// each with what its download takes, for the Weight menu's "Download {weight} ({size})".
    pub more_faces: Vec<MoreFace>,
    /// An installed variable family: only its default weight draws for now.
    pub default_weight_only: bool,
    /// An installed family's name in the system (`InstalledFamily::alias`): the menu draws the row by it (`local()`),
    /// at `stretch`. None for Klipolo's own families.
    pub system_name: Option<String>,
    /// Its faces' OS/2 width class (5 normal; "Futura Condensed" is 3).
    pub stretch: u16,
    /// The face a pick draws is over the cap (`sfnt::MAX_FACE_BYTES`): the menu says "Too large to preview", and the
    /// canvas draws it from the system by name.
    pub too_large: bool,
}

/// One more face of a family downloaded a weight at a time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MoreFace {
    pub weight: u16,
    pub italic: bool,
    pub bytes: u64,
}

/// What a search keeps.
#[derive(Clone, Debug, Default, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchFilter {
    pub category: Option<Category>,
    /// Only families with glyphs for this text's letters.
    pub covers: Option<String>,
    /// The weight and style the selection has now, which each row's `snapped` answers for (None: 400 upright).
    pub want: Option<(u16, bool)>,
    /// Google Fonts is off in Settings: no family that needs a download is listed; downloaded ones are,
    /// since they're on this computer. The app sets it from the setting, never the webview.
    #[serde(skip)]
    #[ts(skip)]
    pub google_off: bool,
}

/// Whether `hay` starts with `needle_lower`, case-insensitively, without allocating.
fn starts_ci(hay: &str, needle_lower: &str) -> bool {
    let mut h = hay.chars().flat_map(char::to_lowercase);
    needle_lower.chars().all(|c| h.next() == Some(c))
}

/// How well `name` matches `q` (already lower-cased): 0 is best; None doesn't match.
pub fn rank(name: &str, q: &str) -> Option<u8> {
    if q.is_empty() {
        return Some(4);
    }
    if starts_ci(name, q) {
        let exact = name.chars().flat_map(char::to_lowercase).count() == q.chars().count();
        return Some(if exact { 0 } else { 1 });
    }
    if name.split_whitespace().skip(1).any(|w| starts_ci(w, q)) {
        return Some(2);
    }
    if name.char_indices().any(|(i, _)| starts_ci(&name[i..], q)) {
        return Some(3);
    }
    let mut chars = name.chars().flat_map(char::to_lowercase);
    q.chars().all(|c| chars.any(|x| x == c)).then_some(5)
}

/// A family that matched, before its row is built: only the rows returned are built.
#[derive(Clone, Copy)]
pub(crate) enum Hit<'a> {
    /// `BUNDLED[i]`, its family's first face.
    Bundled(usize),
    /// `index.installed()[i]`.
    Installed(usize),
    /// A family of the index's catalogue.
    Google(&'a GoogleFamily),
}

impl Hit<'_> {
    /// Which source it is from, for the cap each source has: 0 bundled, 1 Google's, 2 this computer's.
    fn source(&self) -> usize {
        match self {
            Hit::Bundled(_) => 0,
            Hit::Google(_) => 1,
            Hit::Installed(_) => 2,
        }
    }
}

/// The families matching `query` and `filter`, best first, at most `limit` from each source: the bundled set (Adobe
/// NotDef never), Google's catalogue by popularity (a family Klipolo bundles only as the bundled row) and this
/// computer's families. A rank's tie goes to the bundled set, a downloaded Google family, this computer's, then a
/// Google family not downloaded: "roboto" leads with this Mac's Roboto, as `resolve_family` does.
pub fn search(index: &FontIndex, query: &str, filter: &SearchFilter, limit: usize) -> Vec<FontRow> {
    let q = query.trim().to_lowercase();
    let want = filter.covers.as_deref().map(needs).unwrap_or(0);
    let covers = |c: u32| c & want == want;
    // (rank, tier, order, hit). Tiers: 0 bundled, 1 a Google family downloaded here, 2 this computer's, 3 a Google
    // family not downloaded.
    let mut hits: Vec<(u8, u8, usize, Hit)> = Vec::new();
    let mut order = 0usize;
    let mut seen: Vec<&str> = Vec::with_capacity(13);
    for (i, b) in BUNDLED.iter().enumerate() {
        // Each family once; Adobe NotDef is never listed.
        if seen.contains(&b.family) || b.category == Category::Fallback {
            continue;
        }
        seen.push(b.family);
        let fits = filter.category.is_none_or(|c| c == b.category)
            && (want == 0 || covers(bundled_coverage(index.bundled_dir(), i)));
        if let (true, Some(r)) = (fits, rank(b.family, &q)) {
            hits.push((r, 0, order, Hit::Bundled(i)));
        }
        order += 1;
    }
    for g in index.catalogue().map_or(&[][..], |c| &c.families) {
        // Downloaded here (at the catalogue's version, or only an older one): it draws, as a name resolves.
        let here = index.download_of(&g.family, &g.version).is_some();
        let fits = !g.bundled()
            && (here || !filter.google_off)
            && filter.category.is_none_or(|c| c == g.category)
            && covers(g.coverage());
        if let (true, Some(r)) = (fits, rank(&g.family, &q)) {
            let tier = if here { 1 } else { 3 };
            hits.push((r, tier, order, Hit::Google(g)));
        }
        order += 1;
    }
    for (i, f) in index.installed().iter().enumerate() {
        // Installed families have no category: listed under All only.
        let fits = filter.category.is_none() && covers(f.coverage);
        if let (true, Some(r)) = (fits, rank(&f.family, &q)) {
            hits.push((r, 2, order, Hit::Installed(i)));
        }
        order += 1;
    }
    hits.sort_unstable_by_key(|&(r, tier, o, _)| (r, tier, o));
    let want_face = filter.want.unwrap_or((400, false));
    let mut taken = [0usize; 3];
    hits.into_iter()
        .filter(|(_, _, _, hit)| {
            let n = &mut taken[hit.source()];
            *n += 1;
            *n <= limit
        })
        .map(|(_, _, _, hit)| row(index, hit, want_face))
        .collect()
}

/// `hit`'s row, with the face a pick at `(weight, italic)` draws (`snapped`).
pub(crate) fn row(index: &FontIndex, hit: Hit, (weight, italic): (u16, bool)) -> FontRow {
    let snap = |faces: &[(u16, bool)]| {
        crate::matching::nearest(faces, weight, italic).map_or((weight, italic), |i| faces[i])
    };
    match hit {
        Hit::Bundled(i) => {
            let family = BUNDLED[i].family;
            let faces: Vec<(u16, bool)> = BUNDLED
                .iter()
                .filter(|x| x.family == family)
                .map(|x| (x.weight, x.italic))
                .collect();
            FontRow {
                family: family.into(),
                source: FontSource::Bundled,
                category: Some(BUNDLED[i].category),
                snapped: snap(&faces),
                faces,
                state: RowState::Bundled,
                size_bytes: None,
                more_faces: Vec::new(),
                default_weight_only: false,
                system_name: None,
                stretch: crate::installed::NORMAL_WIDTH,
                too_large: false,
            }
        }
        Hit::Installed(i) => {
            let f = &index.installed()[i];
            let faces: Vec<(u16, bool)> = f.faces.iter().map(|x| (x.weight, x.italic)).collect();
            let snapped = snap(&faces);
            let picked = f.faces.iter().find(|x| (x.weight, x.italic) == snapped);
            FontRow {
                family: f.family.clone(),
                source: FontSource::Installed,
                category: None,
                snapped,
                faces,
                state: RowState::Installed,
                size_bytes: None,
                more_faces: Vec::new(),
                default_weight_only: f.faces.iter().any(|x| x.variable),
                system_name: Some(f.alias.clone()),
                stretch: picked.map_or(crate::installed::NORMAL_WIDTH, |x| x.stretch),
                too_large: picked.is_some_and(|x| x.bytes > crate::sfnt::MAX_FACE_BYTES),
            }
        }
        Hit::Google(g) => {
            // Downloaded here (the download store): picked, it draws at once. At the catalogue's version when that one
            // is, else the newest version downloaded, which is what a pick stores (as chat's `styleText` does), never
            // the newer one that isn't here.
            let download = index.download_of(&g.family, &g.version);
            let (source, faces) = match download {
                Some(d) => (
                    d.font().source,
                    d.faces.iter().map(|f| (f.weight, f.italic)).collect(),
                ),
                None => (g.font().source, g.faces()),
            };
            let downloaded = download.is_some();
            // Downloaded a weight at a time, at the version Google serves: the faces it can still gain.
            let more_faces = match download {
                Some(d)
                    if d.version == g.version && g.bytes() > crate::google::WHOLE_FAMILY_MAX =>
                {
                    g.files
                        .iter()
                        .filter(|f| !faces.contains(&(f.weight, f.italic)))
                        .map(|f| MoreFace {
                            weight: f.weight,
                            italic: f.italic,
                            bytes: f.bytes,
                        })
                        .collect()
                }
                _ => Vec::new(),
            };
            FontRow {
                family: g.family.clone(),
                source,
                category: Some(g.category),
                snapped: snap(&faces),
                faces,
                state: if downloaded {
                    RowState::Downloaded
                } else {
                    RowState::Download
                },
                size_bytes: (!downloaded).then(|| g.download_bytes()),
                more_faces,
                default_weight_only: false,
                system_name: None,
                stretch: crate::installed::NORMAL_WIDTH,
                too_large: false,
            }
        }
    }
}
