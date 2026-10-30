//! The render worker's scene text: drawn through the same build function as the
//! preview's and an export's, on the worker's own text build thread, with the fonts the worker was given (`make::fonts`:
//! the bundled faces and the downloads at once). Like the worker's titles it never waits for this computer's font scan,
//! so a request's deadline never pays for it, and its frames wait for their text. Its own test binary: the scan is once
//! a process, and this one holds the background scan back so a wait would show.

mod support;
use std::sync::Arc;
use std::time::Duration;

use klipolo_engine::make::fonts::{WorkerFonts, configure_worker_fonts, write_fonts};
use klipolo_engine::make::gpu::worker_headless;
use klipolo_engine::render::scene::{text_builds_on_other_threads, text_layout_calls};
use klipolo_engine::render::shared_fonts::{
    delay_background_scan_for_tests, fonts_scanned_for_tests,
};
use support::*;

#[tokio::test(flavor = "multi_thread")]
async fn the_workers_scene_text_is_built_on_its_own_thread_without_waiting_for_the_font_scan() {
    let Some(_gpu) = klipolo_engine::test_support::adapter_or_skip().await else {
        return;
    };
    delay_background_scan_for_tests(Duration::from_secs(120));
    let session_in = tempfile::tempdir().unwrap();
    write_fonts(session_in.path(), &WorkerFonts::of_this_process()).unwrap();
    let fonts = configure_worker_fonts(session_in.path());
    let mut h = worker_headless(Some(fonts)).await.unwrap();
    let (p, registry) = project_with_scene("lt", LOWER_THIRD, 0.0, 4.0);
    h.set_registry(registry);
    let (img, notes) = scene_frame(&mut h, &Arc::new(p), 1.5, H_SIZE).await;
    assert!(
        ink_bounds_in(&img, lower_third_title_box_px()).is_some(),
        "its title on the first frame: {notes:?}"
    );
    assert!(!notes.iter().any(|n| n == "frame.fontMissing"), "{notes:?}");
    assert!(text_layout_calls() > 0);
    assert_eq!(
        text_builds_on_other_threads(),
        0,
        "laid out on the worker's text build thread, never its render thread"
    );
    assert!(
        !fonts_scanned_for_tests(),
        "the worker's text never waited for this computer's font scan"
    );
}
