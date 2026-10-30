//! Every media file opens through `media::open`. A file named like media
//! whose contents name other files (an `ffconcat` list, a numbered image pattern) is refused, and the files it names
//! are never read: each is a canary that would change the probe's answer if it were. Photos and pictures still open.
//! And no code outside tests opens a file with libavformat any other way.

use std::path::{Path, PathBuf};

use klipolo_engine::render::probe::{MediaKind, probe_media};

/// A second of 48 kHz mono silence as a 16-bit WAV.
fn wav(path: &Path) {
    let (rate, frames) = (48_000u32, 48_000u32);
    let data = frames * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    b.resize(44 + data as usize, 0);
    std::fs::write(path, b).unwrap();
}

/// `clip.mp4` is an `ffconcat` list naming the canary beside it. Followed, the probe would find the canary's sound and
/// the PCM cache's probe its channel; refused, neither finds anything.
#[test]
fn an_ffconcat_list_named_like_media_reads_no_other_file() {
    let dir = tempfile::tempdir().unwrap();
    let canary = dir.path().join("canary.wav");
    wav(&canary);
    assert_eq!(
        probe_media(&canary).unwrap().kind,
        MediaKind::Audio,
        "the canary reads as sound"
    );
    let list = dir.path().join("clip.mp4");
    std::fs::write(&list, "ffconcat version 1.0\nfile canary.wav\nduration 1\n").unwrap();
    assert!(
        probe_media(&list).is_err(),
        "the probe followed the list: {:?}",
        probe_media(&list)
    );
    assert!(
        klipolo_engine::audio::decode::probe(&list).is_err(),
        "the PCM cache's probe followed the list"
    );
}

/// `x%d.png` is not a picture, and `x0.png`… beside it are (40 × 30). Read as a numbered sequence, it would probe as a
/// 40 × 30 picture; read as the file named, it isn't one.
#[test]
fn a_numbered_image_name_reads_only_itself() {
    let dir = tempfile::tempdir().unwrap();
    for n in 0..3 {
        let canary = dir.path().join(format!("x{n}.png"));
        image::RgbaImage::new(40, 30).save(&canary).unwrap();
    }
    let pattern = dir.path().join("x%d.png");
    std::fs::write(&pattern, b"not a picture").unwrap();
    assert!(
        probe_media(&pattern).is_err(),
        "the probe read a numbered sequence: {:?}",
        probe_media(&pattern)
    );
}

/// A one-frame animated PNG (APNG), `w` × `h`, transparent: its `acTL` makes FFmpeg's `apng` demuxer take it.
fn apng(path: &Path, (w, h): (u32, u32)) {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    // Each row: filter byte 0, then RGBA; stored in one uncompressed deflate block.
    let raw = vec![0u8; (h * (1 + 4 * w)) as usize];
    let mut zlib = vec![0x78, 0x01, 0x01];
    let len = raw.len() as u16;
    zlib.extend_from_slice(&len.to_le_bytes());
    zlib.extend_from_slice(&(!len).to_le_bytes());
    zlib.extend_from_slice(&raw);
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits, RGBA
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]); // one frame, played forever
    let mut fctl = vec![0, 0, 0, 0]; // sequence 0
    fctl.extend_from_slice(&w.to_be_bytes());
    fctl.extend_from_slice(&h.to_be_bytes());
    fctl.extend_from_slice(&[0; 8]); // at 0, 0
    fctl.extend_from_slice(&[0, 1, 0, 10, 0, 0]); // 1/10 s, no dispose, source
    chunk(&mut png, b"fcTL", &fctl);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png).unwrap();
}

/// What the guard keeps: a photo (a JPEG past the size the probe first reads, which `image2` takes by its extension), a
/// PNG, an animated PNG (the `apng` demuxer takes it) and a WebP picture, and a WAV sound.
#[test]
fn photos_pictures_and_sound_still_open() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("photo.jpg");
    image::RgbImage::from_fn(640, 480, |x, y| {
        image::Rgb([((x * 7) ^ (y * 13)) as u8, (x * y) as u8, (x + y) as u8])
    })
    .save(&photo)
    .unwrap();
    assert!(
        std::fs::metadata(&photo).unwrap().len() > 4096,
        "past the first probe read"
    );
    let png = dir.path().join("logo.png");
    image::RgbaImage::new(40, 30).save(&png).unwrap();
    let animated = dir.path().join("spinner.png");
    apng(&animated, (4, 2));
    let webp = dir.path().join("sticker.webp");
    image::RgbaImage::new(32, 16).save(&webp).unwrap();
    let sound = dir.path().join("voice.wav");
    wav(&sound);
    for (path, kind, size) in [
        (&photo, MediaKind::Image, (640, 480)),
        (&png, MediaKind::Image, (40, 30)),
        (&animated, MediaKind::Image, (4, 2)),
        (&webp, MediaKind::Image, (32, 16)),
        (&sound, MediaKind::Audio, (0, 0)),
    ] {
        let info = probe_media(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            (info.kind, (info.width, info.height)),
            (kind, size),
            "{}",
            path.display()
        );
    }
    assert!(klipolo_engine::audio::decode::probe(&sound).is_ok());
}

/// The one place allowed to open an input.
const HELPER: &str = "crates/klipolo-engine/src/media/open.rs";
/// How code opens an input with libavformat directly (`format::open` and `open_with` too, and the context's own
/// own `wrap`). `format::input` covers `avformat::input(`.
const DIRECT: [&str; 7] = [
    "format::input",
    "input_with_dictionary(",
    "input_with_interrupt",
    "avformat_open_input",
    "input_from_stream",
    "format::open",
    "Input::wrap",
];
/// Names a `use ffmpeg::format::{…}` can bring in to open an input by.
const IMPORTED: [&str; 4] = ["input", "open", "open_with", "input_with_dictionary"];

/// Every crate's sources: `src` under each workspace crate (`crates/*`, the app's among them; the vendored code
/// is inside the engine's), and under `tools/*` once there are tools.
fn source_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for parent in ["crates", "tools"] {
        let Ok(entries) = std::fs::read_dir(root.join(parent)) else {
            continue;
        };
        for entry in entries.flatten() {
            let src = entry.path().join("src");
            if src.is_dir() {
                roots.push(src);
            }
        }
    }
    roots
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A test file: `tests.rs` or `tests_*.rs` (a module only `#[cfg(test)]` declares), or under a `tests` folder of the
/// repository (`path` relative to it: a checkout under a folder named `tests` skips nothing). A file
/// named otherwise (`testing.rs`, `test_support.rs`) is scanned: it may be compiled into the app.
fn test_file(path: &Path) -> bool {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    stem == "tests"
        || stem.starts_with("tests_")
        || path.components().any(|c| c.as_os_str() == "tests")
}

/// `text` without its inline test modules (`#[cfg(test)]` then `mod … {`, up to the brace that closes it); code after
/// one is kept.
fn without_tests(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let next = lines.get(i + 1).map_or("", |l| l.trim());
        let module = next
            .trim_start_matches("pub(crate) ")
            .trim_start_matches("pub ");
        if lines[i].trim() == "#[cfg(test)]" && module.starts_with("mod ") && next.ends_with('{') {
            // Skip to the module's closing brace: the first later line that is the `mod` line's indentation and `}`
            // (rustfmt's shape, which the gate checks), never a count of braces, which a `'{'` or a string inside the
            // module would throw. Its lines stay counted as blanks, so line numbers hold.
            let indent = &lines[i + 1][..lines[i + 1].len() - lines[i + 1].trim_start().len()];
            let close = format!("{indent}}}");
            let mut j = i + 2;
            while j < lines.len() && lines[j].trim_end() != close {
                j += 1;
            }
            kept.extend(std::iter::repeat_n("", j + 1 - i));
            i = j + 1;
            continue;
        }
        kept.push(lines[i]);
        i += 1;
    }
    kept.join("\n")
}

/// The lines of `text` (1-based) that open an input directly: a call by its path, a name imported to call by (a
/// `format::{…}` list over one line or several), or the module brought in some other way, under another name or whole
/// (`format as fmt`, `format::*`), which the scan can't follow.
fn direct_opens(text: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let imported = code.split_once("format::{").is_some_and(|(_, names)| {
            // A list that goes on past this line reads up to its `}` (the multi-line import).
            let mut list = names.to_owned();
            let mut more = lines.iter().skip(n + 1);
            while !list.contains('}') {
                let Some(next) = more.next() else { break };
                list.push(',');
                list.push_str(next);
            }
            list.split(['}', ','])
                .map(|name| name.trim().split(" as ").next().unwrap_or("").trim())
                .any(|name| IMPORTED.contains(&name))
        });
        let unfollowed = code.starts_with("use ")
            && (code.contains("format::*")
                || code
                    .split_once("format as ")
                    .is_some_and(|(_, alias)| !alias.starts_with("avformat"))
                || code.contains("format::{self as ")
                    && !code.contains("format::{self as avformat"));
        if imported || unfollowed || DIRECT.iter().any(|d| code.contains(d)) {
            found.push((n + 1, code.to_owned()));
        }
    }
    found
}

/// The scan finds an open by `format::open` and `open_with`, a name imported to open by, and an open placed
/// after an inline test module; it reads `testing.rs`, and skips only test files.
#[test]
fn the_scan_finds_every_way_to_open_an_input() {
    let planted = [
        "use ffmpeg::format::{input, Pixel};",
        "fn a(p: &Path) { let _ = input(p); }",
        "fn b(p: &Path) { ffmpeg::format::open(p, &Format::Input(f)) }",
        "fn c(p: &Path) { ffmpeg::format::open_with(p, &f, opts) }",
        "#[cfg(test)]",
        "mod tests {",
        "    fn t(p: &Path) { ffmpeg::format::input(p); }",
        "}",
        "fn after(p: &Path) { ffmpeg::format::input_with_dictionary(p, o) }",
        "use ffmpeg::format::{self as avformat, context::input::PacketIter};",
        "use ffmpeg::format::{",
        "    Pixel,",
        "    input,",
        "};",
        "use ffmpeg::format as fmt;",
        "use ffmpeg::format::*;",
        "#[cfg(test)]",
        "mod more {",
        "    const OPEN: char = '{';",
        "    fn t(p: &Path) { ffmpeg::format::input(p); }",
        "}",
        "fn last(p: &Path) { ffmpeg::format::open(p, &f) }",
    ]
    .join("\n");
    let lines: Vec<usize> = direct_opens(&without_tests(&planted))
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(
        lines,
        [1, 3, 4, 9, 11, 15, 16, 22],
        "the import, open, open_with, the open after the test module, a list over several lines, a module renamed \
         or brought in whole, and an open after a test module holding a `'{{'`"
    );
    for test in [
        "crates/x/src/tests.rs",
        "crates/x/src/media/tests_scan.rs",
        "crates/x/tests/open.rs",
    ] {
        assert!(test_file(Path::new(test)), "{test}");
    }
    for compiled in [
        "crates/klipolo-engine/src/render/captions/testing.rs",
        "crates/klipolo-app/src/test_support.rs",
    ] {
        assert!(!test_file(Path::new(compiled)), "{compiled}");
    }
}

#[test]
fn every_media_open_goes_through_the_one_helper() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let roots = source_roots(&root);
    for crate_src in [
        "crates/klipolo-app/src",
        "crates/klipolo-engine/src",
        "crates/klipolo-model/src",
    ] {
        assert!(
            roots.contains(&root.join(crate_src)),
            "{crate_src} is scanned"
        );
    }
    let mut files = Vec::new();
    for dir in &roots {
        rust_files(dir, &mut files);
    }
    let helper = root.join(HELPER);
    let mut direct = Vec::new();
    let mut scanned = 0;
    for path in files {
        let relative = path.strip_prefix(&root).unwrap_or(&path);
        if test_file(relative) || path == helper {
            continue;
        }
        scanned += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        for (n, code) in direct_opens(&without_tests(&text)) {
            let shown = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            direct.push(format!("{shown}:{n}: {code}"));
        }
    }
    // The workspace's crates hold about 840 source files (the engine alone about 540).
    assert!(scanned > 750, "the sources were scanned: {scanned}");
    assert!(
        direct.is_empty(),
        "open media through klipolo_engine::media::open, never directly:\n{}",
        direct.join("\n")
    );
}

/// The scan's backstop: only the engine links FFmpeg's bindings outside tests, so any other crate reaches FFmpeg
/// through the engine (and its one open), whatever its imports look like.
#[test]
fn only_the_engine_links_ffmpeg() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut linking = Vec::new();
    for src in source_roots(&root) {
        let manifest = src.parent().unwrap().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let mut section = String::new();
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                section = line.to_owned();
                continue;
            }
            let normal =
                section.ends_with("dependencies]") && !section.ends_with("dev-dependencies]");
            if normal && (line.contains("ffmpeg-next") || line.contains("ffmpeg-sys-next")) {
                let shown = manifest
                    .strip_prefix(&root)
                    .unwrap_or(&manifest)
                    .display()
                    .to_string();
                linking.push(shown);
            }
        }
    }
    linking.dedup();
    assert_eq!(linking, ["crates/klipolo-engine/Cargo.toml"]);
}
