//! The fonts a project uses that aren't here: never silent. A font is missing exactly when the one resolver answers a
//! stand-in (only the project's exact Google version counts), so the preflight, the engine and the webview agree.
//! Chinese, Japanese or Korean text whose Noto Sans isn't downloaded is missing too, for the scripts the engine's runs
//! ask for (`cjk_segments`), less what the text's own face covers (a picked Hiragino draws its own kanji).
//!
//! Each missing font says where it is used (its layers, the captions), which weights, what its download takes when
//! Google still serves that exact version, and the version Google serves now when it doesn't ("Use {family}
//! {version}", a Replace, never a silent swap). Cost: one resolve per distinct (font, weight, style), one CJK scan of
//! each layer's words (`has_cjk` first, so Latin titles allocate nothing for it), and O(layers × missing fonts) to
//! group them; asked after an edit settles, never per frame.

use std::collections::HashMap;

use klipolo_model::font::{FontRef, FontSource};
use klipolo_model::project::Project;
use klipolo_model::text_language::{CjkScript, cjk_script};
use klipolo_model::timeline::{ItemContent, Solo};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::cjk::{CjkCover, cjk_segments, has_cjk};
use crate::download::{Which, which_for};
use crate::fallback::cjk_family;
use crate::google::GoogleFamily;
use crate::index::FontIndex;
use crate::matching::nearest;
use crate::resolve::resolve;

mod scenes;
pub use scenes::{SceneUse, missing_with};

/// A font a project uses that isn't here, and where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MissingFont {
    /// The font the project names; for CJK text (`cjk`), the catalogue's Noto Sans of that script.
    pub font: FontRef,
    /// The (weight, italic) its users ask for, in the order first used.
    pub weights: Vec<(u16, bool)>,
    /// The text layers that use it and the export draws (by id), in timeline order.
    pub layers: Vec<String>,
    /// The text layers that use it on tracks the export doesn't draw (hidden, or left out by a solo): Replace and the
    /// badge reach them too, but they never stop an export.
    pub hidden_layers: Vec<String>,
    /// The captions use it (they are on).
    pub captions: bool,
    /// The motion graphic layers whose `Font` setting it is: (layer id, setting key), each once. A
    /// scene layer is never in `layers`: Replace writes these settings, never a text style.
    #[serde(default)]
    pub scene_params: Vec<(String, String)>,
    /// A motion graphic the export draws uses it, a setting or a font written inside its saved version (which Replace
    /// can't change: "Export with Inter instead" covers it).
    #[serde(default)]
    pub in_scenes: bool,
    /// What downloading exactly that version takes, for the weights used: only while Google's catalogue serves it.
    pub download_bytes: Option<u64>,
    /// The version Google's catalogue serves now, when it isn't the project's (the "Use {family} {version}" offer).
    pub current: Option<FontRef>,
    /// Chinese, Japanese or Korean text whose Noto Sans isn't downloaded: no stand-in ever draws it in an export.
    pub cjk: Option<CjkScript>,
}

impl MissingFont {
    /// Whether the export draws something in it: a layer on a track it shows, or the captions.
    pub fn drawn(&self) -> bool {
        !self.layers.is_empty() || self.captions || self.in_scenes
    }
}

/// How a message names missing fonts (`exports.fontsMissing`, the export's stop): each family once, in order, the
/// first five joined with commas, then "…" (no words: the message around it is the person's language).
pub fn names<'a>(families: impl IntoIterator<Item = &'a str>) -> String {
    let mut seen: Vec<&str> = Vec::new();
    let mut more = false;
    for family in families {
        if seen.contains(&family) {
            continue;
        }
        if seen.len() == 5 {
            more = true;
            break;
        }
        seen.push(family);
    }
    let mut out = seen.join(", ");
    if more {
        out.push_str(", …");
    }
    out
}

/// The CJK scripts `text` in `language` asks a fallback for, its own face covering `cover`: exactly the scripts of the
/// engine's runs (`cjk_segments`), each once, in the order first met.
pub fn cjk_needed(text: &str, language: &str, cover: CjkCover) -> Vec<CjkScript> {
    let mut out = Vec::new();
    if !has_cjk(text) {
        return out;
    }
    for (_, script) in cjk_segments(text, language, cover) {
        if !out.contains(&script) {
            out.push(script);
        }
    }
    out
}

/// Who uses a font: a text layer (and whether the export draws its track), or the captions.
#[derive(Clone, Copy)]
enum User<'a> {
    Layer(&'a str, bool),
    Captions,
    /// A motion graphic layer's text: its id, the `Font` setting it comes from, whether the export draws it.
    Scene(&'a str, Option<&'a str>, bool),
}

/// Adds `user` of `font` at `face` to the entry for that font (or for that CJK script), making it if it's new.
fn add(
    out: &mut Vec<MissingFont>,
    font: FontRef,
    cjk: Option<CjkScript>,
    face: (u16, bool),
    user: User,
) {
    let at = out
        .iter()
        .position(|m| m.cjk == cjk && (cjk.is_some() || m.font == font));
    let m = match at {
        Some(i) => &mut out[i],
        None => {
            out.push(MissingFont {
                font,
                weights: Vec::new(),
                layers: Vec::new(),
                hidden_layers: Vec::new(),
                captions: false,
                scene_params: Vec::new(),
                in_scenes: false,
                download_bytes: None,
                current: None,
                cjk,
            });
            out.last_mut().expect("just pushed")
        }
    };
    if !m.weights.contains(&face) {
        m.weights.push(face);
    }
    match user {
        User::Layer(id, drawn) => {
            let list = if drawn {
                &mut m.layers
            } else {
                &mut m.hidden_layers
            };
            if !list.iter().any(|l| l == id) {
                list.push(id.to_owned());
            }
        }
        User::Captions => m.captions = true,
        User::Scene(id, param, drawn) => {
            if let Some(key) = param
                && !m.scene_params.iter().any(|(l, k)| l == id && k == key)
            {
                m.scene_params.push((id.to_owned(), key.to_owned()));
            }
            m.in_scenes |= drawn;
        }
    }
}

/// Whether `cover` draws a text language's script itself (captions: their words aren't read here).
fn covers(cover: CjkCover, script: CjkScript) -> bool {
    match script {
        CjkScript::Jp => cover.han && cover.kana,
        CjkScript::Kr => cover.hangul,
        CjkScript::Sc | CjkScript::Tc => cover.han,
    }
}

/// The bytes a download of `g` takes for `faces`: the whole family once (15 MB or less), else each face nearest a
/// weight once (`which_for`, as `fonts_download` fetches).
fn bytes_for(g: &GoogleFamily, faces: &[(u16, bool)]) -> u64 {
    let mut picked: Vec<(u16, bool)> = Vec::new();
    for &(weight, italic) in faces {
        match which_for(g, weight, italic) {
            Which::All => return g.bytes(),
            Which::One { weight, italic } if !picked.contains(&(weight, italic)) => {
                picked.push((weight, italic))
            }
            Which::One { .. } => {}
        }
    }
    let size = |face: &(u16, bool)| {
        g.files
            .iter()
            .find(|f| (f.weight, f.italic) == *face)
            .map_or(0, |f| f.bytes)
    };
    picked.iter().map(size).sum()
}

/// A CJK Noto's download for `weights`: each weight's nearest upright face once (`fonts_cjk_download`).
fn cjk_bytes(g: &GoogleFamily, weights: &[(u16, bool)]) -> u64 {
    let upright: Vec<(u16, bool)> = g.faces().into_iter().filter(|f| !f.1).collect();
    let mut picked: Vec<(u16, bool)> = Vec::new();
    for &(weight, _) in weights {
        if let Some(i) = nearest(&upright, weight, false)
            && !picked.contains(&upright[i])
        {
            picked.push(upright[i]);
        }
    }
    picked
        .iter()
        .filter_map(|face| g.files.iter().find(|f| (f.weight, f.italic) == *face))
        .map(|f| f.bytes)
        .sum()
}

/// Fills in what each missing font's download takes, or the version Google serves now.
fn offers(index: &FontIndex, out: &mut [MissingFont]) {
    let Some(catalogue) = index.catalogue() else {
        return;
    };
    for m in out {
        if m.cjk.is_some() {
            m.download_bytes = catalogue
                .exact(&m.font.family, version_of(&m.font).unwrap_or_default())
                .map(|g| cjk_bytes(g, &m.weights));
            continue;
        }
        let Some(version) = version_of(&m.font) else {
            continue;
        };
        match catalogue.exact(&m.font.family, version) {
            Some(g) => m.download_bytes = Some(bytes_for(g, &m.weights)),
            None => {
                m.current = catalogue
                    .find(&m.font.family.to_lowercase())
                    .filter(|g| !g.bundled())
                    .map(GoogleFamily::font)
            }
        }
    }
}

fn version_of(font: &FontRef) -> Option<&str> {
    match &font.source {
        FontSource::Google { version } => Some(version),
        _ => None,
    }
}

/// The Noto Sans of `script` as the catalogue names it (a project's font for that row), else by its name alone.
fn noto(index: &FontIndex, script: CjkScript) -> FontRef {
    let family = cjk_family(script);
    index
        .catalogue()
        .and_then(|c| c.find(&family.to_lowercase()))
        .map(GoogleFamily::font)
        .unwrap_or_else(|| FontRef {
            family: family.to_owned(),
            source: FontSource::Installed,
        })
}

/// Every font `p` uses that isn't here, with its users: each text layer (those the export draws, its track shown as the
/// engine's solo rule says, apart from the others), and the captions while they're on. Fonts first used first.
/// Only an entry the export draws (`drawn`) stops one.
pub fn missing_in(index: &FontIndex, p: &Project, text_language: &str) -> Vec<MissingFont> {
    missing_with(index, p, text_language, &[])
}

/// [`missing_in`] without its offers: the text layers and the captions.
fn layers_and_captions(index: &FontIndex, p: &Project, text_language: &str) -> Vec<MissingFont> {
    let mut out: Vec<MissingFont> = Vec::new();
    let solo = Solo::of(&p.timeline.tracks);
    let layers = p
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.items.iter().map(move |i| (i, solo.shows(t))));
    // Each distinct face is resolved once: a long video's hundred titles in one font ask once.
    let mut resolved: HashMap<(&FontRef, u16, bool), (bool, CjkCover)> = HashMap::new();
    for (item, drawn) in layers {
        let ItemContent::Text { text, style } = &item.content else {
            continue;
        };
        let face = (style.weight, style.italic);
        let user = User::Layer(&item.id, drawn);
        let (stand_in, cover) = *resolved
            .entry((&style.font, style.weight, style.italic))
            .or_insert_with(|| {
                let r = resolve(index, &style.font, style.weight, style.italic);
                (r.stand_in, index.cjk_cover(&r.face))
            });
        if stand_in {
            add(&mut out, style.font.clone(), None, face, user);
        }
        for script in cjk_needed(text, text_language, cover) {
            if index.cjk(script).is_none() {
                add(&mut out, noto(index, script), Some(script), face, user);
            }
        }
    }
    if p.captions.enabled {
        let (style, _) = klipolo_model::captions::styles::CaptionStyles::builtin()
            .for_settings(&p.captions, &p.settings.formats);
        let font = p
            .captions
            .font
            .clone()
            .unwrap_or_else(|| FontRef::bundled(&style.font.family));
        let face = (style.font.weight, style.font.italic);
        let r = resolve(index, &font, face.0, face.1);
        if r.stand_in {
            add(&mut out, font, None, face, User::Captions);
        }
        if let Some(script) = cjk_script(text_language)
            && !covers(index.cjk_cover(&r.face), script)
            && index.cjk(script).is_none()
        {
            add(
                &mut out,
                noto(index, script),
                Some(script),
                face,
                User::Captions,
            );
        }
    }
    out
}
