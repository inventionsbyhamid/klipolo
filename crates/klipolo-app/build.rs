fn main() {
    // With `bundle.macOS.frameworks` set (the FFmpeg dylibs), tauri-build copies them to `target/Frameworks` and adds
    // the `@executable_path/../Frameworks` rpath, which is how the bundle finds FFmpeg in Contents/Frameworks.
    tauri_build::build();
    // Debug builds and their tests also find the dev FFmpeg prefix. A release binary never names a machine path.
    println!("cargo:rerun-if-env-changed=KLIPOLO_FFMPEG_PREFIX");
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    if let (true, true, Ok(prefix)) = (macos, debug, std::env::var("KLIPOLO_FFMPEG_PREFIX")) {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
    }
}
