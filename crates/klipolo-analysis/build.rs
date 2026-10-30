fn main() {
    // The dev tools (`klipolo-analyze`, `klipolo-chat-eval`) and this package's tests find the dev FFmpeg prefix on
    // macOS, as the engine's own binaries do (crates/klipolo-engine/build.rs). `rustc-link-arg` reaches only this
    // package's binaries and tests, none of which ship; the app sets its own (crates/klipolo-app/build.rs). On Linux the
    // loader's path finds FFmpeg (scripts/ci-linux.sh).
    println!("cargo:rerun-if-env-changed=KLIPOLO_FFMPEG_PREFIX");
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if let (true, Ok(prefix)) = (macos, std::env::var("KLIPOLO_FFMPEG_PREFIX")) {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
    }
}
