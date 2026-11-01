//! The fonts for the page: the bundled families and their faces' bytes (a title typed on the canvas, and measured for
//! its handles, is drawn in the very file the engine draws it with), and the Font menu's search, browse and Google
//! Fonts downloads over `klipolo_app::fonts`. Only files the font index holds are ever read: a family and weight name
//! one of them, never a path.

use std::sync::Arc;

use crate::project::ProjectSessionState;
use klipolo_app::export::ExportState;
use klipolo_app::fonts::cjk::{CjkFonts, cjk_fonts};
use klipolo_app::fonts::downloads::{self, Downloads, Host};
use klipolo_app::fonts::google::{self, GoogleFonts};
use klipolo_app::fonts::{Fonts, MissingAnswer, face_bytes};
use klipolo_fonts::browse::{Browse, BrowseAsk, browse, browse_rows, row_for};
use klipolo_fonts::bundled::{BUNDLED, BundledFace, bundled_dir, fallback_only};
use klipolo_fonts::google::CatalogueInfo;
use klipolo_fonts::search::{FontRow, SearchFilter};
use klipolo_fonts::store::{DownloadedFamily, DownloadedId};
use klipolo_mcp::connection::support_dir;
use klipolo_model::coded::{CodedError, Said};
use klipolo_model::font::FontRef;
use klipolo_model::text_language::CjkScript;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

/// One face of a bundled family.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BundledFont {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

/// Every face of the families a text can use, in the menu's order (Adobe NotDef, a fallback only, is left out).
#[tauri::command]
pub fn fonts_bundled() -> Vec<BundledFont> {
    BUNDLED
        .iter()
        .filter(|face| !fallback_only(face.family))
        .map(|face| BundledFont {
            family: face.family.into(),
            weight: face.weight,
            italic: face.italic,
        })
        .collect()
}

/// The face of `family` nearest `weight` (the same style first), as CSS and the engine pick one from a family's faces.
pub(crate) fn nearest(family: &str, weight: u16, italic: bool) -> Option<&'static BundledFace> {
    BUNDLED
        .iter()
        .filter(|face| face.family == family && !fallback_only(face.family))
        .min_by_key(|face| (face.italic != italic, face.weight.abs_diff(weight)))
}

/// The bytes of `family`'s face nearest `weight` and `italic`, from the bundled folder.
#[tauri::command(async)]
pub fn fonts_face(
    family: String,
    weight: u16,
    italic: bool,
) -> Result<tauri::ipc::Response, String> {
    let face = nearest(&family, weight, italic).ok_or("That font isn't one of Klipolo's.")?;
    let dir = bundled_dir().ok_or("Klipolo's fonts aren't installed.")?;
    std::fs::read(dir.join(face.file))
        .map(tauri::ipc::Response::new)
        .map_err(|error| {
            tracing::warn!(%error, file = face.file, "a bundled font couldn't be read");
            "Klipolo couldn't read that font. Try again, or reinstall Klipolo.".to_string()
        })
}

/// The app as the downloads' host: Google Fonts' setting file in the support folder, and events to the page.
#[derive(Clone)]
pub(crate) struct AppHost(pub AppHandle);

impl Host for AppHost {
    fn google(&self) -> GoogleFonts {
        support_dir()
            .map(|dir| google::load(&dir))
            .unwrap_or_default()
    }

    fn set_google(&self, change: &dyn Fn(&mut GoogleFonts)) -> Result<GoogleFonts, CodedError> {
        let dir = support_dir().ok_or_else(klipolo_fonts::english::save_failed)?;
        let mut now = google::load(&dir);
        change(&mut now);
        google::save(&dir, &now).map_err(|error| {
            tracing::warn!(%error, "fonts: Google Fonts' setting couldn't be saved");
            klipolo_fonts::english::save_failed()
        })?;
        Ok(now)
    }

    fn progress(&self, family: &str, done: u64, total: u64) {
        let _ = self.0.emit(
            "fonts-download",
            json!({ "family": family, "done": done, "total": total }),
        );
    }

    fn changed(&self, family: &str) {
        let _ = self.0.emit("fonts-changed", json!({ "family": family }));
    }
}

/// The families matching `query`, best first: the bundled set, this computer's and Google's (none needing a download
/// while Google Fonts is off).
#[tauri::command]
pub async fn fonts_search(
    app: AppHandle,
    fonts: State<'_, Fonts>,
    query: String,
    mut filter: SearchFilter,
) -> Result<Vec<FontRow>, Said> {
    filter.google_off = !AppHost(app).google().enabled;
    Ok(fonts.search_after_scan(&query, filter).await)
}

/// The menu's browse (an empty query): its sections' counts, the 12 bundled families first; the rows come by window.
#[tauri::command]
pub async fn fonts_browse(
    app: AppHandle,
    fonts: State<'_, Fonts>,
    mut ask: BrowseAsk,
) -> Result<Browse, Said> {
    ask.filter.google_off = !AppHost(app).google().enabled;
    fonts.after_scan().await;
    Ok(browse(&fonts.index(), &ask))
}

/// The browse's rows `from..to`, at most 200 at a time.
#[tauri::command]
pub async fn fonts_browse_rows(
    app: AppHandle,
    fonts: State<'_, Fonts>,
    mut ask: BrowseAsk,
    from: u32,
    to: u32,
) -> Result<Vec<FontRow>, Said> {
    ask.filter.google_off = !AppHost(app).google().enabled;
    fonts.after_scan().await;
    Ok(browse_rows(
        &fonts.index(),
        &ask,
        from,
        to.min(from.saturating_add(200)),
    ))
}

/// The row of exactly `font` (else a family of its name), for the menu's button and its Weight menu.
#[tauri::command]
pub async fn fonts_row(
    fonts: State<'_, Fonts>,
    font: FontRef,
    weight: u16,
    italic: bool,
) -> Result<Option<FontRow>, Said> {
    fonts.after_scan().await;
    Ok(row_for(&fonts.index(), &font, (weight, italic)))
}

/// The bytes of the face `font` draws with at `weight` and `italic` (bundled, downloaded or this computer's), as the
/// engine resolves it: the canvas draws a title in the very file the engine does.
#[tauri::command]
pub async fn fonts_face_of(
    fonts: State<'_, Fonts>,
    font: FontRef,
    weight: u16,
    italic: bool,
) -> Result<tauri::ipc::Response, Said> {
    let index = fonts.index();
    let bytes =
        tauri::async_runtime::spawn_blocking(move || face_bytes(&index, &font, weight, italic))
            .await
            .map_err(|_| klipolo_fonts::english::missing(""))??;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Google Fonts' switch and the person's one consent, as saved.
#[tauri::command(async)]
pub fn fonts_google(app: AppHandle) -> GoogleFonts {
    AppHost(app).google()
}

/// The person confirmed "Download from Google Fonts?" once.
#[tauri::command(async)]
pub fn fonts_consent(app: AppHandle) -> Result<GoogleFonts, Said> {
    Ok(downloads::consent(&AppHost(app))?)
}

/// Settings' "Google Fonts" switch; off stops the downloads running now.
#[tauri::command(async)]
pub fn fonts_set_google(
    app: AppHandle,
    downloads: State<'_, Downloads>,
    enabled: bool,
) -> Result<GoogleFonts, Said> {
    Ok(downloads::set_enabled(&AppHost(app), &downloads, enabled)?)
}

/// Downloads a Google family for a text at `weight` and `italic` (`version`: a project's own, else the catalogue's).
/// Progress comes as `fonts-download`, the end as `fonts-changed`. The family's `FontRef`.
#[tauri::command]
pub async fn fonts_download(
    app: AppHandle,
    fonts: State<'_, Fonts>,
    downloads: State<'_, Downloads>,
    family: String,
    version: Option<String>,
    weight: u16,
    italic: bool,
) -> Result<FontRef, Said> {
    let host: Arc<dyn Host> = Arc::new(AppHost(app));
    Ok(downloads::download(
        host,
        &fonts,
        &downloads,
        &family,
        version.as_deref(),
        weight,
        italic,
    )
    .await?)
}

/// Stops `family`'s download (its finished files stay).
#[tauri::command]
pub fn fonts_cancel(downloads: State<'_, Downloads>, family: String) {
    downloads.cancel(&family);
}

/// The online preview of `family`'s name, as a binary response: only after the consent, never while off.
#[tauri::command]
pub async fn fonts_preview(
    app: AppHandle,
    fonts: State<'_, Fonts>,
    downloads: State<'_, Downloads>,
    family: String,
) -> Result<tauri::ipc::Response, Said> {
    let bytes = downloads::preview(&AppHost(app), &fonts, &downloads, &family).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// The downloaded families, with their sizes (Settings, Fonts).
#[tauri::command(async)]
pub fn fonts_downloaded(downloads: State<'_, Downloads>) -> Vec<DownloadedFamily> {
    downloads::list(&downloads)
}

/// Removes a downloaded family by its id; refused while an export runs, as it draws with every download.
#[tauri::command]
pub async fn fonts_remove(
    app: AppHandle,
    fonts: State<'_, Fonts>,
    downloads: State<'_, Downloads>,
    id: DownloadedId,
) -> Result<(), Said> {
    let exports = app.clone();
    let exporting = move || {
        exports
            .try_state::<ExportState>()
            .is_some_and(|export| export.is_running())
    };
    Ok(downloads::remove(&AppHost(app), &fonts, &downloads, id, &exporting).await?)
}

/// Shows the downloaded fonts' folder in Finder (Settings, Fonts).
#[tauri::command(async)]
pub fn fonts_reveal(downloads: State<'_, Downloads>) {
    if let Some(store) = downloads.store() {
        let _ = super::export::opener(true, &[store.root().to_path_buf()]).spawn();
    }
}

/// Which Google catalogue the app ships: its date, or that it is the sample of a test build.
#[tauri::command]
pub async fn fonts_catalogue() -> CatalogueInfo {
    tauri::async_runtime::spawn_blocking(|| klipolo_fonts::google::catalogue().info())
        .await
        .unwrap_or_else(|_| CatalogueInfo {
            date: String::new(),
            sample: true,
            families: 0,
        })
}

/// The fonts the open project `project_id` uses that aren't here, once this computer's are read (else `pending`): the
/// banner's, the badges' and Replace fonts' one list. None for another project than the open one.
#[tauri::command]
pub async fn fonts_missing(
    fonts: State<'_, Fonts>,
    session: State<'_, ProjectSessionState>,
    project_id: String,
) -> Result<MissingAnswer, Said> {
    let open = session
        .subscribe()
        .borrow()
        .as_ref()
        .filter(|feed| feed.project_id == project_id)
        .map(|feed| (feed.project.clone(), feed.text_language.clone()));
    let Some((project, text_language)) = open else {
        return Ok(MissingAnswer::Ready { fonts: Vec::new() });
    };
    Ok(fonts.missing_after_scan(&project, &text_language).await)
}

/// Noto Sans SC, TC, JP and KR: their faces' sizes, and which are here (Settings).
#[tauri::command]
pub async fn fonts_cjk(fonts: State<'_, Fonts>) -> Result<Vec<CjkFonts>, Said> {
    fonts.catalogue_ready().await;
    Ok(cjk_fonts(&fonts.index()))
}

/// The Noto Sans of `script` at `weight`, Chinese, Japanese or Korean text's fallback.
#[tauri::command]
pub async fn fonts_cjk_download(
    app: AppHandle,
    fonts: State<'_, Fonts>,
    downloads: State<'_, Downloads>,
    script: CjkScript,
    weight: u16,
) -> Result<FontRef, Said> {
    let host: Arc<dyn Host> = Arc::new(AppHost(app));
    Ok(downloads::download_cjk(host, &fonts, &downloads, script, weight).await?)
}

/// The fonts' start: Google's catalogue decoded and the downloads registered with the engine, each on a blocking
/// thread, never the main one.
pub(crate) fn start(app: &tauri::App) {
    let data = app.path().app_data_dir().ok();
    let downloads = Downloads::open(data.as_deref());
    app.manage(Fonts::new());
    app.manage(downloads.clone());
    let starting = app.handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        klipolo_app::fonts::downloads::start(&starting.state::<Fonts>(), &downloads);
    });
    // Chat's and other apps' calls name fonts from the same index, without Google's while Google Fonts is off.
    let naming = app.handle().clone();
    klipolo_app::fonts::install_names(klipolo_chat::FontNames::new(Arc::new(
        move |name: &str, weight, italic| {
            let fonts = naming.state::<Fonts>();
            let google = AppHost(naming.clone()).google().enabled;
            klipolo_app::fonts::named(&fonts.index(), google, name, weight, italic)
        },
    )));
    // This computer's fonts arrive with the engine's one scan: an open font menu asks again then.
    let scanning = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let mut scanned = klipolo_engine::render::fonts::subscribe_scanned();
        if scanned.wait_for(|done| *done).await.is_ok() {
            let _ = scanning.emit("fonts-scanned", ());
        }
    });
    let decoding = app.handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        decoding
            .state::<Fonts>()
            .publish_catalogue(klipolo_fonts::google::shipped());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_face_is_the_nearest_weight_of_the_family() {
        assert_eq!(nearest("Inter", 700, false).unwrap().weight, 700);
        assert_eq!(nearest("Inter", 800, false).unwrap().weight, 700);
        assert_eq!(nearest("Montserrat", 700, false).unwrap().weight, 900);
        assert!(nearest("Adobe NotDef", 400, false).is_none());
        assert!(nearest("../../etc/passwd", 400, false).is_none());
        let families: Vec<String> = fonts_bundled().into_iter().map(|f| f.family).collect();
        assert!(
            families.contains(&"Inter".to_string())
                && !families.contains(&"Adobe NotDef".to_string())
        );
    }
}
