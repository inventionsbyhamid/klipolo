//! The profile builder's tests (every OS). Nothing here sandboxes the test process: the Mac proof that the profile
//! holds is `klipolo-engine/tests/make_sandbox_mac.rs`, in a child process.

use super::*;

/// A canonical temporary folder (macOS: `/var/folders/…` is `/private/var/folders/…`), with `name` made in it.
fn folder(root: &tempfile::TempDir, name: &str) -> PathBuf {
    let dir = std::fs::canonicalize(root.path()).unwrap().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn profile(root: &tempfile::TempDir) -> SandboxProfile {
    SandboxProfile {
        session: folder(root, "session"),
        bundle: folder(root, "Klipolo.app"),
        font_dirs: vec![folder(root, "fonts")],
    }
}

#[test]
fn the_builder_refuses_a_relative_dotted_linked_or_control_character_path() {
    let root = tempfile::tempdir().unwrap();
    let good = profile(&root);
    let with = |change: &dyn Fn(&mut SandboxProfile)| {
        let mut p = good.clone();
        change(&mut p);
        parameters(&p)
    };
    assert!(with(&|p| p.session = "session".into()).is_err(), "relative");
    assert!(
        with(&|p| p.bundle = good.session.join("..").join("Klipolo.app")).is_err(),
        ".."
    );
    assert!(
        with(&|p| p.bundle = folder(&root, "Klipolo\n.app")).is_err(),
        "a newline"
    );
    assert!(
        with(&|p| p.font_dirs = vec![folder(&root, "fonts\u{7}")]).is_err(),
        "a bell"
    );
    assert!(
        with(&|p| p.font_dirs = vec!["fonts".into()]).is_err(),
        "a relative font folder"
    );
    let session = good.session.to_str().unwrap();
    let trailing = PathBuf::from(format!("{session}/"));
    assert!(
        with(&|p| p.session = trailing.clone()).is_err(),
        "a trailing /"
    );
    let dotted = good.session.parent().unwrap().join(".").join("session");
    assert!(with(&|p| p.session = dotted.clone()).is_err(), "a /./");
    #[cfg(unix)]
    {
        let link = good.session.parent().unwrap().join("link");
        std::os::unix::fs::symlink(&good.session, &link).unwrap();
        assert!(
            with(&|p| p.session = link.clone()).is_err(),
            "a link on the way"
        );
    }
    let four = vec![good.font_dirs[0].clone(); FONT_SLOTS + 1];
    assert!(
        with(&|p| p.font_dirs = four.clone()).is_err(),
        "more font folders than slots"
    );
}

#[test]
fn every_parameter_is_set_and_an_empty_or_missing_font_slot_is_the_session() {
    let root = tempfile::tempdir().unwrap();
    let mut p = profile(&root);
    let quoted = folder(&root, "fonts\") (allow network*)");
    p.font_dirs.push(missing_folder(&root));
    p.font_dirs[0] = quoted.clone();
    let values = parameters(&p).unwrap();
    let names: Vec<_> = values.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, PARAMETERS);
    let session = p.session.to_str().unwrap();
    let text = |name| values.iter().find(|(n, _)| *n == name).unwrap().1.as_str();
    assert_eq!(text("SESSION"), session);
    assert_eq!(text("BUNDLE"), p.bundle.to_str().unwrap());
    assert_eq!(
        text("FONTS_0"),
        quoted.to_str().unwrap(),
        "a quote and a bracket are a value"
    );
    assert_eq!(
        text("FONTS_1"),
        session,
        "a folder that doesn't exist names nothing"
    );
    assert_eq!(text("FONTS_2"), session, "an empty slot");
    // Metal's shader cache: this user's own cache folder, found by the process itself (never a value from the profile),
    // else the session.
    let metal = text("METAL_CACHE");
    #[cfg(not(target_os = "macos"))]
    assert_eq!(metal, session, "no Metal cache here");
    #[cfg(target_os = "macos")]
    assert!(
        metal == session
            || std::fs::canonicalize(metal)
                .is_ok_and(|c| c.as_os_str() == std::ffi::OsStr::new(metal)),
        "{metal}"
    );
}

/// The Metal cache is shared with the app (whose executable the worker is): the worker reads it, never writes it, so a
/// compromised worker can't poison the shaders the app loads. The Mac test proves the
/// kernel refuses the write.
#[test]
fn the_profile_reads_the_metal_cache_and_never_writes_it() {
    assert!(PARAMETERS.contains(&"METAL_CACHE"));
    let rules: Vec<&str> = PROFILE.split("\n(").collect();
    let naming: Vec<&str> = rules
        .iter()
        .copied()
        .filter(|r| r.contains("(param \"METAL_CACHE\")"))
        .collect();
    assert!(!naming.is_empty());
    for rule in naming {
        assert!(
            rule.starts_with("allow file-read* ") || rule.starts_with("allow file-read-metadata "),
            "{rule}"
        );
    }
}

/// A canonical path to a folder that doesn't exist.
fn missing_folder(root: &tempfile::TempDir) -> PathBuf {
    std::fs::canonicalize(root.path()).unwrap().join("not-yet")
}

#[test]
fn the_profile_has_no_variables_but_its_parameters() {
    let mut named = Vec::new();
    let mut rest = PROFILE;
    while let Some(at) = rest.find("(param ") {
        let after = &rest[at + "(param ".len()..];
        let name = after
            .strip_prefix('"')
            .and_then(|s| s.split_once("\")"))
            .unwrap()
            .0;
        named.push(name);
        rest = after;
    }
    named.sort_unstable();
    named.dedup();
    let mut expected = PARAMETERS.to_vec();
    expected.sort_unstable();
    assert_eq!(named, expected);
    // No other way for a value to reach the text: no template marks, no other definitions.
    for mark in ["{", "}", "%", "$", "(define", "(param-", "(getenv"] {
        assert!(!PROFILE.contains(mark), "{mark}");
    }
}

#[test]
fn the_escaping_path_escapes_quotes_and_backslashes_and_refuses_control_characters() {
    let crafted = "/tmp/fonts\") (allow network*) (allow file-read* (subpath \"/\"))";
    let mut values: Vec<(&str, String)> =
        PARAMETERS.iter().map(|n| (*n, format!("/s/{n}"))).collect();
    values[2].1 = crafted.into();
    values[3].1 = "/back\\slash\\\"".into();
    let text = escaped_profile(&values).unwrap();
    assert!(!text.contains("(param "), "every parameter replaced");
    assert!(text.contains(
        r#"(subpath "/tmp/fonts\") (allow network*) (allow file-read* (subpath \"/\"))")"#
    ));
    assert!(text.contains(r#"(subpath "/back\\slash\\\"")"#));
    assert!(text.contains(r#"(subpath (string-append "/s/SESSION" "/tmp"))"#));
    // Outside string literals the text is the fixed profile's: the crafted value added no rule.
    assert_eq!(
        outside_strings(&text),
        outside_strings(PROFILE).replace("(param )", "")
    );
    values[4].1 = "/fonts\u{0}x".into();
    assert!(escaped_profile(&values).is_err(), "a NUL");
    values[4].1 = "/fonts\rx".into();
    assert!(escaped_profile(&values).is_err(), "a carriage return");
    assert!(
        escaped_profile(&values[..4]).is_err(),
        "a parameter without a value"
    );
}

/// `text` without its string literals (`\` escapes inside them), as a Scheme reader splits it.
fn outside_strings(text: &str) -> String {
    let (mut out, mut inside, mut escaped) = (String::new(), false, false);
    for c in text.chars() {
        match (inside, escaped, c) {
            (true, true, _) => escaped = false,
            (true, false, '\\') => escaped = true,
            (_, false, '"') => inside = !inside,
            (false, _, c) => out.push(c),
            (true, false, _) => {}
        }
    }
    out
}

#[test]
fn the_fake_reports_sandboxed_unless_a_test_sets_unsupported() {
    use crate::fake::sandbox::{sandbox_self, set_applied};
    let root = tempfile::tempdir().unwrap();
    let p = profile(&root);
    assert_eq!(sandbox_self(&p), Ok(Applied::Sandboxed));
    set_applied(Applied::Unsupported);
    assert_eq!(sandbox_self(&p), Ok(Applied::Unsupported));
    set_applied(Applied::Sandboxed);
    assert!(
        sandbox_self(&SandboxProfile {
            session: "rel".into(),
            ..p
        })
        .is_err()
    );
}

/// Linux and Windows have no worker sandbox yet: a valid profile is `Unsupported` (never on macOS, where this would
/// sandbox the test process).
#[cfg(not(target_os = "macos"))]
#[test]
fn without_a_worker_sandbox_a_valid_profile_is_unsupported() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        super::sandbox_self(&profile(&root)),
        Ok(Applied::Unsupported)
    );
}

/// `system.sb` lets every process reach the kernel's DTrace helper (its DOF parser); the profile
/// closes it after the import (a later rule with a filter wins), as it closes the syslog socket.
#[test]
fn the_profile_closes_the_dtrace_helper_after_the_import() {
    let import = PROFILE.find("(import \"system.sb\")").unwrap();
    let deny = PROFILE
        .find("(deny file-read* file-test-existence file-write* file-ioctl (literal \"/dev/dtracehelper\"))")
        .expect("closed");
    assert!(deny > import);
}

/// The worker can't change a mode, BSD flags or an ACL in the folders it writes
/// (one deny after their allow), so it can't lock the parent out of what it wrote. What it can still give at creation,
/// the parent's walk fails closed on and its removal undoes.
#[test]
fn the_profile_refuses_mode_flags_and_acl_changes_in_the_folders_it_writes() {
    let allow = PROFILE.find("(allow file-write* ").unwrap();
    let deny = PROFILE
        .find(
            "(deny file-write-mode file-write-flags file-write-acl \
             (subpath (string-append (param \"SESSION\") \"/tmp\")) \
             (subpath (string-append (param \"SESSION\") \"/out\")))",
        )
        .expect("refused");
    assert!(deny > allow);
}

/// The app's soft open-files limit goes up to min(hard, 4096) at start, so a deep tree or a busy
/// moment doesn't run it out of descriptors (launchd gives 256).
#[cfg(unix)]
#[test]
fn the_open_files_limit_is_raised() {
    let raised = crate::process::raise_open_files(4096).unwrap();
    let mut now = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: reads this process's own limit into a value on the stack.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut now) }, 0);
    assert_eq!(now.rlim_cur, raised);
    assert!(raised >= now.rlim_max.min(4096), "{raised}");
}
