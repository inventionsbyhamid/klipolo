//! The render worker's protocol and loop: JSON lines of bytes, never paths, each line
//! capped; the worker's profile comes only from its own session; leftover sessions are swept.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use klipolo_engine::make::protocol::{
    MAX_BLOB_BYTES, WireKind, WorkerReply, WorkerRequest, fit_jpeg, reencode_jpeg,
};
use klipolo_engine::make::worker::serve;
use klipolo_model::contrib::{ContribDef, ContribKind};

#[tokio::test]
async fn validate_answers_without_a_gpu_and_shutdown_ends_the_loop() {
    let def = ContribDef::new("lib.k3f9q2ma/generator.t.v1", ContribKind::Generator, "T");
    let reqs = [
        WorkerRequest::Validate {
            id: 1,
            kind: WireKind::Generator,
            def: def.clone(),
            body: "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }"
                .into(),
        },
        WorkerRequest::Validate {
            id: 2,
            kind: WireKind::Generator,
            def,
            body: "fn effect(".into(),
        },
        WorkerRequest::Shutdown,
    ];
    let input = reqs
        .iter()
        .map(|r| serde_json::to_string(r).unwrap() + "\n")
        .collect::<String>();
    let mut out = Vec::new();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(serve(input.as_bytes(), &mut out, dir.path()).await, 0);
    let replies: Vec<WorkerReply> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(matches!(replies[0], WorkerReply::Ready { .. }));
    assert!(matches!(
        &replies[1],
        WorkerReply::Validated {
            id: 1,
            ok: true,
            ..
        }
    ));
    assert!(
        matches!(&replies[2], WorkerReply::Validated { id: 2, ok: false, diagnostics, .. } if !diagnostics.is_empty())
    );
}

#[tokio::test]
async fn an_oversize_request_is_refused_and_the_worker_goes_on() {
    let big = format!(
        "{{\"op\":\"validate\",\"id\":3,\"kind\":\"generator\",\"def\":{{}},\"body\":\"{}\"}}\n",
        "x".repeat(2 << 20)
    );
    let mut out = Vec::new();
    let dir = tempfile::tempdir().unwrap();
    let input = big + &serde_json::to_string(&WorkerRequest::Shutdown).unwrap() + "\n";
    assert_eq!(serve(input.as_bytes(), &mut out, dir.path()).await, 0);
    assert!(String::from_utf8(out).unwrap().contains("\"failed\""));
}

#[test]
fn input_names_never_leave_the_sessions_in_folder() {
    use klipolo_engine::make::protocol::InputName;
    for bad in ["../x", "a/b", "/etc/hosts", "", ".", ".."] {
        assert!(InputName::new(bad).is_err(), "{bad}");
    }
    assert!(InputName::new("project.json").is_ok());
    // The worker's own profile isn't an input the parent can write over.
    assert!(InputName::new("profile.json").is_err());
}

/// A field the protocol doesn't have (a path, say) is refused, in a nested value too; a blob is capped decoded.
#[test]
fn a_reply_carries_no_field_it_lacks_and_no_blob_over_the_cap() {
    for line in [
        r#"{"op":"compiled","id":3,"ok":true,"diagnostics":[],"path":"/etc/hosts"}"#,
        r#"{"op":"ready","version":"1","sandboxed":true,"path":"/"}"#,
        r#"{"op":"validated","id":3,"ok":false,"diagnostics":[{"severity":"error","line":1,"column":1,"code":"scan.size","message":"m","path":"/etc/hosts"}]}"#,
        r#"{"op":"validated","id":3,"ok":false,"diagnostics":[{"severity":"error","line":1,"column":1,"code":"../../etc","message":"m"}]}"#,
    ] {
        assert!(serde_json::from_str::<WorkerReply>(line).is_err(), "{line}");
    }
    let thumbnail = |n: usize| {
        format!(
            r#"{{"op":"thumbnail","id":1,"jpeg":"{}"}}"#,
            BASE64.encode(vec![0u8; n])
        )
    };
    assert!(serde_json::from_str::<WorkerReply>(&thumbnail(MAX_BLOB_BYTES)).is_ok());
    assert!(serde_json::from_str::<WorkerReply>(&thumbnail(MAX_BLOB_BYTES + 1)).is_err());
}

/// The worker fits every image to the cap (quality, then size); the parent takes only a JPEG within the
/// size it asked for and encodes it again itself.
#[test]
fn every_image_fits_a_reply_and_the_parent_takes_only_jpegs_it_asked_for() {
    // Noise barely compresses: at 1024 × 768 it is far over the cap at the first quality.
    let mut seed = 7u32;
    let noise = image::RgbImage::from_fn(1024, 768, |_, _| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        image::Rgb([(seed >> 24) as u8, (seed >> 16) as u8, (seed >> 8) as u8])
    });
    let jpeg = fit_jpeg(&noise);
    assert!(jpeg.len() <= MAX_BLOB_BYTES, "{}", jpeg.len());
    let again = reencode_jpeg(&jpeg, 1024).unwrap();
    let decoded = image::load_from_memory_with_format(&again, image::ImageFormat::Jpeg).unwrap();
    assert!(decoded.width() <= 1024 && decoded.height() <= 768);
    assert!(
        reencode_jpeg(&jpeg, 256).is_err(),
        "larger than the parent asked"
    );
    assert!(reencode_jpeg(b"/etc/hosts", 1024).is_err(), "not a JPEG");
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    assert!(reencode_jpeg(&png, 1024).is_err(), "a PNG");
}

/// The worker reads its sandbox's paths from its own session's `in/profile.json`, which the parent wrote, and from no
/// other session; a missing or oversize profile is refused (the worker then exits before it reads a request).
#[test]
fn the_worker_takes_its_profile_only_from_its_own_session() {
    use klipolo_engine::make::sandbox::read_profile;
    use klipolo_engine::make::session::Session;
    use klipolo_platform::process::sandbox::SandboxProfile;
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let session = Session::create(&root).unwrap();
    let other = Session::create(&root).unwrap();
    assert!(read_profile(session.dir()).is_err(), "no profile yet");
    let mine = SandboxProfile {
        session: session.dir().to_owned(),
        bundle: root.clone(),
        font_dirs: vec![],
    };
    session.write_profile(&mine).unwrap();
    assert_eq!(read_profile(session.dir()).unwrap(), mine);
    other
        .write_profile(&SandboxProfile {
            session: session.dir().to_owned(),
            ..mine.clone()
        })
        .unwrap();
    assert!(
        read_profile(other.dir()).is_err(),
        "another session's profile"
    );
    std::fs::write(other.dir().join("in/profile.json"), vec![b' '; 64 * 1024]).unwrap();
    assert!(read_profile(other.dir()).is_err(), "oversize");
}

/// The parent deletes the sessions an earlier run left (its process is gone) at the next start, and never a live
/// run's.
#[test]
fn leftover_sessions_are_swept_and_live_ones_kept() {
    use klipolo_engine::make::session::{Session, sweep};
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let live = Session::create(&root).unwrap();
    let gone = root.join("0-1");
    std::fs::create_dir_all(gone.join("in")).unwrap();
    std::fs::write(gone.join("in/project.json"), b"{}").unwrap();
    sweep(&root);
    assert!(!gone.exists(), "a session whose process is gone");
    assert!(live.dir().join("in").is_dir(), "this run's");
    live.remove();
    assert!(!root.join(live.dir().file_name().unwrap()).exists());
}

/// A worker an earlier run left (the app crashed or quit without stopping it) is stopped at the next
/// start, by the pid, start time and boot recorded in its session; then its session goes.
#[cfg(unix)]
#[test]
fn a_leftover_worker_is_stopped_at_the_next_start() {
    use klipolo_engine::make::session::{record_worker, sweep};
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let gone = root.join("0-1");
    std::fs::create_dir_all(gone.join("in")).unwrap();
    let mut worker = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    record_worker(&gone, worker.id()).unwrap();
    sweep(&root);
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = worker.try_wait().unwrap() {
            break status;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "still running"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(!status.success(), "killed");
    assert!(!gone.exists());
}

/// A folder the worker made unreadable doesn't keep its session on disk: once
/// the worker is gone (its watcher saw it exit; a leftover's recorded worker isn't running), the parent restores its
/// own access (never following a link) and deletes it. A leftover with no record of its worker is only deleted as it
/// is: nothing is unlocked for a worker that may still run.
#[cfg(unix)]
#[test]
fn a_session_with_an_unreadable_folder_is_still_deleted() {
    use std::os::unix::fs::PermissionsExt;

    use klipolo_engine::make::session::{Session, WORKER_FILE, sweep};
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let lock = |session: &std::path::Path| {
        let locked = session.join("tmp/locked");
        std::fs::create_dir_all(locked.join("inner")).unwrap();
        std::fs::write(locked.join("inner/hidden"), b"x").unwrap();
        std::fs::set_permissions(locked.join("inner"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    };
    let session = Session::create(&root).unwrap();
    lock(session.dir());
    session.remove();
    assert!(!session.dir().exists(), "removed when the worker stops");
    // A leftover whose recorded worker is gone (a pid that isn't running, this boot).
    let leftover = root.join("0-1");
    std::fs::create_dir_all(leftover.join("tmp")).unwrap();
    lock(&leftover);
    let boot = klipolo_platform::process::boot_id().unwrap();
    let record = serde_json::json!({ "pid": u32::MAX - 1, "started": 1, "boot": boot });
    std::fs::write(leftover.join(WORKER_FILE), record.to_string()).unwrap();
    // A leftover with no record.
    let unrecorded = root.join("0-2");
    std::fs::create_dir_all(unrecorded.join("tmp")).unwrap();
    lock(&unrecorded);
    sweep(&root);
    assert!(!leftover.exists(), "swept");
    assert!(
        unrecorded.join("tmp/locked").exists(),
        "left as it is: not unlocked"
    );
    std::fs::set_permissions(
        unrecorded.join("tmp/locked"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::set_permissions(
        unrecorded.join("tmp/locked/inner"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
}
