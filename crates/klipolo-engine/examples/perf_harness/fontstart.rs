//! `cargo run --release -p klipolo-engine --example perf_harness -- fontstart [<faces>]`: what an export worker's fonts
//! cost before frame 0, the old way and the new. A folder of `<faces>` font
//! files (default 800, copies of the bundled ones under other names: a Mac's 700 to 1,000 installed faces, never the
//! person's own) stands in for this computer's fonts, and each way runs in a fresh process, as a worker does:
//! - **scan** (before): the worker's first font system scans the system's font folders and the fixture's, and reads
//!   every face's coverage;
//! - **recorded** (after): the worker reads its job's record of the app's scan and builds its first font system from
//!   it (`render::fonts::use_recorded_scan`).
//!
//! Each reports the ms from its start to the first font system ready with a glyph shaped (what the compositor needs
//! before frame 0), and the record's size and write time (the app's side, once a run).

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use klipolo_engine::render::fonts;

/// The bundled font files the fixture copies.
fn bundled_files() -> Vec<std::path::PathBuf> {
    let dir = klipolo_fonts::bundled::bundled_dir().expect("the bundled fonts");
    let mut files: Vec<_> = walk(&dir)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "ttf"))
        .collect();
    files.sort();
    files
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// The first font system, as the compositor makes it before frame 0, with one glyph shaped.
fn first_font_system() -> f64 {
    let started = Instant::now();
    let _ = fonts::test_template_face("Inter", 400, false, "M");
    ms(started)
}

/// The parent: the fixture, then each way in its own process.
pub fn run(args: &[String]) {
    let faces: usize = args.first().and_then(|a| a.parse().ok()).unwrap_or(800);
    let fixture = tempfile::tempdir().expect("a temp folder");
    let files = bundled_files();
    let started = Instant::now();
    for n in 0..faces {
        let from = &files[n % files.len()];
        std::fs::copy(from, fixture.path().join(format!("face-{n:04}.ttf"))).expect("a copy");
    }
    println!(
        "fontstart: {faces} face files in the fixture ({:.0} ms to copy)",
        ms(started)
    );
    let job = tempfile::tempdir().expect("a job folder");
    let me = std::env::current_exe().expect("this binary");
    for (way, arg) in [
        ("scan (before)", fixture.path()),
        ("recorded (after)", job.path()),
    ] {
        let mode = if way.starts_with("scan") {
            "scan"
        } else {
            "recorded"
        };
        let out = Command::new(&me)
            .args(["fontstart-child", mode])
            .arg(arg)
            .arg(job.path())
            .output()
            .expect("the child runs");
        print!("{}", String::from_utf8_lossy(&out.stdout));
        if !out.status.success() {
            eprintln!("{way}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
}

/// A child: `scan <fixture> <job>` scans (the old worker start), then writes the record the app would (`<job>`);
/// `recorded <job> <job>` builds from that record (the new worker start).
pub fn child(args: &[String]) {
    let (Some(mode), Some(from), Some(job)) = (args.first(), args.get(1), args.get(2)) else {
        eprintln!("usage: fontstart-child <scan|recorded> <folder> <job folder>");
        std::process::exit(2);
    };
    let started = Instant::now();
    fonts::configure_for_tests();
    if mode == "scan" {
        klipolo_engine::vendor::cap::rendering::layers::scan_also_for_tests(Path::new(from));
        let ready = first_font_system();
        let total = ms(started);
        let wrote = Instant::now();
        fonts::record_scan(Path::new(job));
        let write_ms = ms(wrote);
        let bytes = std::fs::metadata(Path::new(job).join("installed-fonts.json"))
            .map(|m| m.len())
            .unwrap_or(0);
        println!(
            "scan (before): first font system {ready:.0} ms ({total:.0} ms from the worker's start); {} installed \
             families; the app's record of it: {:.0} KB, written in {write_ms:.1} ms",
            fonts::installed_families().len(),
            bytes as f64 / 1024.0,
        );
    } else {
        let read = Instant::now();
        let given = fonts::use_recorded_scan(Path::new(from));
        let read_ms = ms(read);
        let ready = first_font_system();
        let total = ms(started);
        println!(
            "recorded (after): record read in {read_ms:.1} ms ({}), first font system {ready:.0} ms ({total:.0} ms \
             from the worker's start); {} installed families",
            if given { "used" } else { "NOT used" },
            fonts::installed_families().len(),
        );
    }
}
