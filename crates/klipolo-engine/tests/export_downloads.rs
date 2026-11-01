//! An export draws the downloaded fonts the app had: the job records the downloaded families the app registered, and
//! its worker registers them before frame 0, a Google family and a Noto Sans JP, whose faces are then the Japanese
//! fallback. Its own binary: the engine's fonts are once a process.

use std::sync::Arc;

use klipolo_engine::export::job::ExportJob;
use klipolo_engine::render::fonts::{self, configure_for_tests, family_for, test_fallback_face};
use klipolo_fonts::cjk::CjkCover;
use klipolo_fonts::index::{Downloaded, DownloadedFace};
use klipolo_fonts::resolve::FaceFile;
use klipolo_model::text_language::CjkScript;

/// A downloaded family whose one face is a bundled file standing in for Google's bytes.
fn download(family: &str, version: &str, weight: u16, file: &str) -> Downloaded {
    let path = klipolo_fonts::bundled::bundled_dir().unwrap().join(file);
    Downloaded {
        family: family.into(),
        version: version.into(),
        faces: vec![DownloadedFace {
            weight,
            italic: false,
            file: FaceFile { path, index: 0 },
        }],
        cover: CjkCover::NONE,
    }
}

#[test]
fn an_export_registers_the_jobs_downloads_before_frame_zero() {
    configure_for_tests();
    let testa = download("Testa", "v1", 700, "Inter/Inter-Bold.ttf");
    let noto = download("Noto Sans JP", "v54", 400, "Inter/Inter-Regular.ttf");
    // The app's side: its downloads registered; a job made now records them.
    fonts::register(vec![testa.clone(), noto.clone()]);
    let (p, sp) = klipolo_engine::test_support::captions_project("caption.clean", &[]);
    let out = tempfile::tempdir().unwrap();
    let job = ExportJob::create_for_test(out.path(), &p, Arc::new(sp));
    assert_eq!(job.downloads, [testa.clone(), noto.clone()]);
    // The worker's side, a process that has registered nothing: the job's downloads, before frame 0.
    fonts::unregister(&testa.alias());
    fonts::unregister(&noto.alias());
    assert!(family_for(&fonts::index(), &testa.font(), 700, false).stand_in);
    assert!(
        !fonts::cjk_registered(CjkScript::Jp),
        "no Japanese fallback before"
    );
    let opened = ExportJob::open(&job.dir).unwrap();
    klipolo_engine::export::worker::fonts_before_frame_zero(&opened);
    let choice = family_for(&fonts::index(), &testa.font(), 700, false);
    assert_eq!(
        (choice.family.as_str(), choice.stand_in),
        ("kl:g:testa:v1", false),
        "the Google family draws, no stand-in"
    );
    assert!(fonts::cjk_registered(CjkScript::Jp));
    // A Japanese title in an English project asks for the Japanese fallback, never NotDef's boxes…
    let runs =
        klipolo_engine::render::script_runs::runs("今日は", "kl:b:inter", "en", CjkCover::NONE);
    assert_eq!(&*runs[0].1, "kl:fallback:jp");
    // …which the engine registered from the Noto file at every weight (its stand-in file draws Latin: it answers).
    for weight in [100, 400, 900] {
        assert_eq!(
            test_fallback_face("kl:fallback:jp", "A", weight, false),
            ("kl:fallback:jp".to_string(), weight)
        );
    }
}
