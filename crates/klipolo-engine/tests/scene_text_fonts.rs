//! An export's scene text waits for this computer's fonts: its build thread scans
//! them before laying an export's text out, whichever way the build was asked (ahead of its frame, or by the frame that
//! waits for it). Its own test binary: the scan is once a process, and this one must see the folder below first. The
//! background scan every compositor starts is held back, so the face can only
//! come from the build thread's own wait: a thread that didn't wait would draw the stand-in.

mod support;
use support::*;

/// `font` with every family and full name `from` renamed `to` (as long), in ASCII and UTF-16BE name records: a face
/// no bundled family matches. Its tables' checksums are left as they were; the parsers don't check them.
fn renamed(font: &[u8], from: &str, to: &str) -> Vec<u8> {
    assert_eq!(from.len(), to.len());
    let wide = |s: &str| s.bytes().flat_map(|b| [0, b]).collect::<Vec<u8>>();
    let mut out = font.to_vec();
    for (a, b) in [
        (from.as_bytes().to_vec(), to.as_bytes().to_vec()),
        (wide(from), wide(to)),
    ] {
        let mut i = 0;
        while i + a.len() <= out.len() {
            if out[i..i + a.len()] == a[..] {
                out[i..i + a.len()].copy_from_slice(&b);
                i += a.len();
            } else {
                i += 1;
            }
        }
    }
    out
}

/// A family only the system scan can find (Anton renamed "Antox", in a folder the scan also reads, as installed), named
/// by a scene's text node, drawn the export's way on the first frame: in that family, not its stand-in.
#[tokio::test(flavor = "multi_thread")]
async fn an_exported_scene_draws_a_family_that_only_the_scan_knows() {
    let Some(_gpu) = klipolo_engine::test_support::adapter_or_skip().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let anton = klipolo_fonts::bundled::bundled_dir()
        .unwrap()
        .join("Anton/Anton-Regular.ttf");
    let antox = renamed(&std::fs::read(anton).unwrap(), "Anton", "Antox");
    std::fs::write(dir.path().join("Antox-Regular.ttf"), antox).unwrap();
    klipolo_engine::vendor::cap::rendering::layers::scan_also_for_tests(dir.path());
    klipolo_engine::render::shared_fonts::delay_background_scan_for_tests(
        std::time::Duration::from_secs(120),
    );
    let scene = |font: &str| {
        format!(
            r##"{{"scene":1,"duration":2,"nodes":[{{"id":"t","type":"text","text":"MMMM","box":[0.05,0.3,0.9,0.4],
              "size":160,"fill":"#FFFFFF","font":{font}}}]}}"##
        )
    };
    let installed = scene(r#"{"family":"Antox","source":{"type":"installed"}}"#);
    let bundled = scene(r#"{"family":"Inter","source":{"type":"bundled"}}"#);
    let registry = scene_registry(&[("anton", installed.as_str()), ("inter", bundled.as_str())]);
    let mut widths = Vec::new();
    for name in ["anton", "inter"] {
        let (mut p, _) = project_with_scene(name, &installed, 0.0, 2.0);
        if let klipolo_model::timeline::ItemContent::Generator { source, .. } =
            &mut scene_item(&mut p, name).content
        {
            *source = scene_id(name).as_str().into();
        }
        // Nothing scanned before this export's first frame.
        let mut h = klipolo_engine::headless::Headless::new(registry.clone())
            .await
            .unwrap();
        if name == "anton" {
            assert!(
                !klipolo_engine::render::shared_fonts::fonts_scanned_for_tests(),
                "the background scan is held back: only the build thread's wait can find Antox"
            );
        }
        let (img, notes) = scene_frame(&mut h, &std::sync::Arc::new(p), 1.0, H_SIZE).await;
        assert!(!notes.iter().any(|n| n == "frame.fontMissing"), "{notes:?}");
        let all = Bounds {
            x0: 0.0,
            y0: 0.0,
            x1: f64::from(H_SIZE.0),
            y1: f64::from(H_SIZE.1),
        };
        widths.push(ink_bounds_in(&img, all).map_or(0.0, |b| b.x1 - b.x0));
    }
    // Antox (Anton) is condensed: its four Ms are about 0.85 of Inter's, its stand-in's.
    assert!(
        widths[0] > 0.0 && widths[0] < 0.92 * widths[1],
        "Antox, not its stand-in: {widths:?}"
    );
}
