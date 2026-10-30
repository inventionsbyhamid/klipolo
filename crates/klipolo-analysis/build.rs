fn main() {
    // On a Mac the engine's FFmpeg is a dev prefix of dylibs, and `rustc-link-arg` from the engine's own build script
    // reaches only its own binaries: this crate's tests and its dev command find the prefix the same way. Nothing here
    // ships, so every profile may name the machine's path.
    println!("cargo:rerun-if-env-changed=KLIPOLO_FFMPEG_PREFIX");
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if let (true, Ok(prefix)) = (macos, std::env::var("KLIPOLO_FFMPEG_PREFIX")) {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
    }
}
