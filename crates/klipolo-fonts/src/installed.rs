//! This computer's fonts as the menu and the resolver see them: families, each face with its file and
//! what it is, grouped from the faces the engine's scan found (`group_faces`).
//!
//! Width: cosmic-text finds a face only at its own width, so every face carries its OS/2 width class
//! and the engine asks for exactly that. A family with faces of several widths (Futura, Helvetica Neue) lists its
//! normal-width faces under its name, and each other width as its own entry ("Futura Condensed"), registered under the
//! family's own name (`alias`), so its plain weights never snap to a condensed face. A family of one width (Impact, DIN
//! Condensed) keeps its name. Within an entry each (weight, style) is listed once: the first face, which is the one
//! cosmic-text's lowest-id tie-break draws.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::coverage::Coverage;

/// The normal width (OS/2 width class 5).
pub const NORMAL_WIDTH: u16 = 5;

/// One installed face: its file, its index in a collection, and what it is. Serialized only into an export job's record
/// of the app's scan (the engine's `render::fonts::record_scan`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFace {
    pub weight: u16,
    pub italic: bool,
    /// Its OS/2 width class: 1 ultra-condensed … 5 normal … 9 ultra-expanded.
    pub stretch: u16,
    pub path: PathBuf,
    pub index: u32,
    /// A variable face: drawn at its default weight until the text stack can set axes.
    pub variable: bool,
    /// The face's own size: the file's, or for a face in a collection, its tables'. A face over the cap
    /// (`sfnt::MAX_FACE_BYTES`) is never read for the webview.
    pub bytes: u64,
}

/// An installed family, or one width of it, as the menu lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFamily {
    /// The name the menu shows and a `FontRef` keeps ("Futura Condensed").
    pub family: String,
    /// The name its faces are registered under in the font database ("Futura"): what the engine asks for, and what
    /// the webview's system lookup knows. The same as `family` but for a width split off a family.
    pub alias: String,
    pub faces: Vec<InstalledFace>,
    /// The scripts its faces cover together (computed once, at scan time).
    pub coverage: Coverage,
}

/// Families a person never picks: hidden system faces, the last-resort face, Klipolo's own aliases.
pub fn hidden(family: &str) -> bool {
    family.starts_with('.') || family == "LastResort" || family.starts_with("kl:")
}

/// One face as the scan found it: its family's name in the database, the face, and the scripts it covers.
#[derive(Clone, Debug)]
pub struct ScannedFace {
    pub family: String,
    pub face: InstalledFace,
    pub coverage: Coverage,
}

/// A width's name, after the family's ("Futura Condensed").
pub fn width_name(class: u16) -> &'static str {
    match class {
        1 => "Ultra Condensed",
        2 => "Extra Condensed",
        3 => "Condensed",
        4 => "Semi Condensed",
        6 => "Semi Expanded",
        7 => "Expanded",
        8 => "Extra Expanded",
        9 => "Ultra Expanded",
        _ => "",
    }
}

/// The scan's faces as families, in the order first seen: each family's main width (normal, else the nearest to it,
/// narrower first) under its own name, every other width as "{family} {width}" (unless a family already has that
/// name), each (weight, style) once.
pub fn group_faces(scanned: Vec<ScannedFace>) -> Vec<InstalledFamily> {
    let mut order: Vec<String> = Vec::new();
    let mut by_family: HashMap<String, Vec<(InstalledFace, Coverage)>> = HashMap::new();
    for s in scanned {
        by_family
            .entry(s.family.clone())
            .or_insert_with(|| {
                order.push(s.family.clone());
                Vec::new()
            })
            .push((s.face, s.coverage));
    }
    let names: HashSet<String> = order.iter().cloned().collect();
    let mut out = Vec::new();
    for name in order {
        let faces = by_family.remove(&name).unwrap_or_default();
        let mut widths: Vec<u16> = faces.iter().map(|(f, _)| f.stretch).collect();
        widths.sort_unstable_by_key(|&w| (w.abs_diff(NORMAL_WIDTH), w));
        widths.dedup();
        let main = widths.first().copied().unwrap_or(NORMAL_WIDTH);
        for width in widths {
            let family = if width == main {
                name.clone()
            } else {
                let split = format!("{name} {}", width_name(width));
                if names.contains(&split) {
                    continue;
                }
                split
            };
            let mut entry = InstalledFamily {
                family,
                alias: name.clone(),
                faces: Vec::new(),
                coverage: 0,
            };
            for (face, coverage) in faces.iter().filter(|(f, _)| f.stretch == width) {
                if entry
                    .faces
                    .iter()
                    .any(|f| (f.weight, f.italic) == (face.weight, face.italic))
                {
                    continue;
                }
                entry.coverage |= coverage;
                entry.faces.push(face.clone());
            }
            out.push(entry);
        }
    }
    out
}
