//! The render worker's entry (`klipolo --render-worker <session>`), in a child process as the app starts it:
//! a cleared environment, its session as the working directory, its profile in `<session>/in/profile.json`. The child
//! is this test binary, which calls `make::worker::main` (the app's `main.rs` does the same for `--render-worker`).
//! On macOS the child sandboxes itself (Seatbelt; `make_sandbox_mac.rs` proves what it refuses), elsewhere it says
//! `sandboxed: false`. A profile for another session ends it before it reads anything, with exit code 5.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use klipolo_engine::make::protocol::{WireKind, WorkerReply, WorkerRequest};
use klipolo_engine::make::session::Session;
use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_platform::process::sandbox::SandboxProfile;

const MARK: &str = "--klipolo-worker-child";
/// The watchdog test's middle process: it starts the worker child, says its id and exits at once.
const MIDDLE: &str = "--klipolo-worker-middle";

/// The worker, when the parent ran this binary with `MARK <session>`; nothing otherwise.
#[test]
fn worker_child() {
    let args: Vec<String> = std::env::args().collect();
    let Some(at) = args.iter().position(|a| a == MARK) else {
        return;
    };
    println!(); // ends libtest's "test worker_child ... " line: the replies start on their own lines
    std::process::exit(klipolo_engine::make::worker::main(&args[at + 1..]));
}

/// The middle process, when the watchdog test ran this binary with `MIDDLE <session>`: it starts the worker with its
/// own stdin and stdout (the test's pipes) and its stderr piped to itself, as the app pipes it, and exits without
/// waiting or reading it: the worker is orphaned and its stderr's reader is gone.
#[test]
fn middle_child() {
    let args: Vec<String> = std::env::args().collect();
    let Some(at) = args.iter().position(|a| a == MIDDLE) else {
        return;
    };
    let exe = std::env::current_exe().unwrap();
    let worker = worker_command(&exe, Path::new(&args[at + 1]))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    eprintln!("worker {}", worker.id());
    // `worker`'s stderr pipe closes with this process.
    std::process::exit(0);
}

#[test]
fn the_worker_answers_from_its_session_and_refuses_another_sessions_profile() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let session = Session::create(&root).unwrap();
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let profile = SandboxProfile {
        session: session.dir().to_owned(),
        bundle: exe.parent().unwrap().to_owned(),
        font_dirs: vec![],
    };
    session.write_profile(&profile).unwrap();
    let def = ContribDef::new("lib.k3f9q2ma/generator.t.v1", ContribKind::Generator, "T");
    let requests = [
        WorkerRequest::Validate {
            id: 1,
            kind: WireKind::Generator,
            def,
            body: "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }"
                .into(),
        },
        WorkerRequest::Shutdown,
    ];
    let (code, replies, log) = run(&exe, session.dir(), &requests);
    assert_eq!(code, Some(0), "{replies:?}");
    // RLIMIT_FSIZE, read by the worker itself (`getrlimit`): the Mac's check of the file-size cap too.
    #[cfg(unix)]
    assert!(
        log.contains(&format!(
            "files capped at {} bytes",
            klipolo_engine::make::worker::MAX_FILE_BYTES
        )),
        "{log}"
    );
    assert!(
        matches!(replies[0], WorkerReply::Ready { sandboxed, .. } if sandboxed == cfg!(target_os = "macos")),
        "{replies:?}"
    );
    assert!(matches!(
        replies[1],
        WorkerReply::Validated {
            id: 1,
            ok: true,
            ..
        }
    ));

    let other = Session::create(&root).unwrap();
    other.write_profile(&profile).unwrap();
    let (code, replies, _) = run(&exe, other.dir(), &requests);
    assert_eq!(code, Some(5), "can't start safely");
    assert!(
        replies.is_empty(),
        "nothing before the sandbox: {replies:?}"
    );
}

/// This test binary as the worker child (`MARK`) in `session`, as the app starts a worker: a cleared environment, in
/// its session, every stream piped.
fn worker_command(exe: &Path, session: &Path) -> Command {
    child_command(exe, "worker_child", MARK, session)
}

/// This test binary running only its test `name`, given `mark` and `session`.
fn child_command(exe: &Path, name: &str, mark: &str, session: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .args([
            "--exact",
            name,
            "--nocapture",
            "--test-threads=1",
            // libtest takes what follows `--` as test names (matching nothing), so it doesn't refuse the mark.
            "--",
            mark,
        ])
        .arg(session)
        .env_clear()
        .env("TMPDIR", session.join("tmp"))
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .current_dir(session)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Linux CI finds the FFmpeg prefix's libraries this way (the test binary links them).
    if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
        command.env("LD_LIBRARY_PATH", path);
    }
    command
}

/// A session under `root` with its profile written, as the app makes one before a launch.
fn session_with_profile(root: &Path) -> Session {
    let session = Session::create(root).unwrap();
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    session
        .write_profile(&SandboxProfile {
            session: session.dir().to_owned(),
            bundle: exe.parent().unwrap().to_owned(),
            font_dirs: vec![],
        })
        .unwrap();
    session
}

/// A worker that outlives the app exits within 1 s, even while its stdin stays open (a
/// crashed app's pipe can outlive it in another process). The middle process starts the worker on the test's pipes and
/// exits; the worker's stdout then closes once its watchdog sees its new parent. A worker still there after 5 s is
/// killed, so the test fails rather than hang.
#[cfg(unix)]
#[test]
fn a_worker_whose_parent_is_gone_exits_within_a_second() {
    use std::io::{BufRead, Read};

    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let session = session_with_profile(&root);
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    let mut middle = child_command(&exe, "middle_child", MIDDLE, session.dir())
        .spawn()
        .unwrap();
    // The test keeps the worker's stdin open all along: only its parent's exit can stop it.
    let _stdin = middle.stdin.take().unwrap();
    let mut stdout = middle.stdout.take().unwrap();
    let pid: i32 = std::io::BufReader::new(middle.stderr.take().unwrap())
        .lines()
        .map_while(Result::ok)
        .find_map(|l| {
            l.strip_prefix("worker ")
                .and_then(|p| p.trim().parse().ok())
        })
        .expect("the middle names its worker");
    assert!(middle.wait().unwrap().success());
    let exited = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut all = Vec::new();
        let _ = stdout.read_to_end(&mut all);
        let _ = tx.send(all);
    });
    let out = rx.recv_timeout(std::time::Duration::from_secs(5));
    let took = exited.elapsed();
    if out.is_err() {
        // SAFETY: signals the worker this test started.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    let all = out.expect("the orphaned worker never exited");
    assert!(
        String::from_utf8_lossy(&all).contains("\"ready\""),
        "the worker started: {}",
        String::from_utf8_lossy(&all)
    );
    assert!(
        took < std::time::Duration::from_millis(1500),
        "the orphaned worker ran on for {took:?}"
    );
}

/// Runs the worker child in `session` with `requests` on stdin; its exit code, its reply lines and its log.
fn run(
    exe: &Path,
    session: &Path,
    requests: &[WorkerRequest],
) -> (Option<i32>, Vec<WorkerReply>, String) {
    let mut child = worker_command(exe, session).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        // A worker that refused to start has closed its stdin: the write may fail, harmlessly.
        let _ = writeln!(stdin, "{}", serde_json::to_string(request).unwrap());
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let log = String::from_utf8_lossy(&out.stderr).into_owned();
    eprintln!("{log}");
    let replies = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (out.status.code(), replies, log)
}

/// Before its sandbox the worker caps the size of any file it writes at the disk cap
/// (`RLIMIT_FSIZE`, soft and hard), so one file can't pass the cap between two of the parent's polls. Read from
/// `/proc` while the worker waits for a request.
#[cfg(target_os = "linux")]
#[test]
fn the_worker_caps_the_size_of_a_file_it_writes() {
    use std::io::BufRead;

    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let session = Session::create(&root).unwrap();
    let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    session
        .write_profile(&SandboxProfile {
            session: session.dir().to_owned(),
            bundle: exe.parent().unwrap().to_owned(),
            font_dirs: vec![],
        })
        .unwrap();
    let mut command = Command::new(&exe);
    command
        .args([
            "--exact",
            "worker_child",
            "--nocapture",
            "--test-threads=1",
            "--",
            MARK,
        ])
        .arg(session.dir())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(session.dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
        command.env("LD_LIBRARY_PATH", path);
    }
    let mut child = command.spawn().unwrap();
    let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let ready = lines
        .by_ref()
        .map(Result::unwrap)
        .find(|l| l.starts_with('{'))
        .expect("Ready");
    assert!(ready.contains("\"ready\""), "{ready}");
    let limits = std::fs::read_to_string(format!("/proc/{}/limits", child.id())).unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        serde_json::to_string(&WorkerRequest::Shutdown).unwrap()
    )
    .unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let line = limits
        .lines()
        .find(|l| l.starts_with("Max file size"))
        .unwrap();
    let cap = klipolo_engine::make::worker::MAX_FILE_BYTES.to_string();
    assert_eq!(
        line.split_whitespace().filter(|w| *w == cap).count(),
        2,
        "{line}"
    );
}
