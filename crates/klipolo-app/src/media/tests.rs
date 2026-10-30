use super::*;
use klipolo_engine::render::probe::MediaInfo;

#[path = "../../../klipolo-engine/tests/support/fixture_set.rs"]
mod fixture_set;

fn info(kind: MediaKind, width: u32, height: u32, fps: f64) -> MediaInfo {
    MediaInfo {
        kind,
        duration: 2.5,
        width,
        height,
        fps,
        rotation: 90,
        has_audio: kind != MediaKind::Image,
        audio_lead_us: 0,
        created_ms: None,
        audio: None,
    }
}

#[test]
fn probe_results_become_assets_with_their_frame_rate() {
    let asset = asset_from_info(
        Path::new("/v.mp4"),
        info(MediaKind::Video, 1920, 1080, 29.97),
    );
    assert_eq!(asset.duration_us, 2_500_000);
    assert_eq!(asset.kind, AssetKind::Video);
    assert_eq!(
        (asset.name.as_str(), asset.path.as_str()),
        ("v.mp4", "/v.mp4")
    );
    // Kept, so the decoder opens without probing the file again.
    assert_eq!((asset.fps, asset.rotation), (29.97, Some(90)));
}

/// A video's sound lead is kept, so its own sound plays in step with its picture.
#[test]
fn a_videos_sound_lead_is_kept_from_the_probe() {
    for lead in [40_000, -21_333, 0] {
        let probed = MediaInfo {
            audio_lead_us: lead,
            ..info(MediaKind::Video, 1920, 1080, 30.0)
        };
        let asset = asset_from_info(Path::new("/phone.mp4"), probed);
        assert_eq!(asset.audio_lead_us, Some(lead));
    }
}

/// A JPEG's APP1 segment holding only an EXIF orientation tag (big-endian TIFF, one IFD entry).
fn exif_orientation(orientation: u8) -> Vec<u8> {
    let mut tiff = b"MM\0\x2A\0\0\0\x08\0\x01".to_vec();
    tiff.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, orientation, 0, 0]);
    tiff.extend([0, 0, 0, 0]);
    let mut app1 = vec![0xFF, 0xE1];
    app1.extend(((2 + 6 + tiff.len()) as u16).to_be_bytes());
    app1.extend(b"Exif\0\0");
    app1.extend(tiff);
    app1
}

#[test]
fn portrait_phone_photos_import_with_their_shown_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("phone.jpg");
    // Stored 40×20 with EXIF orientation 6 ("turn 90° clockwise to view"), as phones save portrait shots.
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode(&[90u8; 40 * 20 * 3], 40, 20, image::ExtendedColorType::Rgb8)
        .unwrap();
    let mut tagged = jpeg[..2].to_vec();
    tagged.extend(exif_orientation(6));
    tagged.extend(&jpeg[2..]);
    std::fs::write(&path, tagged).unwrap();
    // The probe reports the stored size.
    let probe = |p: &Path| Ok(asset_from_info(p, info(MediaKind::Image, 40, 20, 0.0)));
    let result = probe_each(vec![path], probe);
    let photo = &result.assets[0];
    assert_eq!((photo.width, photo.height), (20, 40));
}

#[test]
fn mixed_import_keeps_good_files_and_names_the_rest() {
    let probe = |path: &Path| {
        if path.ends_with("broken.mov") {
            return Err("probe failed".to_string());
        }
        Ok(asset_from_info(
            path,
            info(MediaKind::Video, 1920, 1080, 30.0),
        ))
    };
    let paths = ["/clips/talk.mp4", "/docs/notes.txt", "/clips/broken.mov"]
        .map(PathBuf::from)
        .to_vec();
    let result = probe_each(paths, probe);
    assert_eq!(result.assets.len(), 1);
    assert_eq!(result.assets[0].path, "/clips/talk.mp4");
    assert_eq!(
        result.skipped,
        vec![
            SkippedFile {
                name: "notes.txt".into(),
                reason: UNSUPPORTED.into()
            },
            SkippedFile {
                name: "broken.mov".into(),
                reason: UNREADABLE.into()
            },
        ]
    );
}

/// Files the probe can't read are each named with the reason, in the order given, and nothing else is skipped: a
/// damaged MP4, an empty one, a text file and one that isn't there. The good file around them still comes in.
#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn import_rejects_unreadable_files_with_reason() {
    let dir = tempfile::tempdir().unwrap();
    let damaged = dir.path().join("damaged.mp4");
    std::fs::write(&damaged, vec![7u8; 4096]).unwrap();
    let empty = dir.path().join("empty.mp4");
    std::fs::write(&empty, b"").unwrap();
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "not a video").unwrap();
    let missing = dir.path().join("missing.mov");
    let good = fixture_set::fixture_dir().join("lead-40ms.mp4");
    assert!(good.exists(), "run scripts/gen-engine-fixtures.sh");
    let result = probe_each(
        vec![damaged, good.clone(), empty, text, missing],
        probe_asset,
    );
    assert_eq!(result.assets.len(), 1);
    assert_eq!(
        Path::new(&result.assets[0].path),
        std::fs::canonicalize(&good).unwrap()
    );
    let skipped: Vec<(&str, &str)> = result
        .skipped
        .iter()
        .map(|s| (s.name.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(
        skipped,
        [
            ("damaged.mp4", UNREADABLE),
            ("empty.mp4", UNREADABLE),
            ("notes.txt", UNSUPPORTED),
            ("missing.mov", UNREADABLE),
        ]
    );
    // A reason never names a folder.
    let folder = dir.path().to_string_lossy();
    assert!(
        result
            .skipped
            .iter()
            .all(|s| !s.reason.contains(folder.as_ref()))
    );
}

/// A phone's portrait video keeps its rotation, and an NTSC video its 30000/1001 rate, so the preview draws them
/// upright and on their own frame grid.
#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn import_keeps_rotation_and_rate() {
    let dir = fixture_set::fixture_dir();
    let result = probe_each(
        vec![
            dir.join("phone-portrait.mov"),
            dir.join("ntsc-luma-10s.mp4"),
        ],
        probe_asset,
    );
    assert!(result.skipped.is_empty(), "{:?}", result.skipped);
    let [portrait, ntsc] = &result.assets[..] else {
        panic!("two assets")
    };
    assert_eq!(portrait.kind, AssetKind::Video);
    assert!(
        matches!(portrait.rotation, Some(90) | Some(270)),
        "{:?}",
        portrait.rotation
    );
    assert!((ntsc.fps - 30_000.0 / 1001.0).abs() < 1e-3, "{}", ntsc.fps);
    assert_eq!(ntsc.rotation, Some(0));
    assert!(
        (10_000_000..10_100_000).contains(&ntsc.duration_us),
        "{}",
        ntsc.duration_us
    );
}
