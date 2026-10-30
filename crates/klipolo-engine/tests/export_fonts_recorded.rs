//! An export worker never scans this computer's fonts again before frame 0:
//! the app records what its own scan found beside the job's manifest each run, and the worker builds its first font
//! system from that record: the same families, faces and forbidden fallbacks, with no folder walked and no face read
//! for its coverage. Its own binary: the engine's fonts are once a process.

use std::path::Path;
use std::sync::Arc;

use klipolo_engine::export::job::ExportJob;
use klipolo_engine::render::fonts::{
    self, configure_for_tests, family_for, test_group_installed, test_record_scan_of,
    test_template_face,
};
use klipolo_model::font::{FontRef, FontSource};

/// A bundled file copied into `dir` under another name: a family found there counts as installed.
fn installed_copy(dir: &Path, file: &str) -> std::path::PathBuf {
    let to = dir.join(format!("Installed-{}", file.replace('/', "-")));
    std::fs::copy(
        klipolo_fonts::bundled::bundled_dir().unwrap().join(file),
        &to,
    )
    .unwrap();
    to
}

fn installed(family: &str) -> FontRef {
    FontRef {
        family: family.into(),
        source: FontSource::Installed,
    }
}

#[test]
fn an_export_worker_draws_the_apps_scan_and_never_scans_again() {
    configure_for_tests();
    // The app's side: its scan found Anton installed, and a job made now records that scan beside its manifest.
    let app_fonts = tempfile::tempdir().unwrap();
    let anton = installed_copy(app_fonts.path(), "Anton/Anton-Regular.ttf");
    let (p, sp) = klipolo_engine::test_support::captions_project("caption.clean", &[]);
    let out = tempfile::tempdir().unwrap();
    let job = ExportJob::create_for_test(out.path(), &p, Arc::new(sp));
    test_record_scan_of(app_fonts.path(), &job.dir);
    // A folder a scan of this computer would read: the worker must never walk it.
    let unread = tempfile::tempdir().unwrap();
    installed_copy(unread.path(), "BebasNeue/BebasNeue-Regular.ttf");
    klipolo_engine::vendor::cap::rendering::layers::scan_also_for_tests(unread.path());

    // The worker's side, before frame 0: the job's fonts, then its first font system (the compositor's).
    let opened = ExportJob::open(&job.dir).unwrap();
    klipolo_engine::export::worker::fonts_before_frame_zero(&opened);
    assert_eq!(
        test_template_face("Anton", 400, false, "M"),
        ("Anton".to_string(), 400),
        "the app's family draws from its own file"
    );
    let choice = family_for(&fonts::index(), &installed("Anton"), 400, false);
    assert_eq!(
        (choice.family.as_str(), choice.stand_in),
        ("Anton", false),
        "and resolves as installed"
    );
    assert!(
        family_for(&fonts::index(), &installed("Bebas Neue"), 400, false).stand_in,
        "no folder was scanned"
    );
    assert_eq!(
        *fonts::installed_families(),
        *test_group_installed(&[anton]),
        "the families as the app's scan grouped them"
    );
}
