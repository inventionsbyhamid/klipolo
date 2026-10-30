//! The export worker waits for the system font scan and registers its job's faces before frame 0. Its own test
//! binary: the scan is once per process, and this one must see the folder below.

use klipolo_model::font::{FontRef, FontSource};

#[tokio::test(flavor = "multi_thread")]
async fn an_export_draws_a_family_that_only_the_scan_knows() {
    let Some(gpu) = klipolo_engine::test_support::adapter_or_skip().await else {
        return;
    };
    // A family only the system scan can find: a bundled file copied into a folder the scan also reads (as installed).
    let dir = tempfile::tempdir().unwrap();
    let anton = klipolo_fonts::bundled::bundled_dir()
        .unwrap()
        .join("Anton/Anton-Regular.ttf");
    std::fs::copy(anton, dir.path().join("Installed-Anton.ttf")).unwrap();
    klipolo_engine::vendor::cap::rendering::layers::scan_also_for_tests(dir.path());
    let font = FontRef {
        family: "Anton".into(),
        source: FontSource::Installed,
    };
    // As the worker does it: `configure(font_dirs(&job))`, `register(job faces)`, then a compositor with `wait`.
    // Nothing started the scan before this call.
    let frame = klipolo_engine::test_support::render_text_via_export(&gpu, "MMMM", &font).await;
    assert!(
        !frame.warnings.iter().any(|w| w == "frame.fontMissing"),
        "{:?}",
        frame.warnings
    );
    let index = klipolo_engine::render::fonts::index();
    assert!(
        !klipolo_engine::render::fonts::family_for(&index, &font, 400, false).stand_in,
        "the scan ran and published it"
    );
}
