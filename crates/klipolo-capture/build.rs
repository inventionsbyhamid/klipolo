fn main() {
    // On a Mac the tests' FFmpeg (the `ffmpeg-next` dev-dependency) is a dev prefix of dylibs, and every test and
    // example binary of this crate loads it, so they find the prefix through an rpath. `rustc-link-arg` reaches only
    // this package's own binaries, none of which ship, so every profile may name the machine's path.
    println!("cargo:rerun-if-env-changed=KLIPOLO_FFMPEG_PREFIX");
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if let (true, Ok(prefix)) = (macos, std::env::var("KLIPOLO_FFMPEG_PREFIX")) {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
    }
}
