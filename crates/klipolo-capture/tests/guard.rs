#[test]
#[should_panic(expected = "real capture in a test")]
fn a_real_capture_call_panics_in_tests() {
    // SAFETY: libtest may run this file's tests on parallel threads, but they reach the environment only through std's
    // `set_var` and `var_os`, which hold std's environment lock, and no C code reads it meanwhile. Both set the same
    // value, which every test command has set already.
    unsafe { std::env::set_var("KLIPOLO_TEST", "1") };
    klipolo_capture::guard::assert_real_capture_allowed("screen");
}

#[cfg(feature = "real-capture")]
#[test]
#[should_panic(expected = "real capture in a test")]
fn the_real_backend_refuses_before_touching_the_os() {
    // SAFETY: as above.
    unsafe { std::env::set_var("KLIPOLO_TEST", "1") };
    let _ = klipolo_capture::platform::backend();
}

/// Set only in the child process `the_guard_reads_klipolo_test` starts, where `KLIPOLO_TEST` is unset.
const CHILD: &str = "KLIPOLO_GUARD_CHILD";

/// The guard reads `KLIPOLO_TEST` itself: set, it panics before anything else; unset, it returns. The unset half runs in
/// a child process (this test binary again, with only this test), as this process's tests set `KLIPOLO_TEST`. The child
/// calls the guard alone, so nothing is captured either way.
#[test]
fn the_guard_reads_klipolo_test() {
    if std::env::var_os(CHILD).is_some() {
        assert!(std::env::var_os("KLIPOLO_TEST").is_none());
        klipolo_capture::guard::assert_real_capture_allowed("x");
        return;
    }
    // SAFETY: as above.
    unsafe { std::env::set_var("KLIPOLO_TEST", "1") };
    let panic =
        std::panic::catch_unwind(|| klipolo_capture::guard::assert_real_capture_allowed("x"))
            .expect_err("the guard returned with KLIPOLO_TEST set");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .unwrap_or_default();
    assert!(message.contains("real capture in a test"), "{message}");

    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "the_guard_reads_klipolo_test", "--nocapture"])
        .env_remove("KLIPOLO_TEST")
        .env(CHILD, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the guard panicked without KLIPOLO_TEST: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran no test: {stdout}"
    );
}
