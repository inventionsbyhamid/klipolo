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

/// A drop of more than 100 files brings in the first 100 and names each of the rest with why.
#[test]
fn a_drop_over_100_files_takes_the_first_100_and_names_the_rest() {
    let probe = |path: &Path| Ok(asset_from_info(path, info(MediaKind::Audio, 0, 0, 0.0)));
    let paths: Vec<PathBuf> = (0..103)
        .map(|n| PathBuf::from(format!("/sounds/s{n}.wav")))
        .collect();
    let result = probe_each(paths, probe);
    assert_eq!(result.assets.len(), 100);
    assert_eq!(result.assets[99].path, "/sounds/s99.wav");
    assert_eq!(
        result.skipped,
        (100..103)
            .map(|n| SkippedFile {
                name: format!("s{n}.wav"),
                reason: "Only 100 files can be added at once.".into(),
            })
            .collect::<Vec<_>>()
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

/// An SVG is named with what to do instead; nothing else in the drop changes.
#[test]
fn svg_is_refused_with_a_plain_reason() {
    let probe = |path: &Path| Ok(asset_from_info(path, info(MediaKind::Image, 512, 512, 0.0)));
    let result = probe_each(
        ["/art/logo.svg", "/art/logo.png", "/art/Badge.SVG"]
            .map(PathBuf::from)
            .to_vec(),
        probe,
    );
    assert_eq!(result.assets.len(), 1);
    assert_eq!(
        result.skipped,
        vec![
            SkippedFile {
                name: "logo.svg".into(),
                reason: "SVG isn't supported yet. Export it as PNG.".into()
            },
            SkippedFile {
                name: "Badge.SVG".into(),
                reason: "SVG isn't supported yet. Export it as PNG.".into()
            },
        ]
    );
}

/// Sounds and pictures of every type Klipolo imports come in as what they are (the page puts them in the library, never
/// on the main track); a GIF plays as a video.
#[test]
fn sounds_and_pictures_import_as_their_kinds() {
    for name in ["a.mp3", "a.wav", "a.m4a", "a.aac", "a.flac", "a.ogg"] {
        assert!(supported(Path::new(name)), "{name}");
    }
    for name in [
        "a.png", "a.jpg", "a.jpeg", "a.webp", "a.heic", "a.avif", "a.tif", "a.TIFF", "a.gif",
    ] {
        assert!(supported(Path::new(name)), "{name}");
    }
    let audio = asset_from_info(Path::new("/m/song.wav"), info(MediaKind::Audio, 0, 0, 0.0));
    assert_eq!(
        (audio.kind, audio.rotation, audio.audio_lead_us),
        (AssetKind::Audio, None, None)
    );
}

/// An MP3 with its cover picture (an ID3 `APIC` frame, which FFmpeg shows as a picture stream) is a sound, not a video.
#[test]
fn mp3_with_cover_art_is_audio() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.mp3");
    let mut cover = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut cover)
        .encode(
            &[200u8; 16 * 16 * 3],
            16,
            16,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    // ID3v2.3: one APIC frame (Latin-1, image/jpeg, front cover, no description).
    let mut apic = vec![0u8];
    apic.extend(b"image/jpeg\0");
    apic.extend([3u8, 0]);
    apic.extend(&cover);
    let mut frame = b"APIC".to_vec();
    frame.extend((apic.len() as u32).to_be_bytes());
    frame.extend([0, 0]);
    frame.extend(&apic);
    let syncsafe = |n: usize| {
        [
            (n >> 21) as u8 & 0x7F,
            (n >> 14) as u8 & 0x7F,
            (n >> 7) as u8 & 0x7F,
            n as u8 & 0x7F,
        ]
    };
    let mut file = b"ID3\x03\x00\x00".to_vec();
    file.extend(syncsafe(frame.len()));
    file.extend(&frame);
    // 60 silent MPEG-1 Layer III frames: 128 kb/s at 44.1 kHz, 417 bytes each, zero side info (1.6 s).
    for _ in 0..60 {
        let mut mp3 = vec![0xFF, 0xFB, 0x90, 0x00];
        mp3.resize(417, 0);
        file.extend(mp3);
    }
    std::fs::write(&path, file).unwrap();
    let result = probe_each(vec![path], probe_asset);
    assert!(result.skipped.is_empty(), "{:?}", result.skipped);
    let song = &result.assets[0];
    assert_eq!(song.kind, AssetKind::Audio);
    assert!(song.has_audio);
    assert!(
        (1_000_000..2_500_000).contains(&song.duration_us),
        "{}",
        song.duration_us
    );
}
