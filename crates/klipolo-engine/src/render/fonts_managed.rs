//! The managed faces' registration: Klipolo's own faces are pushed into a font database as face records
//! under their aliases, pointing at their files, never read until a face is used (then memory-mapped once per database).
//!
//! Two kinds of record besides, each a face record only:
//! - **upright twins.** A family with only italic faces (Rubik Black Italic, or such an installed or downloaded family)
//!   is registered upright too, from the same files, and `family_for` asks for it upright: cosmic-text falls back only
//!   to faces of the asked style, and the system's script faces are upright;
//! - **fallbacks at every weight a face has.** The fallback aliases exist at 100–900 and at each other weight an
//!   installed or downloaded face has (an installed 350), since cosmic-text matches exact weights only. They are
//!   They are normal width only: now every face is asked at normal width (`twins`), so the
//!   copies at each installed width went.
//!
//! The width twins and the emoji face at every weight are made in `twins`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use glyphon::fontdb::{Database, FaceInfo, ID, Language, Source, Stretch, Style, Weight};
use klipolo_fonts::bundled::{BUNDLED, Category};
use klipolo_fonts::fallback::{Fallback, LATIN};
use klipolo_fonts::index::FontIndex;
use klipolo_fonts::installed::NORMAL_WIDTH;
use klipolo_fonts::resolve::resolve_fallback;
use klipolo_model::font::FontRef;
use klipolo_model::text_language::CjkScript;

use super::index;
use super::twins::{emoji_weights, width_twins};

/// One face registered under an alias: its file, and the weight, style and width it is declared at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedFace {
    pub alias: String,
    pub weight: u16,
    pub italic: bool,
    /// Its OS/2 width class (5 normal).
    pub stretch: u16,
    pub path: PathBuf,
    /// The face's index in a collection (0 for a plain file).
    pub index: u32,
}

/// Every face to register for `index`: the bundled faces (Adobe NotDef only as its fallback alias), the downloaded
/// ones, the upright twins of italic-only families, then the fallback aliases at every weight × both styles.
pub(crate) fn managed_faces(index: &FontIndex) -> Vec<ManagedFace> {
    let mut faces = Vec::new();
    if let Some(dir) = index.bundled_dir() {
        for b in BUNDLED.iter().filter(|b| b.category != Category::Fallback) {
            faces.push(ManagedFace {
                alias: klipolo_fonts::alias::alias(&FontRef::bundled(b.family)),
                weight: b.weight,
                italic: b.italic,
                stretch: NORMAL_WIDTH,
                path: dir.join(b.file),
                index: 0,
            });
        }
    }
    for d in index.downloads() {
        let alias = d.alias();
        faces.extend(d.faces.iter().map(|f| ManagedFace {
            alias: alias.clone(),
            weight: f.weight,
            italic: f.italic,
            stretch: NORMAL_WIDTH,
            path: f.file.path.clone(),
            index: f.file.index,
        }));
    }
    faces.extend(upright_twins(index, &faces));
    faces.extend(width_twins(index));
    let fallbacks = [Fallback::Latin]
        .into_iter()
        .chain(CjkScript::ALL.map(Fallback::Cjk))
        .chain([Fallback::Notdef]);
    let weights = fallback_weights(index);
    faces.extend(emoji_weights(index, &weights));
    for which in fallbacks {
        for &weight in &weights {
            for italic in [false, true] {
                if let Some(face) = resolve_fallback(index, which, weight, italic)
                    && let Some(file) = face.file
                {
                    faces.push(ManagedFace {
                        alias: face.alias,
                        weight,
                        italic,
                        stretch: NORMAL_WIDTH,
                        path: file.path,
                        index: file.index,
                    });
                }
            }
        }
    }
    faces
}

/// Upright copies of the faces of every family that has no upright face: the bundled and downloaded ones among
/// `managed`, and the installed ones (registered under their own names, beside the scan's italic records).
fn upright_twins(index: &FontIndex, managed: &[ManagedFace]) -> Vec<ManagedFace> {
    let italic_only = |alias: &str| {
        managed
            .iter()
            .filter(|f| f.alias == alias)
            .all(|f| f.italic)
    };
    let mut twins: Vec<ManagedFace> = managed
        .iter()
        .filter(|f| f.italic && italic_only(&f.alias))
        .map(|f| ManagedFace {
            italic: false,
            ..f.clone()
        })
        .collect();
    for family in index.installed() {
        if family.faces.iter().all(|f| f.italic) {
            twins.extend(family.faces.iter().map(|f| ManagedFace {
                alias: family.alias.clone(),
                weight: f.weight,
                italic: false,
                stretch: f.stretch,
                path: f.path.clone(),
                index: f.index,
            }));
        }
    }
    twins
}

/// 100 to 900, and every other weight an installed or downloaded face has (1–1000), sorted.
fn fallback_weights(index: &FontIndex) -> Vec<u16> {
    let installed = index
        .installed()
        .iter()
        .flat_map(|f| &f.faces)
        .map(|f| f.weight);
    let downloaded = index
        .downloads()
        .iter()
        .flat_map(|d| &d.faces)
        .map(|f| f.weight);
    (100..=900)
        .step_by(100)
        .chain(
            installed
                .chain(downloaded)
                .filter(|w| (1..=1000).contains(w)),
        )
        .collect::<BTreeSet<u16>>()
        .into_iter()
        .collect()
}

/// fontdb's width for an OS/2 width class (5, normal, for anything out of 1–9).
pub(crate) fn width_of(class: u16) -> Stretch {
    match class {
        1 => Stretch::UltraCondensed,
        2 => Stretch::ExtraCondensed,
        3 => Stretch::Condensed,
        4 => Stretch::SemiCondensed,
        6 => Stretch::SemiExpanded,
        7 => Stretch::Expanded,
        8 => Stretch::ExtraExpanded,
        9 => Stretch::UltraExpanded,
        _ => Stretch::Normal,
    }
}

/// Whether a face in a database is one of ours: a `kl:` alias (never an installed family's name), or an installed
/// family's upright twin (its PostScript name says so).
pub(super) fn is_managed(face: &FaceInfo) -> bool {
    face.post_script_name.starts_with(TWIN)
        || face
            .families
            .first()
            .is_some_and(|(name, _)| name.starts_with("kl:"))
}

/// The PostScript name prefix of an installed family's upright twin.
const TWIN: &str = "kl:twin:";

/// Registers the engine's managed faces (`index()`) in `db`, in place of any registered before.
pub(crate) fn add_managed(db: &mut Database) {
    let stale: Vec<ID> = db.faces().filter(|f| is_managed(f)).map(|f| f.id).collect();
    for id in stale {
        db.remove_face(id);
    }
    push_managed(db, &index());
}

/// Pushes `index`'s managed faces into `db` and pins the generic sans-serif to the Latin fallback when it exists. Face
/// records only: no file is read until a face is used, and then it is memory-mapped once per database.
pub(crate) fn push_managed(db: &mut Database, index: &FontIndex) {
    let faces = managed_faces(index);
    if faces.iter().any(|f| f.alias == LATIN) {
        db.set_sans_serif_family(LATIN);
    }
    for f in faces {
        let twin = if f.alias.starts_with("kl:") { "" } else { TWIN };
        let post_script_name = format!(
            "{twin}{}-{}{}-{}",
            f.alias,
            f.weight,
            if f.italic { "i" } else { "" },
            f.stretch
        );
        db.push_face_info(FaceInfo {
            id: ID::dummy(),
            source: Source::File(f.path),
            index: f.index,
            families: vec![(f.alias, Language::English_UnitedStates)],
            post_script_name,
            style: if f.italic {
                Style::Italic
            } else {
                Style::Normal
            },
            weight: Weight(f.weight),
            stretch: width_of(f.stretch),
            monospaced: false,
        });
    }
}
