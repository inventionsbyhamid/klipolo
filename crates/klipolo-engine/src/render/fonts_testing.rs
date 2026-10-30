//! Tests' view of the engine's fonts (doc-hidden): the face cosmic-text really shapes a text with, in a font system of
//! the engine's managed faces only (no system scan, so no installed face can answer for one of ours).

use std::path::PathBuf;

use glyphon::fontdb::{self, Database, FaceInfo, ID, Language, Source};
use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, Weight};
use klipolo_fonts::bundled::bundled_dir;
use klipolo_fonts::index::{FontIndex, InstalledFamily};

use super::installed::{group_installed, published_of};
use super::recorded::ScanRecord;
use super::{FaceChoice, index, push_managed};
use crate::render::font_fallback::KlipoloFallback;

/// The bundled set (`bundled_dir()`: the repository's `assets/fonts` in a debug build), as a value.
#[doc(hidden)]
pub fn test_index() -> FontIndex {
    FontIndex::shipped(bundled_dir())
}

/// A font system of the engine's managed faces (`index()`: bundled, registered downloads, fallback aliases) and
/// nothing else, with Klipolo's fallback chain.
fn managed_system() -> FontSystem {
    let mut db = Database::new();
    push_managed(&mut db, &index());
    FontSystem::new_with_locale_and_db_and_fallback("en-US".into(), db, KlipoloFallback)
}

/// `managed_system` plus one stand-in system face, "Test Thai": Adobe NotDef's file (a glyph for every character),
/// registered upright at 400 under another name, as a system's upright script face is.
fn with_a_system_face() -> FontSystem {
    let mut db = Database::new();
    push_managed(&mut db, &index());
    let notdef = bundled_dir()
        .expect("the bundled folder")
        .join("AdobeNotDef/AND-Regular.ttf");
    db.push_face_info(FaceInfo {
        id: ID::dummy(),
        source: Source::File(notdef),
        index: 0,
        families: vec![("Test Thai".into(), Language::English_UnitedStates)],
        post_script_name: "TestThai-Regular".into(),
        style: fontdb::Style::Normal,
        weight: fontdb::Weight(400),
        stretch: fontdb::Stretch::Normal,
        monospaced: false,
    });
    FontSystem::new_with_locale_and_db_and_fallback("en-US".into(), db, KlipoloFallback)
}

/// The family and weight of the face that draws the first glyph of `text`, asked as `family` at `weight` and `italic`.
fn first_glyph(family: &str, weight: u16, italic: bool, text: &str) -> (String, u16) {
    first_glyph_in(managed_system(), family, (weight, italic, 5), text)
}

/// `first_glyph` in `system`, asked as `family` at `(weight, italic, width class)`.
fn first_glyph_in(
    mut system: FontSystem,
    family: &str,
    (weight, italic, stretch): (u16, bool, u16),
    text: &str,
) -> (String, u16) {
    let mut buffer = Buffer::new(&mut system, Metrics::new(32.0, 40.0));
    buffer.set_size(&mut system, Some(4000.0), None);
    let attrs = Attrs::new()
        .family(Family::Name(family))
        .weight(Weight(weight))
        .stretch(super::width_of(stretch))
        .style(if italic { Style::Italic } else { Style::Normal });
    buffer.set_text(&mut system, text, &attrs, Shaping::Advanced);
    buffer.shape_until_scroll(&mut system, false);
    let glyph = buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter())
        .next()
        .map(|g| g.font_id)
        .expect("the text has a glyph");
    let face = system.db().face(glyph).expect("its face");
    (face.families[0].0.clone(), face.weight.0)
}

/// The face cosmic-text shapes `text` with when asked exactly as `choice` says.
#[doc(hidden)]
pub fn test_shape_face(choice: &FaceChoice, text: &str) -> (String, u16) {
    first_glyph_in(
        managed_system(),
        &choice.family,
        (choice.weight, choice.italic, choice.stretch),
        text,
    )
}

/// The face that draws `text`'s first glyph when it is asked as `primary` at `weight` and `italic`: the primary's own,
/// or the fallback that answers for what it lacks.
#[doc(hidden)]
pub fn test_fallback_face(primary: &str, text: &str, weight: u16, italic: bool) -> (String, u16) {
    first_glyph(primary, weight, italic, text)
}

/// `test_fallback_face` in a font system that also has one upright system face for every script ("Test Thai"): whether
/// a text asked as `primary` at `italic` reaches the system's upright script faces.
#[doc(hidden)]
pub fn test_system_fallback_face(
    primary: &str,
    text: &str,
    weight: u16,
    italic: bool,
) -> (String, u16) {
    first_glyph_in(with_a_system_face(), primary, (weight, italic, 5), text)
}

/// The families the scan would publish for a database of `files` (each loaded as fontdb loads a system font).
#[doc(hidden)]
pub fn test_group_installed(files: &[PathBuf]) -> Vec<InstalledFamily> {
    let mut db = Database::new();
    for file in files {
        db.load_font_file(file).expect("a font file");
    }
    group_installed(&db)
}

/// The record an app whose scan found the fonts in `folder` (and nothing else) writes into the export job folder `dir`
/// (`record_scan`), made from that folder as the scan makes it, without touching this process's fonts.
#[doc(hidden)]
pub fn test_record_scan_of(folder: &std::path::Path, dir: &std::path::Path) {
    let mut db = Database::new();
    db.load_fonts_dir(folder);
    let published = published_of(&mut db);
    ScanRecord::of("en-US", &db, published)
        .write(dir)
        .expect("the record is written");
}

/// The face that draws `text`'s first glyph asked as `family` at `weight` and `italic`, in a font system made from this
/// process's font template (the compositor's): the first one builds the template, by scan or from a job's record.
#[doc(hidden)]
pub fn test_template_face(family: &str, weight: u16, italic: bool, text: &str) -> (String, u16) {
    first_glyph_in(
        crate::vendor::cap::rendering::layers::new_font_system(),
        family,
        (weight, italic, 5),
        text,
    )
}

/// One installed face for `test_installed_draw`: its family, its file, its weight and its width (OS/2 class, 5 normal).
#[doc(hidden)]
pub type TestFace<'a> = (&'a str, PathBuf, u16, u16);

/// What draws "Hello" for `family` at `weight` on a computer whose only installed faces are `faces`: the family's face
/// as the scan records it, resolved through an index of that scan (as the engine's own is), and shaped as the drawer
/// asks for it. The managed faces are there too, so a face cosmic-text can't find falls to the Latin fallback.
#[doc(hidden)]
pub fn test_installed_draw(
    faces: &[TestFace<'_>],
    family: &str,
    weight: u16,
) -> (FaceChoice, String) {
    let font = klipolo_model::font::FontRef {
        family: family.into(),
        source: klipolo_model::font::FontSource::Installed,
    };
    let (choice, drawn, _) = test_installed_shape(faces, &font, weight, "Hello");
    (choice, drawn)
}

/// `test_installed_draw` for any `font` and `text`: the face chosen, and the family and weight of the face that draws
/// the text's first glyph (a fallback when the font lacks it).
#[doc(hidden)]
pub fn test_installed_shape(
    faces: &[TestFace<'_>],
    font: &klipolo_model::font::FontRef,
    weight: u16,
    text: &str,
) -> (FaceChoice, String, u16) {
    let mut db = Database::new();
    for (name, file, w, stretch) in faces {
        db.push_face_info(FaceInfo {
            id: ID::dummy(),
            source: Source::File(file.clone()),
            index: 0,
            families: vec![((*name).into(), Language::English_UnitedStates)],
            post_script_name: format!("{name}-{w}-{stretch}"),
            style: fontdb::Style::Normal,
            weight: fontdb::Weight(*w),
            stretch: super::width_of(*stretch),
            monospaced: false,
        });
    }
    let index = test_index().with_installed(group_installed(&db));
    push_managed(&mut db, &index);
    let choice = super::family_for(&index, font, weight, false);
    let system =
        FontSystem::new_with_locale_and_db_and_fallback("en-US".into(), db, KlipoloFallback);
    let asked = (choice.weight, choice.italic, choice.stretch);
    let (drawn, drawn_weight) = first_glyph_in(system, &choice.family, asked, text);
    (choice, drawn, drawn_weight)
}
