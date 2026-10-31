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
