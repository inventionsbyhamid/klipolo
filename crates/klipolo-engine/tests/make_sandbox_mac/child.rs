//! The worker half of `make_sandbox_mac.rs`: closes what it inherited, sandboxes itself, then reports each probe on
//! stdout as `probe <name> <ok | refused …>`.

use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use klipolo_platform::process::sandbox::{self, Applied, SandboxProfile};

use super::MARK;
use super::metal::metal_frame;
use super::probes::{
    GLOBAL_NAME, XPC_SERVICE_NAME, clear_acl, first_file_in, head, is_open, policy, set_flags,
    task_port, udp_connect,
};

/// The worker, when the parent ran this binary with `MARK`; nothing otherwise.
#[test]
#[ignore = "Mac only: Seatbelt (the child half of the tests above)"]
pub fn sandboxed_child() {
    let args: Vec<String> = std::env::args().collect();
    let Some(at) = args.iter().position(|a| a == MARK) else {
        return;
    };
    println!(); // after libtest's "test child::sandboxed_child ... "
    let spec: BTreeMap<&str, &str> = args[at + 1..]
        .iter()
        .filter_map(|a| a.split_once('='))
        .collect();
    let path = |k: &str| PathBuf::from(spec[k]);
    let probe = |name: &str, result: std::io::Result<()>| match result {
        Ok(()) => println!("probe {name} ok"),
        Err(e) => println!("probe {name} refused {e}"),
    };
    let profile = SandboxProfile {
        session: path("session"),
        bundle: path("bundle"),
        font_dirs: vec![path("fonts")],
    };
    // Metal's shader cache, as the sandbox names it: this user's cache folder (the session when there is none).
    let metal = sandbox::parameters(&profile)
        .ok()
        .and_then(|values| values.into_iter().find(|(name, _)| *name == "METAL_CACHE"))
        .map(|(_, value)| PathBuf::from(value))
        .filter(|dir| *dir != profile.session);
    println!(
        "probe metal_cache_param {}",
        if metal.is_some() { "set" } else { "session" }
    );
    let leaked: libc::c_int = spec["leaked"].parse().unwrap();
    println!(
        "probe leaked_before {}",
        if is_open(leaked) { "open" } else { "closed" }
    );
    sandbox::close_inherited();
    println!(
        "probe leaked_after {}",
        if is_open(leaked) { "open" } else { "closed" }
    );
    let started = Instant::now();
    let applied = match spec["apply"] {
        "none" => Ok(Applied::Unsupported),
        "parameters" => sandbox::sandbox_self(&profile),
        _ => sandbox::sandbox_self_escaped(&profile),
    };
    println!("probe sandbox_call {applied:?} in {:?}", started.elapsed());
    match applied {
        Ok(Applied::Sandboxed) => println!("probe sandbox Sandboxed"),
        Ok(Applied::Unsupported) => {}
        Err(e) => {
            println!("probe sandbox failed {e}");
            return;
        }
    }
    let session = &profile.session;
    let port: u16 = spec["port"].parse().unwrap();
    probe(
        "tcp_loopback",
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2))
            .map(drop),
    );
    probe("tcp_listen", TcpListener::bind("127.0.0.1:0").map(drop));
    probe("udp_out", udp_connect([192, 0, 2, 1], 9));
    // `system.sb` allows this one socket; the profile's last rule refuses it.
    probe(
        "syslog_socket",
        std::os::unix::net::UnixDatagram::unbound()
            .and_then(|s| s.connect("/private/var/run/syslog")),
    );
    probe(
        "unix_socket",
        std::os::unix::net::UnixStream::connect(path("socket")).map(drop),
    );
    // The policy itself (`sandbox_check`: nothing is looked up). Metal's compiler service is the one XPC service it
    // reaches, and trustd (which fetches certificate URLs for its callers) is closed.
    probe(
        "xpc_compiler",
        policy(
            c"mach-lookup",
            XPC_SERVICE_NAME,
            c"com.apple.MTLCompilerService",
        ),
    );
    probe(
        "xpc_other",
        policy(
            c"mach-lookup",
            XPC_SERVICE_NAME,
            c"com.apple.WebKit.Networking",
        ),
    );
    probe(
        "trustd",
        policy(c"mach-lookup", GLOBAL_NAME, c"com.apple.trustd"),
    );
    probe("spawn", Command::new("/usr/bin/true").status().map(drop));
    // It can't reach its parent (no signal, no task port), nor the kernel's DTrace helper.
    // SAFETY: reads this process's parent id.
    let parent = unsafe { libc::getppid() };
    // SAFETY: signal 0 only asks whether the signal would be allowed.
    let signalled = unsafe { libc::kill(parent, 0) };
    probe(
        "signal_parent",
        if signalled == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        },
    );
    probe(
        "dtracehelper",
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/dtracehelper")
            .map(drop),
    );
    probe("task_name_parent", task_port(parent, true));
    probe("task_parent", task_port(parent, false));
    probe(
        "write_outside",
        std::fs::write(path("outside").with_extension("new"), b"x"),
    );
    probe("write_in", std::fs::write(session.join("in/new"), b"x"));
    probe("write_tmp", std::fs::write(session.join("tmp/new"), b"x"));
    // No chmod in its folders, so it can't lock the parent out of what it wrote.
    let mode = <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600);
    probe(
        "chmod_tmp",
        std::fs::set_permissions(session.join("tmp/new"), mode),
    );
    // No BSD flags and no ACL either (a harmless flag and an empty ACL: the control sets them).
    probe(
        "chflags_tmp",
        set_flags(&session.join("tmp/new"), libc::UF_NODUMP),
    );
    probe("acl_tmp", clear_acl(&session.join("tmp/new")));
    probe("write_out", std::fs::write(session.join("out/new"), b"x"));
    probe("read_outside", std::fs::read(path("outside")).map(drop));
    probe("read_library", std::fs::read(path("library")).map(drop));
    probe("read_crafted", std::fs::read(path("crafted")).map(drop));
    probe(
        "read_in",
        std::fs::read(session.join("in/still.jpg")).map(drop),
    );
    probe(
        "read_bundle_font",
        head(
            &profile
                .bundle
                .join("Contents/Resources/fonts/Anton-Regular.ttf"),
        ),
    );
    probe(
        "read_user_font",
        head(&path("fonts").join("Anton-Regular.ttf")),
    );
    probe(
        "read_system_font",
        first_file_in(Path::new("/System/Library/Fonts")),
    );
    probe(
        "read_library_fonts",
        first_file_in(Path::new("/Library/Fonts")),
    );
    // Read only: the worker shares the cache with the app, so it must never write there. The control child
    // doesn't try the write: it would change the person's real cache.
    if let Some(metal) = &metal {
        let cache = metal.join("com.apple.metal");
        probe("read_metal_cache", std::fs::read_dir(&cache).map(drop));
        if spec["apply"] == "none" {
            println!("probe write_metal_cache skipped");
        } else {
            let probe_file = cache.join(format!("klipolo-sandbox-probe-{}", std::process::id()));
            probe("write_metal_cache", std::fs::write(&probe_file, b"x"));
            let _ = std::fs::remove_file(&probe_file);
        }
    }
    println!("probe stdin_line {}", stdin_line());
    match metal_frame() {
        Ok(done) => println!("probe metal ok {done}"),
        Err(e) => println!("probe metal failed {e}"),
    }
    if let Some(m) = klipolo_platform::process::memory(std::process::id()) {
        println!(
            "probe memory footprint {} MiB, resident {} MiB",
            m.footprint >> 20,
            m.resident >> 20
        );
    }
}

/// One line from stdin, read on a tokio runtime's blocking pool as the worker's loop would (its threads, timers and
/// kqueue start after the sandbox).
fn stdin_line() -> String {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build();
    let Ok(runtime) = runtime else {
        return "no runtime".into();
    };
    runtime.block_on(async {
        tokio::time::sleep(Duration::from_millis(1)).await;
        let read = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map(|_| line)
        });
        match read.await {
            Ok(Ok(line)) => line.trim().to_owned(),
            Ok(Err(e)) => format!("refused {e}"),
            Err(e) => format!("failed {e}"),
        }
    })
}
