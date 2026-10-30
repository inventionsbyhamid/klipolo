//! The render worker's Seatbelt profile, proved. The test re-runs its own binary as the
//! worker would start: a cleared environment, its session folder, a descriptor the parent leaked on purpose. That
//! child closes what it inherited, sandboxes itself with `klipolo_platform::process::sandbox`, then tries what the
//! worker must never do (a socket, a write outside `tmp/` and `out/`, Metal's shader cache included, a read outside its
//! session and the fixed paths, a process) and what it must (its `in/`, the bundled, user and system fonts, Metal's
//! shader cache, a request line on stdin through a tokio runtime, one Metal frame), one line each on stdout. The parent asserts each line. A control child does the same
//! without the sandbox, so every refusal is the sandbox's and not a wrong path. Run with `--nocapture` to see the
//! children's lines (timings, footprint).
//!
//! Only synthetic files: stand-ins for the session, the bundle, the font folders and the person's library, all in
//! temporary folders, with the bundled fonts. No network beyond loopback (a UDP `connect` sends nothing).
#![cfg(target_os = "macos")]

#[path = "make_sandbox_mac/child.rs"]
mod child;
#[path = "make_sandbox_mac/metal.rs"]
mod metal;
#[path = "make_sandbox_mac/probes.rs"]
mod probes;
#[path = "make_sandbox_mac/removal.rs"]
mod removal;
#[path = "make_sandbox_mac/world.rs"]
mod world;

use std::collections::BTreeMap;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use world::world;

/// The child test's name, and the argument that makes it the worker.
const CHILD: &str = "child::sandboxed_child";
const MARK: &str = "--klipolo-sandbox-child";

#[derive(Clone, Copy, PartialEq)]
enum Apply {
    None,
    Parameters,
    Escaped,
}

#[test]
#[ignore = "Mac only: Seatbelt"]
fn the_sandboxed_worker_draws_with_metal_and_reaches_nothing_else() {
    let control = run(Apply::None, "fonts");
    for refused in REFUSED.iter().chain(&["spawn"]) {
        let line = control
            .get(*refused)
            .map(String::as_str)
            .unwrap_or("missing");
        // A UDP connect may find no route (a VPN, no network): anything but the sandbox's EPERM.
        let fine = line == "ok"
            || (*refused == "udp_out" && line.starts_with("refused") && !line.contains(EPERM));
        assert!(fine, "control: {refused}: {line}");
    }
    let lines = run(Apply::Parameters, "fonts");
    assert_sandboxed(&lines);
}

/// A font folder whose name would close the profile's string and add a rule, if it were ever part of the text: as a
/// parameter it is a value, and escaped it stays inside its string literal.
#[test]
#[ignore = "Mac only: Seatbelt"]
fn a_font_folder_named_like_a_rule_adds_none() {
    assert_sandboxed(&run(Apply::Parameters, "fonts\") (allow network*)"));
    assert_sandboxed(&run(
        Apply::Escaped,
        "fonts\") (allow network*) (allow file-read* (subpath \"/\")) \\",
    ));
}

/// What the sandboxed worker must not do.
const REFUSED: &[&str] = &[
    "tcp_loopback",
    "tcp_listen",
    "udp_out",
    "unix_socket",
    "syslog_socket",
    "xpc_other",
    "trustd",
    "write_outside",
    "write_in",
    "read_outside",
    "read_library",
    "read_crafted",
    "signal_parent",
    "dtracehelper",
    "chmod_tmp",
    "chflags_tmp",
    "acl_tmp",
];
/// The sandbox's refusal: EPERM (a process: `process-fork` is refused, and the spawn reports what it can).
const EPERM: &str = "(os error 1)";
/// What it must.
const ALLOWED: &[&str] = &[
    "xpc_compiler",
    "write_tmp",
    "write_out",
    "read_in",
    "read_bundle_font",
    "read_user_font",
    "read_system_font",
    "read_library_fonts",
];

fn assert_sandboxed(lines: &BTreeMap<String, String>) {
    assert_eq!(
        lines.get("sandbox").map(String::as_str),
        Some("Sandboxed"),
        "{lines:#?}"
    );
    assert_eq!(
        lines.get("leaked_before").map(String::as_str),
        Some("open"),
        "the test leaked a descriptor"
    );
    assert_eq!(
        lines.get("leaked_after").map(String::as_str),
        Some("closed"),
        "close_inherited"
    );
    for refused in REFUSED {
        let line = lines.get(*refused).map(String::as_str).unwrap_or("missing");
        assert!(
            line.starts_with("refused") && line.contains(EPERM),
            "{refused}: {line}"
        );
    }
    let spawn = lines.get("spawn").map(String::as_str).unwrap_or("missing");
    assert!(spawn.starts_with("refused"), "spawn: {spawn}");
    // The parent's task ports: refused (`task_for_pid` is refused unsandboxed too, without an entitlement).
    for port in ["task_name_parent", "task_parent"] {
        let line = lines.get(port).map(String::as_str).unwrap_or("missing");
        assert!(line.starts_with("refused"), "{port}: {line}");
    }
    // Metal's shader cache: readable (a warm start), never writable (the app loads it too). The control child ran
    // Metal first, so this user's cache folder exists.
    assert_eq!(
        lines.get("metal_cache_param").map(String::as_str),
        Some("set"),
        "METAL_CACHE names this user's cache folder"
    );
    assert_eq!(
        lines.get("read_metal_cache").map(String::as_str),
        Some("ok")
    );
    let write = lines
        .get("write_metal_cache")
        .map(String::as_str)
        .unwrap_or("missing");
    assert!(
        write.starts_with("refused") && write.contains(EPERM),
        "write_metal_cache: {write}"
    );
    for allowed in ALLOWED {
        assert_eq!(
            lines.get(*allowed).map(String::as_str),
            Some("ok"),
            "{allowed}"
        );
    }
    assert_eq!(
        lines.get("stdin_line").map(String::as_str),
        Some(REQUEST.trim())
    );
    let metal = lines.get("metal").map(String::as_str).unwrap_or("missing");
    assert!(metal.starts_with("ok"), "metal: {metal}");
}

/// The line the parent writes to the worker's stdin.
const REQUEST: &str = "{\"op\":\"shutdown\"}\n";

/// Runs the child with `apply` and returns its `name value` lines.
fn run(apply: Apply, font_folder: &str) -> BTreeMap<String, String> {
    let w = world(font_folder);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let _unix = std::os::unix::net::UnixListener::bind(&w.socket).unwrap();
    // A connected socket without close-on-exec: the child inherits it until `close_inherited`.
    let leaked = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let _accepted = listener.accept().unwrap();
    // SAFETY: clears close-on-exec on a descriptor this test owns.
    assert_eq!(
        unsafe { libc::fcntl(leaked.as_raw_fd(), libc::F_SETFD, 0) },
        0
    );
    let fonts = &w.profile.font_dirs[0];
    let spec = [
        (
            "apply",
            match apply {
                Apply::None => "none",
                Apply::Parameters => "parameters",
                Apply::Escaped => "escaped",
            }
            .to_owned(),
        ),
        ("session", text(&w.profile.session)),
        ("bundle", text(&w.profile.bundle)),
        ("fonts", text(fonts)),
        ("outside", text(&w.outside)),
        ("library", text(&w.library_file)),
        ("crafted", text(&w.crafted)),
        ("socket", text(&w.socket)),
        ("port", port.to_string()),
        ("leaked", leaked.as_raw_fd().to_string()),
    ];
    let mut command = Command::new(&w.exe);
    command
        .args([
            "--exact",
            CHILD,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
            "--",
            MARK,
        ])
        .args(spec.iter().map(|(k, v)| format!("{k}={v}")))
        // As `make::launch::worker_command` starts it: a cleared environment, in its session.
        .env_clear()
        .env("TMPDIR", w.profile.session.join("tmp"))
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .current_dir(&w.profile.session);
    let started = Instant::now();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The worker's requests arrive on stdin, once it is sandboxed.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(REQUEST.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    println!(
        "--- child ({}, {font_folder:?}) in {:?}\n{stdout}{}",
        spec[0].1,
        started.elapsed(),
        String::from_utf8_lossy(&out.stderr)
    );
    drop(leaked);
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix("probe "))
        .filter_map(|l| l.split_once(' '))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

fn text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}
