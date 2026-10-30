//! The engine's media seams: the parts of the OS a GPU,
//! decoder or encoder touches, so they need wgpu or FFmpeg types and live here rather than in `klipolo-platform`. Each
//! file picks its macOS, Windows and Linux arm with `cfg`; shared engine code goes through these and never names an
//! OS type. The macOS presenter itself stays in `present/macos.rs` and `present/native/thread.rs`.
//! - `gpu`: which graphics API the instance uses, and the GPU's class (the preview budget);
//! - `present`: the native view the app hands the presenter (`NativeSurface`);
//! - `decode`: the FFmpeg hardware decoder;
//! - `encode`: the FFmpeg encoders each codec role can use;
//! - `activity`: the screen activity pass's decoder (FFmpeg).

pub mod activity;
pub mod decode;
pub mod encode;
pub mod gpu;
pub mod present;

pub use present::NativeSurface;
