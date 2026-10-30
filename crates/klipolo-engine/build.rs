fn main() {
    // `rustc-link-arg` only reaches this package's own test/example binaries; crates/klipolo-app/build.rs repeats it
    // for the app. None of them ship, so they find the dev FFmpeg prefix in every profile (`cargo test --release` too);
    // a bundle finds FFmpeg in Contents/Frameworks.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rerun-if-env-changed=KLIPOLO_FFMPEG_PREFIX");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
        if let Ok(prefix) = std::env::var("KLIPOLO_FFMPEG_PREFIX") {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
        }
    }
}
