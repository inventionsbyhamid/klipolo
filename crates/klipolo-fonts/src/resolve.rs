//! The one face resolver: same family, nearest weight by the CSS rules, then the style. The engine shapes
//! with exactly this face and the webview loads exactly these bytes, so the two never disagree. A font that isn't here
//! resolves to Inter's nearest face, marked `stand_in`.

use std::path::{Path, PathBuf};

use klipolo_model::font::{FontRef, FontSource};

use crate::alias::{STAND_IN_FAMILY, alias};
use crate::bundled::{BUNDLED, BundledFace};
use crate::fallback::Fallback;
use crate::index::FontIndex;
use crate::installed::NORMAL_WIDTH;
use crate::matching::nearest;

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FaceFile {
    pub path: PathBuf,
    /// The face's index in a collection (0 for a plain file).
    pub index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Face {
    /// The family name the face is registered under (`kl:b:montserrat`, `kl:g:poppins:v22`, or an installed family).
    pub alias: String,
    /// The face's own weight, style and width (not the asked ones): the engine asks cosmic-text for exactly these.
    pub weight: u16,
    pub italic: bool,
    /// Its OS/2 width class (5 normal; installed condensed faces are 3).
    pub stretch: u16,
    /// None only for the last-resort stand-in of a broken install (no bundled folder): nothing to register or send.
    pub file: Option<FaceFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub face: Face,
    pub stand_in: bool,
}

/// The face a bundled file is, in the bundled folder `dir`.
pub(crate) fn bundled_row_face(dir: &Path, b: &BundledFace) -> Face {
    Face {
        alias: alias(&FontRef::bundled(b.family)),
        weight: b.weight,
        italic: b.italic,
        stretch: NORMAL_WIDTH,
        file: Some(FaceFile {
            path: dir.join(b.file),
            index: 0,
        }),
    }
}

fn bundled_face(index: &FontIndex, family: &str, weight: u16, italic: bool) -> Option<Face> {
    let faces: Vec<_> = BUNDLED.iter().filter(|b| b.family == family).collect();
    let cands: Vec<(u16, bool)> = faces.iter().map(|b| (b.weight, b.italic)).collect();
    let b = faces[nearest(&cands, weight, italic)?];
    Some(bundled_row_face(index.bundled_dir()?, b))
}

/// The installed `family`'s nearest face, registered under the name the scan found it by (`alias`), at its own width.
fn installed_face(index: &FontIndex, family: &str, weight: u16, italic: bool) -> Option<Face> {
    let f = index.installed_family(family)?;
    let cands: Vec<(u16, bool)> = f.faces.iter().map(|x| (x.weight, x.italic)).collect();
    let x = &f.faces[nearest(&cands, weight, italic)?];
    Some(Face {
        alias: f.alias.clone(),
        weight: x.weight,
        italic: x.italic,
        stretch: x.stretch,
        file: Some(FaceFile {
            path: x.path.clone(),
            index: x.index,
        }),
    })
}

/// The downloaded `family` at exactly `version`, its nearest face (another version never stands in).
fn downloaded_face(
    index: &FontIndex,
    family: &str,
    version: &str,
    weight: u16,
    italic: bool,
) -> Option<Face> {
    let d = index.downloaded(family, version)?;
    let cands: Vec<(u16, bool)> = d.faces.iter().map(|f| (f.weight, f.italic)).collect();
    let f = &d.faces[nearest(&cands, weight, italic)?];
    Some(Face {
        alias: d.alias(),
        weight: f.weight,
        italic: f.italic,
        stretch: NORMAL_WIDTH,
        file: Some(f.file.clone()),
    })
}

pub fn resolve(index: &FontIndex, font: &FontRef, weight: u16, italic: bool) -> Resolved {
    let found = match &font.source {
        // Adobe NotDef is a fallback only: a project naming it is a missing font, never boxes on purpose.
        FontSource::Bundled if crate::bundled::fallback_only(&font.family) => None,
        FontSource::Bundled => bundled_face(index, &font.family, weight, italic),
        // This computer's family, its nearest face; missing on another computer.
        FontSource::Installed => installed_face(index, &font.family, weight, italic),
        // The exact version if downloaded, else missing; another downloaded version never stands in.
        FontSource::Google { version } => {
            downloaded_face(index, &font.family, version, weight, italic)
        }
    };
    match found {
        Some(face) => Resolved {
            face,
            stand_in: false,
        },
        // Never a panic: without the bundled folder (a broken install) the stand-in is the system sans-serif,
        // with no file to send, and the engine says `frame.fontMissing` as for any stand-in.
        None => Resolved {
            face: bundled_face(index, STAND_IN_FAMILY, weight, italic).unwrap_or_else(|| Face {
                alias: "sans-serif".into(),
                weight,
                italic,
                stretch: NORMAL_WIDTH,
                file: None,
            }),
            stand_in: true,
        },
    }
}

/// The face a fallback alias points at for (weight, style). The engine registers each of `kl:fallback:*`'s
/// eighteen weight × style entries from this, and the webview registers the same file under the same alias at the
/// asked weight and style. So both draw the same glyphs for a character the main face lacks. Never a system face.
pub fn resolve_fallback(
    index: &FontIndex,
    which: Fallback,
    weight: u16,
    italic: bool,
) -> Option<Face> {
    let real = match which {
        Fallback::Latin => bundled_face(index, STAND_IN_FAMILY, weight, italic)?,
        // The downloaded Noto Sans SC, TC, JP or KR's nearest face (`index.cjk(script)`); none before its download, and
        // never a system face.
        Fallback::Cjk(script) => {
            let d = index.cjk(script)?;
            let cands: Vec<(u16, bool)> = d.faces.iter().map(|f| (f.weight, f.italic)).collect();
            let f = &d.faces[nearest(&cands, weight, italic)?];
            Face {
                alias: d.alias(),
                weight: f.weight,
                italic: f.italic,
                stretch: NORMAL_WIDTH,
                file: Some(f.file.clone()),
            }
        }
        // The boxes both draw for CJK before a download: one file, declared at every weight and style.
        Fallback::Notdef => bundled_face(index, "Adobe NotDef", 400, false)?,
    };
    Some(Face {
        alias: which.alias().into(),
        weight,
        italic,
        stretch: NORMAL_WIDTH,
        file: real.file,
    })
}
