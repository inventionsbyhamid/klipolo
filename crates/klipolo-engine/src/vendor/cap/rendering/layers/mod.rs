// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/rendering/src/layers/mod.rs (excerpt: 30–80 new_font_system)
// Copyright (c) 2023-present Cap Software, Inc. Used by Klipolo under the AGPL-3.0.
// Klipolo changes: Cap's readiness phase timing (Phase::start/finish, measure) removed from new_font_system; the Windows and Linux generic-family pins removed; the module list is Klipolo's (background, display, image, text) and nothing is glob re-exported; the font template is a module static and `fonts_scanned` and `fallback_font_system` were added (the preview never waits for the font scan); both font databases get Klipolo's managed faces (`render::fonts::add_managed`: the bundled fonts, downloads and the fallback aliases, registered from their files), the template is versioned by `render::fonts::generation()` (a font registered later reaches the next font system), the scan also loads the fonts a font manager activated (`klipolo_platform::fonts::activated_font_files`) and publishes this computer's families with their coverage (`render::fonts::publish_installed`), every font system uses Klipolo's fallback chain (`render::font_fallback::KlipoloFallback`), the generic sans-serif is pinned to the Latin fallback (Inter), and `fallback_font_system` loads only the managed faces (Helvetica only for a broken install); `template_generation`, `new_font_system_at` and the doc-hidden `scan_also_for_tests` were added (`render::shared_fonts`, and the export test of a family only the scan finds); an export worker given its app's scan builds the template from that record instead of scanning, and `with_scanned_template` lends the app's template to record it (`render::fonts::record_scan`).

//! Vendored Cap layers.

pub mod background;
pub mod display;
pub mod image;
pub mod text;

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError, RwLock};

use crate::render::font_fallback::KlipoloFallback;
use crate::render::fonts;

/// Building a `glyphon::FontSystem` scans and parses every installed system font,
/// which costs hundreds of milliseconds to over a second on macOS. cosmic-text
/// explicitly documents that it should be created once and shared. We previously
/// built three of them per `RendererLayers` (text, captions, keyboard) and a fresh
/// `RendererLayers` per editor/screenshot instance, so every open paid that scan
/// several times over.
///
/// Instead, scan the system fonts a single time per process, then cheaply clone the
/// resulting font database (memory-mapped faces are reference counted) for each new
/// `FontSystem`.
///
/// Klipolo: the database holds the managed faces of `fonts::generation()` too; a newer generation (a font registered or
/// removed) re-registers them under the write lock before the next clone.
static FONT_TEMPLATE: OnceLock<(String, RwLock<(u64, glyphon::fontdb::Database)>)> =
    OnceLock::new();

pub(crate) fn new_font_system() -> glyphon::FontSystem {
    new_font_system_at().0
}

/// Klipolo: `new_font_system`, and the fonts' generation its database is at (`render::shared_fonts` keeps it, so a
/// change made while it was built is never missed).
pub(crate) fn new_font_system_at() -> (glyphon::FontSystem, u64) {
    let (locale, template) = FONT_TEMPLATE.get_or_init(|| {
        // Klipolo: an export worker given its app's scan builds the database from that record: no folder
        // walked, no face read for its coverage, and the same faces, families and forbidden fallbacks as the app's.
        if let Some(recorded) = fonts::take_recorded_scan() {
            let (locale, mut db) = recorded.into_database();
            let generation = fonts::generation();
            fonts::add_managed(&mut db);
            return (locale, RwLock::new((generation, db)));
        }
        let font_system = glyphon::FontSystem::new();
        let mut db = font_system.db().clone();
        // Pin the generic families to the fonts the editor webview resolves
        // them to. fontdb's stock defaults (e.g. "Arial") often don't match
        // any installed face, in which case cosmic-text silently shapes with
        // an arbitrary fallback font — and canvas overlays measured in the
        // webview no longer match what the renderer draws.
        #[cfg(target_os = "macos")]
        {
            // WKWebView: sans-serif → Helvetica, serif → Times, monospace →
            // Courier. Klipolo: sans-serif is the Latin fallback once the managed faces are in (below).
            db.set_sans_serif_family("Helvetica");
            db.set_serif_family("Times New Roman");
            db.set_monospace_family("Courier New");
        }
        // Klipolo: fonts a font manager activated outside the system's folders join the scan (macOS; none elsewhere),
        // then this computer's families are published once: the engine's index, the forbidden fallback list and the
        // app's font menu all take them from here.
        let activated = klipolo_platform::fonts::activated_font_files();
        // Klipolo: a test's folder of fonts the scan finds as installed (`scan_also_for_tests`), after the system's.
        for dir in SCAN_ALSO
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            db.load_fonts_dir(dir);
        }
        let unread = activated
            .iter()
            .filter(|path| db.load_font_file(path).is_err())
            .count();
        if unread > 0 {
            // Once, as a count: a font manager's `.dfont` or Type 1 files are formats fontdb can't read.
            tracing::warn!(
                unread,
                of = activated.len(),
                "activated fonts the scan can't read"
            );
        }
        fonts::publish_installed(&mut db);
        let generation = fonts::generation();
        fonts::add_managed(&mut db);
        (
            font_system.locale().to_string(),
            RwLock::new((generation, db)),
        )
    });
    let now = fonts::generation();
    let (generation, db) = {
        let held = template.read().unwrap_or_else(PoisonError::into_inner);
        if held.0 == now {
            (held.0, held.1.clone())
        } else {
            drop(held);
            let mut held = template.write().unwrap_or_else(PoisonError::into_inner);
            if held.0 != now {
                fonts::add_managed(&mut held.1);
                held.0 = now;
            }
            (held.0, held.1.clone())
        }
    };
    let system = glyphon::FontSystem::new_with_locale_and_db_and_fallback(
        locale.clone(),
        db,
        KlipoloFallback,
    );
    (system, generation)
}

/// Klipolo: folders the scan also reads, after the system's, as it reads the files a font manager activated.
static SCAN_ALSO: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Klipolo, tests: the system font scan also reads `dir`, as if its fonts were installed. Only before the scan (once a
/// process): a test that needs it has its own binary (`tests/export_fonts.rs`).
#[doc(hidden)]
pub fn scan_also_for_tests(dir: &std::path::Path) {
    SCAN_ALSO
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(dir.to_path_buf());
}

/// Klipolo: the fonts' generation the next font system is made at (`render::fonts::generation`): the template takes a
/// newer one's managed faces before its next clone, so a font system made at an older one is out of date.
pub(crate) fn template_generation() -> u64 {
    fonts::generation()
}

/// Klipolo: `f` of the scanned template's locale and database (its managed faces among them), once the scan is done; None
/// before. An export job records the app's scan from it (`render::fonts::record_scan`).
pub(crate) fn with_scanned_template<T>(
    f: impl FnOnce(&str, &glyphon::fontdb::Database) -> T,
) -> Option<T> {
    let (locale, template) = FONT_TEMPLATE.get()?;
    let held = template.read().unwrap_or_else(PoisonError::into_inner);
    Some(f(locale, &held.1))
}

/// Klipolo: whether the system font scan is done (`new_font_system` returns without waiting).
pub(crate) fn fonts_scanned() -> bool {
    FONT_TEMPLATE.get().is_some()
}

/// Klipolo: a font system with only the managed faces (the bundled fonts, downloads and fallback aliases), made in
/// milliseconds, for text drawn while the scan of every installed font still runs (callers swap in `new_font_system`
/// once `fonts_scanned`). Its sans-serif is the Latin fallback; a broken install (no bundled folder) gets the system
/// sans-serif on macOS, as before.
pub(crate) fn fallback_font_system() -> glyphon::FontSystem {
    let mut db = glyphon::fontdb::Database::new();
    fonts::add_managed(&mut db);
    #[cfg(target_os = "macos")]
    if db.is_empty() {
        if let Err(error) = db.load_font_file("/System/Library/Fonts/Helvetica.ttc") {
            tracing::warn!(%error, "system sans-serif font");
        }
        db.set_sans_serif_family("Helvetica");
    }
    glyphon::FontSystem::new_with_locale_and_db_and_fallback("en-US".into(), db, KlipoloFallback)
}
