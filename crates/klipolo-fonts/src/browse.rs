//! The font menu's browse (an empty query), windowed: the webview gets each section's
//! count, then only the rows it draws (`browse_rows`), never every family of a Mac's thousands and Google's 1,900. The
//! sections are the menu's: "In this project" and "Recent" (the fonts the webview names, in its order, each only while
//! it is the very font listed: a Google font at another version isn't), "Included with Klipolo", "On this Mac", then
//! "Google Fonts" (downloaded here or not; while Google Fonts is off, only what's downloaded).
//!
//! Each call walks the families once (well under a millisecond for about 4,000) and builds only the rows asked for:
//! nothing is kept between calls, so the answer always follows the index as it is.

use klipolo_model::font::{FontRef, FontSource};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::bundled::{BUNDLED, Category};
use crate::coverage::{bundled_coverage, needs};
use crate::index::FontIndex;
use crate::search::{FontRow, Hit, SearchFilter, row};

/// A section of the menu's browse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Section {
    Project,
    Recent,
    Bundled,
    Installed,
    Google,
}

/// What the webview asks a browse for.
#[derive(Clone, Debug, Default, Deserialize, TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export)]
pub struct BrowseAsk {
    pub filter: SearchFilter,
    /// The open project's fonts ("In this project"), each once.
    pub project: Vec<FontRef>,
    /// The recently picked fonts, most recent first.
    pub recent: Vec<FontRef>,
    /// The chosen font: the list starts at its first row.
    pub value: Option<FontRef>,
}

/// A browse's sections, in order, with their rows' counts (empty ones left out), and where the chosen font's first row
/// is among all rows.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Browse {
    pub sections: Vec<(Section, u32)>,
    pub value: Option<u32>,
}

/// Every family the browse lists, in its order: (section, family).
fn list<'a>(index: &'a FontIndex, ask: &BrowseAsk) -> Vec<(Section, Hit<'a>)> {
    let filter = &ask.filter;
    let want = filter.covers.as_deref().map(needs).unwrap_or(0);
    let covers = |c: u32| c & want == want;
    let mut bundled = Vec::new();
    let mut seen: Vec<&str> = Vec::with_capacity(13);
    for (i, b) in BUNDLED.iter().enumerate() {
        if seen.contains(&b.family) || b.category == Category::Fallback {
            continue;
        }
        seen.push(b.family);
        if filter.category.is_none_or(|c| c == b.category)
            && (want == 0 || covers(bundled_coverage(index.bundled_dir(), i)))
        {
            bundled.push(Hit::Bundled(i));
        }
    }
    let installed: Vec<Hit> = if filter.category.is_none() {
        (0..index.installed().len())
            .filter(|&i| covers(index.installed()[i].coverage))
            .map(Hit::Installed)
            .collect()
    } else {
        Vec::new()
    };
    let google: Vec<Hit> = index
        .catalogue()
        .map_or(&[][..], |c| &c.families)
        .iter()
        .filter(|g| {
            let here = index.download_of(&g.family, &g.version).is_some();
            !g.bundled()
                && (here || !filter.google_off)
                && filter.category.is_none_or(|c| c == g.category)
                && covers(g.coverage())
        })
        .map(Hit::Google)
        .collect();
    // A pinned font is listed while it is that very font among the listed rows (same family and source).
    let pinned = |fonts: &[FontRef]| -> Vec<Hit<'a>> {
        let mut out = Vec::new();
        for f in fonts {
            let found = bundled
                .iter()
                .chain(&installed)
                .chain(&google)
                .find(|h| same(index, h, f))
                .copied();
            if let Some(h) = found
                && !out.iter().any(|o| same_hit(o, &h))
            {
                out.push(h);
            }
        }
        out
    };
    let project = pinned(&ask.project);
    let recent = pinned(&ask.recent);
    let tag = |s: Section, hits: Vec<Hit<'a>>| hits.into_iter().map(move |h| (s, h));
    // The 12 bundled families first: the menu opens on them, then the project's and the recent picks.
    tag(Section::Bundled, bundled)
        .chain(tag(Section::Project, project))
        .chain(tag(Section::Recent, recent))
        .chain(tag(Section::Installed, installed))
        .chain(tag(Section::Google, google))
        .collect()
}

/// Whether `hit` lists exactly `font`: its family, and its source as the row has it (a Google row is at the version
/// downloaded, else the catalogue's).
fn same(index: &FontIndex, hit: &Hit, font: &FontRef) -> bool {
    match (hit, &font.source) {
        (Hit::Bundled(i), FontSource::Bundled) => BUNDLED[*i].family == font.family,
        (Hit::Installed(i), FontSource::Installed) => index.installed()[*i].family == font.family,
        (Hit::Google(g), FontSource::Google { version }) => {
            g.family == font.family
                && index
                    .download_of(&g.family, &g.version)
                    .map_or(&g.version, |d| &d.version)
                    == version
        }
        _ => false,
    }
}

fn same_hit(a: &Hit, b: &Hit) -> bool {
    match (a, b) {
        (Hit::Bundled(x), Hit::Bundled(y)) | (Hit::Installed(x), Hit::Installed(y)) => x == y,
        (Hit::Google(x), Hit::Google(y)) => std::ptr::eq(*x, *y),
        _ => false,
    }
}

/// The browse's sections and counts, and the chosen font's first row.
pub fn browse(index: &FontIndex, ask: &BrowseAsk) -> Browse {
    let all = list(index, ask);
    let mut sections: Vec<(Section, u32)> = Vec::new();
    for (s, _) in &all {
        match sections.last_mut() {
            Some((last, n)) if last == s => *n += 1,
            _ => sections.push((*s, 1)),
        }
    }
    let value = ask.value.as_ref().and_then(|v| {
        all.iter()
            .position(|(_, h)| same(index, h, v))
            .map(|i| i as u32)
    });
    Browse { sections, value }
}

/// The browse's rows `from..to` (of all its sections' rows, in order), each with its `snapped` face for the filter's
/// asked weight and style.
pub fn browse_rows(index: &FontIndex, ask: &BrowseAsk, from: u32, to: u32) -> Vec<FontRow> {
    let want = ask.filter.want.unwrap_or((400, false));
    list(index, ask)
        .into_iter()
        .skip(from as usize)
        .take(to.saturating_sub(from) as usize)
        .map(|(_, h)| row(index, h, want))
        .collect()
}

/// The row of exactly `font` (else of a family of its name), for the menu's trigger and its Weight menu: one lookup,
/// not a search.
pub fn row_for(index: &FontIndex, font: &FontRef, want: (u16, bool)) -> Option<FontRow> {
    let all = list(index, &BrowseAsk::default());
    all.iter()
        .find(|(_, h)| same(index, h, font))
        .or_else(|| {
            all.iter().find(|(_, h)| match h {
                Hit::Bundled(i) => BUNDLED[*i].family == font.family,
                Hit::Installed(i) => index.installed()[*i].family == font.family,
                Hit::Google(g) => g.family == font.family,
            })
        })
        .map(|(_, h)| row(index, *h, want))
}
