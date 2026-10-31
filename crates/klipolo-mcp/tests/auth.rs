//! Who can reach Klipolo's MCP server: the connection file only this user can read, the token of each run, and the
//! HTTP server on 127.0.0.1 that refuses another program on the Mac (no token or a wrong one, a browser page, DNS
//! rebinding, a huge or slow body, a flood of connections or sessions) before it reads a body, and stays answering.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use klipolo_mcp::connection::{self, ConnectionFile, FILE, VERSION, new_token, support_dir_from};

fn is_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[test]
fn new_token_is_64_hex_digits_and_new_each_time() {
    let (a, b) = (new_token(), new_token());
    assert!(is_token(&a), "{a}");
    assert!(is_token(&b), "{b}");
    assert_ne!(a, b);
}

#[test]
fn connection_file_is_private_and_reads_back() {
    let temp = tempfile::tempdir().unwrap();
    // The folder is made when needed.
    let dir = temp.path().join("Klipolo");
    let file = ConnectionFile {
        port: 51234,
        token: new_token(),
        pid: std::process::id(),
        version: VERSION,
    };
    let path = connection::write(&dir, &file).unwrap();
    assert_eq!(path, dir.join(FILE));
    assert_eq!(FILE, "mcp.json");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"port": 51234, "token": file.token, "pid": std::process::id(), "version": 1})
    );
    assert_eq!(connection::read(&dir), Some(file.clone()));
    // The token never shows in a log line.
    assert!(!format!("{file:?}").contains(&file.token));

    // A file left by a Klipolo that crashed is replaced.
    let again = ConnectionFile {
        port: 51235,
        token: new_token(),
        ..file.clone()
    };
    connection::write(&dir, &again).unwrap();
    assert_eq!(connection::read(&dir), Some(again));
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    // A file of another format, or none, reads as none.
    std::fs::remove_file(&path).unwrap();
    std::fs::write(
        &path,
        r#"{"port": 1, "token": "x", "pid": 2, "version": 2}"#,
    )
    .unwrap();
    assert_eq!(connection::read(&dir), None);
    std::fs::write(&path, "not json").unwrap();
    assert_eq!(connection::read(&dir), None);
    connection::remove(&dir);
    assert!(!path.exists());
    assert_eq!(connection::read(&dir), None);
    // Removing it again is fine.
    connection::remove(&dir);
}

#[test]
fn connection_file_is_never_written_through_a_link() {
    let temp = tempfile::tempdir().unwrap();
    let elsewhere = temp.path().join("elsewhere.json");
    std::fs::write(&elsewhere, "kept").unwrap();
    std::os::unix::fs::symlink(&elsewhere, temp.path().join(FILE)).unwrap();
    let file = ConnectionFile {
        port: 51234,
        token: new_token(),
        pid: 1,
        version: VERSION,
    };
    // The link goes, not what it points at; the new file is a file of its own.
    connection::write(temp.path(), &file).unwrap();
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "kept");
    let meta = std::fs::symlink_metadata(temp.path().join(FILE)).unwrap();
    assert!(meta.file_type().is_file());
}

#[test]
fn support_dir_is_the_variable_else_application_support() {
    let os = |s: &str| Some(OsString::from(s));
    assert_eq!(
        support_dir_from(os("/tmp/klipolo-test"), os("/Users/someone")),
        Some(PathBuf::from("/tmp/klipolo-test"))
    );
    assert_eq!(
        support_dir_from(None, os("/Users/someone")),
        Some(PathBuf::from(
            "/Users/someone/Library/Application Support/Klipolo"
        ))
    );
    // Set but empty counts as unset.
    assert_eq!(
        support_dir_from(os(""), os("/Users/someone")),
        Some(PathBuf::from(
            "/Users/someone/Library/Application Support/Klipolo"
        ))
    );
    assert_eq!(support_dir_from(None, None), None);
    assert_eq!(connection::SUPPORT_DIR_VAR, "KLIPOLO_SUPPORT_DIR");
}
