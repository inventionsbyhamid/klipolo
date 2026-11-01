use std::sync::{Arc, Mutex};

use klipolo_fonts::english::{google_off, needs_consent, unknown_google, version_gone};
use klipolo_fonts::index::FontIndex;
use klipolo_fonts::search::{RowState, SearchFilter};
use klipolo_model::coded::CodedError;
use klipolo_model::font::{FontRef, FontSource};

use super::downloads::{self, Downloads, Host};
use super::google::GoogleFonts;
use super::{Fonts, face_bytes};

/// The setting in memory, and the events the page would hear.
#[derive(Default)]
struct FakeHost {
    google: Mutex<GoogleFonts>,
    changed: Mutex<Vec<String>>,
}

impl FakeHost {
    fn with(google: GoogleFonts) -> Arc<FakeHost> {
        Arc::new(FakeHost {
            google: Mutex::new(google),
            ..FakeHost::default()
        })
    }
}

impl Host for FakeHost {
    fn google(&self) -> GoogleFonts {
        *self.google.lock().unwrap()
    }
    fn set_google(&self, change: &dyn Fn(&mut GoogleFonts)) -> Result<GoogleFonts, CodedError> {
        let mut g = self.google.lock().unwrap();
        change(&mut g);
        Ok(*g)
    }
    fn progress(&self, _family: &str, _done: u64, _total: u64) {}
    fn changed(&self, family: &str) {
        self.changed.lock().unwrap().push(family.into());
    }
}

/// The bundled set with Google's catalogue, everything in: nothing waits.
fn fonts() -> Fonts {
    let fonts = Fonts::for_tests(FontIndex::shipped(klipolo_fonts::bundled::bundled_dir()));
    fonts.publish_catalogue(klipolo_fonts::google::shipped());
    fonts.downloads_started();
    fonts
}

fn on_and_consented() -> GoogleFonts {
    GoogleFonts {
        enabled: true,
        consented: true,
    }
}

#[tokio::test]
async fn nothing_downloads_before_the_consent_or_while_google_fonts_is_off() {
    let fonts = fonts();
    let dir = tempfile::tempdir().unwrap();
    let downloads = Downloads::open(Some(dir.path()));
    let fresh = FakeHost::with(GoogleFonts::default());
    let got =
        downloads::download(fresh.clone(), &fonts, &downloads, "Lato", None, 400, false).await;
    assert_eq!(got, Err(needs_consent()));
    let off = FakeHost::with(GoogleFonts {
        enabled: false,
        consented: true,
    });
    let got = downloads::download(off.clone(), &fonts, &downloads, "Lato", None, 400, false).await;
    assert_eq!(got, Err(google_off()));
    assert_eq!(
        downloads::preview(&*off, &fonts, &downloads, "Lato").await,
        Err(google_off())
    );
    assert_eq!(downloads.asked(), 0, "no request went out");
    assert!(fresh.changed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_family_google_doesnt_have_or_a_version_it_no_longer_serves_is_refused_by_name() {
    let fonts = fonts();
    let dir = tempfile::tempdir().unwrap();
    let downloads = Downloads::open(Some(dir.path()));
    let host = FakeHost::with(on_and_consented());
    let got = downloads::download(
        host.clone(),
        &fonts,
        &downloads,
        "Nowhere Sans",
        None,
        400,
        false,
    )
    .await;
    assert_eq!(got, Err(unknown_google("Nowhere Sans")));
    let lato = klipolo_fonts::google::catalogue().find("lato").unwrap();
    let got = downloads::download(host, &fonts, &downloads, "Lato", Some("v0"), 400, false).await;
    assert_eq!(got, Err(version_gone(&lato.family, "v0")));
    assert_eq!(downloads.asked(), 0);
}

#[test]
fn the_consent_is_saved_and_the_switch_turned_off_stops_downloads() {
    let host = FakeHost::with(GoogleFonts::default());
    assert_eq!(
        downloads::consent(&*host),
        Ok(GoogleFonts {
            enabled: true,
            consented: true
        })
    );
    let downloads = Downloads::open(None);
    assert_eq!(
        downloads::set_enabled(&*host, &downloads, false),
        Ok(GoogleFonts {
            enabled: false,
            consented: true
        })
    );
    assert!(!host.google().enabled);
}

#[tokio::test]
async fn the_menu_searches_the_bundled_set_and_google_s_catalogue() {
    let fonts = fonts();
    let rows = fonts
        .search_after_scan("lato", SearchFilter::default())
        .await;
    let lato = rows
        .iter()
        .find(|r| r.family == "Lato")
        .expect("Google's Lato is listed");
    assert_eq!(lato.state, RowState::Download);
    assert!(matches!(lato.source, FontSource::Google { .. }));
    let inter = fonts
        .search_after_scan("inter", SearchFilter::default())
        .await;
    assert_eq!(inter[0].family, "Inter");
    assert_eq!(inter[0].state, RowState::Bundled);
}

#[test]
fn a_face_s_bytes_are_the_file_the_engine_draws_with() {
    let index = FontIndex::shipped(klipolo_fonts::bundled::bundled_dir());
    let bytes = face_bytes(&index, &FontRef::bundled("Anton"), 400, false).unwrap();
    let dir = klipolo_fonts::bundled::bundled_dir().unwrap();
    assert_eq!(
        bytes,
        std::fs::read(dir.join("Anton/Anton-Regular.ttf")).unwrap()
    );
}

#[tokio::test]
async fn a_download_is_never_removed_under_a_running_export_nor_one_that_isnt_the_store_s() {
    let fonts = fonts();
    let dir = tempfile::tempdir().unwrap();
    let downloads = Downloads::open(Some(dir.path()));
    let host = FakeHost::with(on_and_consented());
    let id = klipolo_fonts::store::DownloadedId {
        family: "Lato".into(),
        version: "v24".into(),
    };
    let busy = downloads::remove(&*host, &fonts, &downloads, id.clone(), &|| true).await;
    assert_eq!(busy, Err(klipolo_fonts::english::in_use("Lato")));
    let not_ours = downloads::remove(&*host, &fonts, &downloads, id, &|| false).await;
    assert_eq!(
        not_ours,
        Err(klipolo_fonts::download::FontError::NotOurs.code())
    );
    assert!(host.changed.lock().unwrap().is_empty());
    assert!(downloads::list(&downloads).is_empty());
}
