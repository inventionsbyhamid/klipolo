//! The parent's removal of a worker's session against what Seatbelt can't stop (`make_sandbox_mac.rs`'s module).

use std::process::Command;

/// A worker's session goes even with what Seatbelt can't stop it giving at creation, or what an
/// earlier build let it set: a `uchg` file and a folder whose ACL denies deleting what is in it. No sandbox: the parent's
/// own removal, once the worker is gone. Made with the system's `chflags` and `chmod +a`.
#[test]
#[ignore = "Mac only: BSD flags and ACLs"]
fn flags_and_acls_dont_keep_a_session() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let session = klipolo_engine::make::session::Session::create(&root).unwrap();
    let tmp = session.dir().join("tmp");
    std::fs::write(tmp.join("locked"), b"x").unwrap();
    std::fs::create_dir(tmp.join("guarded")).unwrap();
    std::fs::write(tmp.join("guarded/inside"), b"x").unwrap();
    let run = |program: &str, args: &[&str]| {
        assert!(
            Command::new(program).args(args).status().unwrap().success(),
            "{program} {args:?}"
        );
    };
    run(
        "/usr/bin/chflags",
        &["uchg", tmp.join("locked").to_str().unwrap()],
    );
    run(
        "/bin/chmod",
        &[
            "+a",
            "everyone deny delete",
            tmp.join("guarded/inside").to_str().unwrap(),
        ],
    );
    run(
        "/bin/chmod",
        &[
            "+a",
            "everyone deny delete_child",
            tmp.join("guarded").to_str().unwrap(),
        ],
    );
    assert!(
        std::fs::remove_file(tmp.join("locked")).is_err(),
        "the flag holds"
    );
    session.remove();
    assert!(!session.dir().exists());
}
